//! Camera-free drawing from a stored frame calibration, plus the guided calibration steps (home, pen contact, test pattern).
//! Nothing here needs the camera; the camera may only *propose* a paper position that the test pattern then verifies.

use super::*;
use crate::frame::{self, FrameProfile};

/// What the running job is made of, so progress and time estimates come from the job itself.
pub(super) struct Plan {
    /// cumulative estimated seconds after finishing stroke i
    cum: Vec<f64>,
    total_s: f64,
    stages: Vec<(String, usize)>,
    n: usize,
}

const DRAW_FEED: f64 = 1000.0;
const TRAVEL_FEED: f64 = 3000.0;
/// pen lowered and raised once per stroke (controller z feeds 240 / 300 mm/min over a few mm)
const STROKE_OVERHEAD_S: f64 = 1.8;

/// Estimated seconds per stroke (drawing + travel from the previous end + pen up/down).
pub fn stroke_seconds(strokes: &[Vec<[f64; 2]>], feed: f64) -> Vec<f64> {
    let mut cur: Option<[f64; 2]> = None;
    strokes
        .iter()
        .map(|s| {
            let len: f64 = s.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum();
            let trav = cur.map(|c| ((s[0][0] - c[0]).powi(2) + (s[0][1] - c[1]).powi(2)).sqrt()).unwrap_or(0.0);
            cur = s.last().copied();
            len / feed * 60.0 + trav / TRAVEL_FEED * 60.0 + STROKE_OVERHEAD_S
        })
        .collect()
}

impl Plan {
    fn new(strokes: &[Vec<[f64; 2]>], feed: f64, stages: Vec<(String, usize)>) -> Plan {
        let mut cum = vec![];
        let mut t = 0.0;
        for s in stroke_seconds(strokes, feed) {
            t += s;
            cum.push(t);
        }
        Plan { cum, total_s: t, stages, n: strokes.len() }
    }

    fn stage_at(&self, i: usize) -> &str {
        let mut end = 0;
        for (name, count) in &self.stages {
            end += count;
            if i < end {
                return name;
            }
        }
        self.stages.last().map(|s| s.0.as_str()).unwrap_or("Drawing")
    }
}

/// Progress, time left and the current operation for the main screen. Comes only from the controller's own job progress.
pub(super) fn job_view(plan: Option<&Plan>, run: &crate::ctl::RunView, kind: &str) -> Value {
    let (done, total) = (run.done.min(run.total), run.total);
    let weighted = plan.filter(|p| p.n == total && total > 0);
    let (frac, model_total, model_done) = match weighted {
        Some(p) if done > 0 => (p.cum[done - 1] / p.total_s.max(1e-9), p.total_s, p.cum[done - 1]),
        Some(p) => (0.0, p.total_s, 0.0),
        None => (if total > 0 { done as f64 / total as f64 } else { 0.0 }, 0.0, 0.0),
    };
    let finished = run.state == RunState::Completed;
    let percent = if finished { 100.0 } else { (frac * 1000.0).round() / 10.0 };
    let live = matches!(run.state, RunState::Running | RunState::Pausing | RunState::Stopping);
    // estimate: the model, scaled by how fast the machine has actually been going once there is enough evidence
    let eta = if finished || run.state == RunState::Failed || run.state == RunState::Stopped {
        None
    } else if model_total > 0.0 {
        let pace = if done >= 3 && run.elapsed_s > 10.0 && model_done > 5.0 { (run.elapsed_s / model_done).clamp(0.5, 3.0) } else { 1.0 };
        Some(((model_total - model_done) * pace).max(0.0).round())
    } else if done >= 3 && frac > 0.02 {
        Some((run.elapsed_s / frac * (1.0 - frac)).round())
    } else {
        None
    };
    let stage = weighted.map(|p| p.stage_at(done.min(total.saturating_sub(1)))).unwrap_or("Drawing");
    let operation = match run.state {
        RunState::Running => format!("{stage}: stroke {} of {}", (done + 1).min(total), total),
        RunState::Pausing => "Pausing after the current stroke…".into(),
        RunState::Paused => format!("Paused after stroke {done} of {total}"),
        RunState::Stopping => "Stopping and lifting the pen…".into(),
        RunState::Stopped => "Cancelled".into(),
        RunState::Completed => "Finished".into(),
        RunState::Failed => "Stopped by an error".into(),
    };
    json!({
        "id": run.id, "state": run.state, "kind": kind, "done": done, "total": total, "percent": percent, "etaS": eta,
        "elapsedS": run.elapsed_s, "operation": operation, "stage": stage, "live": live, "resumable": run.resumable,
    })
}

fn pt2(v: &Value) -> Result<[f64; 2], String> {
    let a = v.as_array().filter(|a| a.len() == 2).ok_or("bad point")?;
    Ok([a[0].as_f64().ok_or("bad point")?, a[1].as_f64().ok_or("bad point")?])
}

impl Writer {
    fn frames(&self) -> frame::Store {
        frame::Store::new(&self.dir)
    }

    pub(super) fn active_frame(&self) -> Option<FrameProfile> {
        let name = self.lock().settings.get("activeFrame").and_then(|v| v.as_str()).map(String::from)?;
        self.frames().load(&name).ok()
    }

    /// Frame facts for `state`: the active profile, how much of the safe area the plotter can reach.
    pub(super) fn frame_json(&self, reach: Option<[f64; 4]>) -> Value {
        let name = self.lock().settings.get("activeFrame").and_then(|v| v.as_str()).map(String::from);
        let loaded = name.as_ref().map(|n| self.frames().load(n));
        let (profile, problem) = match loaded {
            Some(Ok(p)) => (Some(p), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };
        let reach_json = profile.as_ref().zip(reach).map(|(p, r)| {
            let (h, f) = p.reach_report(r);
            json!({ "usableHeightMm": (h * 10.0).round() / 10.0, "fraction": f })
        });
        json!({
            "active": profile, "problem": problem, "activeName": name, "reach": reach_json,
            "prior": frame::prior_origin(&self.ctl.root()),
            "ready": profile.as_ref().map(|p| p.verified).unwrap_or(false),
        })
    }

    pub fn frame_list(&self) -> Value {
        json!({ "profiles": self.frames().list(), "active": self.lock().settings.get("activeFrame").cloned().unwrap_or(Value::Null) })
    }

    pub fn frame_save(&self, mut p: FrameProfile, activate: bool) -> Result<Value, String> {
        p.name = p.name.trim().to_string();
        let existing = self.frames().load(&p.name).ok();
        if let Some(e) = &existing {
            p.created = e.created;
        }
        p.updated = frame::now_s();
        p.refresh();
        self.frames().save(&p)?;
        diag::info("frame", format!("saved calibration profile '{}' (verified={}, origin {:?}, rotation {}+{:.2}, contact z {:.2})", p.name, p.verified, p.origin_pen, p.rotation, p.skew_deg, p.surface_z));
        if activate {
            self.set_setting("activeFrame", json!(p.name))?;
        }
        Ok(json!(p))
    }

    pub fn frame_activate(&self, name: &str) -> Result<(), String> {
        self.frames().load(name)?;
        self.set_setting("activeFrame", json!(name))
    }

    pub fn frame_delete(&self, name: &str) -> Result<(), String> {
        self.frames().delete(name)?;
        if self.lock().settings.get("activeFrame").and_then(|v| v.as_str()) == Some(name) {
            self.set_setting("activeFrame", Value::Null)?;
        }
        Ok(())
    }

    /// Validate a draft profile and describe how it lies relative to the plotter's reach (for the calibration diagram).
    pub fn frame_check(&self, p: &FrameProfile) -> Result<Value, String> {
        p.check()?;
        let reach = self.ctl.fresh_status().ok().and_then(|s| s.bbox);
        let rr = reach.map(|r| {
            let (h, f) = p.reach_report(r);
            json!({ "usableHeightMm": (h * 10.0).round() / 10.0, "fraction": f })
        });
        let mut warnings = vec![];
        if let Some(r) = reach {
            let (_, f) = p.reach_report(r);
            if f < 0.999 {
                warnings.push(format!("The plotter cannot reach all of the safe area ({:.0}% of it). Drawings are limited to the part it can reach.", f * 100.0));
            }
            if f < 0.2 {
                return Err("Almost none of this sheet is inside the plotter's reach. Check the paper position and rotation.".into());
            }
        }
        Ok(json!({ "cornersPen": p.corners_pen(), "reach": reach, "reachReport": rr, "warnings": warnings }))
    }

    /// A paper position suggested by the camera. Only a proposal: the test pattern verifies it.
    pub fn frame_propose(&self) -> Result<Value, String> {
        let (geom, cal_ok, cam_ok) = {
            let g = self.lock();
            (g.tracker.locked.clone(), g.cal.is_some(), g.cam.reachable)
        };
        if !cam_ok {
            return Err("The camera is not available. You can enter the paper position by hand instead.".into());
        }
        if !cal_ok {
            return Err("The camera has not been calibrated (Settings > Camera). You can enter the paper position by hand instead.".into());
        }
        let geom = geom.ok_or("The camera does not see a steady sheet. Seat the paper in the frame, move the plotter to its camera position and try again.")?;
        let (p, notes) = frame::propose_from_geom(&geom)?;
        Ok(json!({ "profile": p, "notes": notes, "accuracyMm": 2.0, "note": "A camera picture alone is only approximate. The test pattern checks it against the real pen." }))
    }

    // ── homing ──
    /// Guarded XY homing; Z is never touched. `confirmed` must come from the user's explicit confirmation of "pen raised, head clear".
    pub fn home_xy(&self, confirmed: bool) -> Result<String, String> {
        if !confirmed {
            return Err("Homing needs your confirmation that the pen is clear of the frame and paper.".into());
        }
        if self.is_job_active() {
            return Err("A job is running.".into());
        }
        diag::info("frame", "XY homing requested by the user");
        self.ctl.home_xy()
    }

    fn is_job_active(&self) -> bool {
        self.current_run().map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) || self.lock().preparing
    }

    // ── pen contact (Z) ──
    /// Move (pen up) over the middle of the safe area so the contact height can be found by hand-stepping the pen down.
    pub fn probe_begin(&self, p: &FrameProfile, confirmed: bool) -> Result<Value, String> {
        if !confirmed {
            return Err("Confirm that the sheet is seated and taped in the frame first.".into());
        }
        p.check()?;
        if self.is_job_active() {
            return Err("A job is running.".into());
        }
        let st = self.ctl.fresh_status()?;
        let why = crate::ctl::unsafe_reasons(&st);
        if !why.is_empty() {
            return Err(format!("The plotter is not ready: {}", why.join("; ")));
        }
        let reach = st.bbox.ok_or("the plotter reports no working area")?;
        // the middle of the safe area; if that is unreachable the sheet is placed wrongly
        let c = p.page_to_pen_pt([p.page_w / 2.0, p.page_h / 2.0]);
        if c[0] < reach[0] + 2.0 || c[0] > reach[1] - 2.0 || c[1] < reach[2] + 2.0 || c[1] > reach[3] - 2.0 {
            return Err("The middle of the sheet is outside the plotter's reach. Check the paper position first.".into());
        }
        self.ctl.confirm_sheet("frame-probe", true)?;
        self.ctl.pen_up()?;
        self.ctl.goto_xy(c[0], c[1])?;
        self.lock().probe = Some(c);
        diag::info("frame", format!("probing pen contact at pen X{:.1} Y{:.1}", c[0], c[1]));
        let z = self.ctl.fresh_status()?.pos.map(|q| q[2]);
        Ok(json!({ "pen": c, "z": z, "zfloor": st.zfloor }))
    }

    /// Move the pen straight down/up by at most 1 mm over the probe point. The controller's own floor still applies.
    pub fn probe_step(&self, dz: f64) -> Result<Value, String> {
        let probe = self.lock().probe.ok_or("Start the pen-height step first.")?;
        if !dz.is_finite() || dz == 0.0 || dz.abs() > 1.0 {
            return Err("A step is at most 1 mm.".into());
        }
        let st = self.ctl.fresh_status()?;
        let pos = st.pos.ok_or("position unknown")?;
        if (pos[0] - probe[0]).abs() > 1.0 || (pos[1] - probe[1]).abs() > 1.0 {
            return Err("The pen is no longer over the test point; start the pen-height step again.".into());
        }
        self.ctl.z_step(dz, None)?;
        let z = self.ctl.fresh_status()?.pos.map(|q| q[2]);
        Ok(json!({ "z": z }))
    }

    /// Read the pen height as the contact height (when `record`), then lift the pen. Never leaves the pen down.
    pub fn probe_finish(&self, record: bool) -> Result<Value, String> {
        let st = self.ctl.fresh_status()?;
        let z = st.pos.ok_or("position unknown")?[2];
        let zfloor = st.zfloor.unwrap_or(0.0);
        if record && (z < zfloor - 1e-6 || z > 80.0) {
            return Err(format!("A contact height of {z:.2} mm is not plausible (the controller's lowest allowed height is {zfloor:.2} mm)."));
        }
        let lifted = self.ctl.pen_up();
        self.lock().probe = None;
        lifted?;
        Ok(json!({ "z": if record { Some(z) } else { None } }))
    }

    // ── test pattern + verification ──
    pub fn frame_adjust(&self, p: &FrameProfile, inset: f64, d1: [f64; 2], d2: [f64; 2]) -> Result<Value, String> {
        let (q, scale_err) = frame::adjust(p, inset, d1, d2)?;
        let mut warnings = vec![];
        if scale_err.abs() > 1.0 {
            warnings.push(format!("The two marks are {:.1} mm further apart/closer than expected. A rigid correction cannot fix that: check the page size and that the sheet lies flat.", scale_err.abs()));
        }
        Ok(json!({ "profile": q, "warnings": warnings }))
    }

    pub fn frame_test(self: &Arc<Self>, p: FrameProfile) -> Result<Value, String> {
        p.check()?;
        let reach = self.ctl.fresh_status()?.bbox.ok_or("the plotter reports no working area")?;
        let (strokes, inset) = frame::test_pattern(&p, reach);
        if strokes.len() < 2 {
            return Err("There is no room inside the plotter's reach for the test pattern; check the paper position.".into());
        }
        let n = strokes.len();
        self.start_frame_job(p, strokes, format!("test-{}", frame::now_s()), "Calibration test pattern".into(), "test", vec![("Test pattern".into(), n)])?;
        Ok(json!({ "inset": inset }))
    }

    // ── drawing ──
    /// Draw strokes (paper space, mm, origin top-left, +y down) using the active, verified frame calibration. No camera involved.
    pub fn draw(self: &Arc<Self>, strokes: Vec<Vec<[f64; 2]>>, key: String, name: String, stages: Vec<(String, usize)>) -> Result<(), String> {
        let p = self.active_frame().ok_or("No calibrated frame yet. Open Calibration and set up your frame once.")?;
        if !p.verified {
            return Err("The frame calibration has not been verified with the test pattern yet.".into());
        }
        let strokes: Vec<Vec<[f64; 2]>> = strokes.into_iter().filter(|s| s.len() >= 2).collect();
        self.start_frame_job(p, strokes, key, name, "draw", stages)
    }

    #[allow(clippy::too_many_arguments)]
    fn start_frame_job(self: &Arc<Self>, p: FrameProfile, strokes: Vec<Vec<[f64; 2]>>, key: String, name: String, kind: &'static str, stages: Vec<(String, usize)>) -> Result<(), String> {
        {
            let mut g = self.lock();
            if g.preparing {
                return Err("A job is already being prepared.".into());
            }
            if let Some((k, t)) = &g.last_key {
                if *k == key && t.elapsed() < Duration::from_secs(20) {
                    return Err("This was just sent. Wait for it to finish.".into());
                }
            }
            g.failure = None;
            g.finished_note = None;
            g.kind = kind;
            g.preparing = true;
        }
        let me = self.clone();
        let spawn = std::thread::Builder::new().name("frame-job".into()).spawn(move || {
            if let Err(e) = me.frame_job_inner(&p, strokes, key, name, kind, stages) {
                me.fail("prepare", "Could not start drawing.", e);
            }
        });
        if let Err(e) = spawn {
            self.lock().preparing = false;
            return Err(e.to_string());
        }
        Ok(())
    }

    fn frame_job_inner(&self, p: &FrameProfile, strokes: Vec<Vec<[f64; 2]>>, key: String, name: String, kind: &'static str, stages: Vec<(String, usize)>) -> Result<(), String> {
        diag::info("draw", format!("{kind} requested on frame '{}': {} strokes", p.name, strokes.len()));
        let cs = self.ctl.snapshot();
        let st = self.ctl.fresh_status()?;
        let why = crate::ctl::unsafe_reasons(&st);
        if !why.is_empty() {
            return Err(format!("plotter not ready: {}", why.join("; ")));
        }
        if cs.run.as_ref().map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
            return Err("a job is already running".into());
        }
        let reach = st.bbox.ok_or("the plotter reports no working area")?;
        frame::validate(&strokes, p, reach)?;
        let c = p.page_to_pen_pt([p.page_w / 2.0, p.page_h / 2.0]);
        let sheet = Sheet {
            id: format!("frame:{}", frame::slug(&p.name)),
            label: format!("frame '{}' {:.0}x{:.0} mm, contact z {:.2}", p.name, p.page_w, p.page_h, p.surface_z),
            a: p.matrix(),
            plane: Plane { x0: c[0], y0: c[1], z0: p.surface_z, gx: 0.0, gy: 0.0 },
            page: [p.page_w, p.page_h],
            provisional: !p.verified,
            warnings: vec![format!("pen-down height is the contact height measured at calibration ({:.2} mm), assuming a flat sheet", p.surface_z)],
        };
        let prof = crate::ctl::read_profiles(&self.ctl.root());
        let depth = self.setting_f("depth", prof.write_depth_mm.unwrap_or(0.5));
        let feed = self.setting_f("feed", DRAW_FEED);
        let view = self.ctl.prepare_sheet(name, &sheet, &strokes, p.page_w, p.page_h, depth, feed)?;
        if !view.accepted {
            return Err(format!("the plotter's safety check rejected the job: {}", view.reason.unwrap_or_default()));
        }
        diag::info("draw", format!("job {} checked: {} strokes, {:.0} mm, pen bbox {:?}", view.id, view.strokes, view.draw_len_mm, view.bbox_pen));
        self.ctl.confirm_sheet(&sheet.id, true)?; // pressing Draw (or the calibration button) is the explicit confirmation
        self.ctl.start()?;
        let plan = Plan::new(&strokes, feed, if stages.iter().map(|s| s.1).sum::<usize>() == strokes.len() { stages } else { vec![("Drawing".into(), strokes.len())] });
        let mut g = self.lock();
        g.plan = Some(plan);
        g.preparing = false;
        g.own_run = Some(view.id.clone());
        g.last_key = Some((key, Instant::now()));
        diag::info("draw", format!("job {} started", view.id));
        Ok(())
    }

    fn cam_url(&self, side: bool) -> String {
        if side {
            self.lock().settings.get("camera2Url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_CAMERA2).to_string()
        } else {
            self.camera_url()
        }
    }

    /// Health of both camera hubs, each checked on its own so one failing never hides the other.
    pub fn cameras(&self) -> Value {
        let one = |side: bool| {
            let url = self.cam_url(side);
            let label = if side { "Side Camera" } else { "Paper Camera" };
            match paper::http_get(&url, "/api/stats", Duration::from_secs(2)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                Some(v) => {
                    let age = v["camera"]["frame_age_s"].as_f64();
                    let state = v["camera"]["state"].as_str().unwrap_or("unknown").to_string();
                    let live = state == "streaming" && age.map(|a| a < 3.0).unwrap_or(false);
                    json!({ "id": if side { "side" } else { "paper" }, "label": label, "url": url, "hub": true, "live": live, "state": state, "reason": v["camera"]["reason"], "frameAgeS": age,
                        "size": v["mode"]["size"], "fps": v["mode"]["fps"], "measuredFps": v["capture_fps"], "wanted": self.cam_mode(side) })
                }
                None => json!({ "id": if side { "side" } else { "paper" }, "label": label, "url": url, "hub": false, "live": false, "state": "offline", "reason": "The camera service is not running.", "frameAgeS": null }),
            }
        };
        json!([one(false), one(true)])
    }

    /// The mode the user chose for a camera: "auto" (highest the hub offers) or "WxH@fps". Persisted in settings.
    pub fn cam_mode(&self, side: bool) -> String {
        let k = if side { "camModeSide" } else { "camModePaper" };
        self.lock().settings.get(k).and_then(|v| v.as_str()).unwrap_or("auto").to_string()
    }

    fn parse_mode(m: &str) -> Result<(u32, u32, u32), String> {
        if m == "auto" {
            return Ok((1920, 1080, 30));
        }
        let (sz, fps) = m.split_once('@').ok_or("bad mode")?;
        let (w, h) = sz.split_once('x').ok_or("bad mode")?;
        let t = |x: &str| x.parse::<u32>().map_err(|_| "bad mode".to_string());
        let (w, h, f) = (t(w)?, t(h)?, t(fps)?);
        if ![(1920, 1080), (1280, 720), (800, 600), (640, 480)].contains(&(w, h)) || !(5..=30).contains(&f) {
            return Err("that camera mode is not supported".into());
        }
        Ok((w, h, f))
    }

    /// Reconfigure the real stream (the hub restarts its capture at that size), and remember the choice.
    pub fn camera_set_mode(&self, side: bool, mode: &str) -> Result<Value, String> {
        let (w, h, f) = Self::parse_mode(mode)?;
        let body = json!({ "size": format!("{w}x{h}"), "fps": f }).to_string();
        paper::http_post_json(&self.cam_url(side), "/api/mode", &body, Duration::from_secs(8)).map_err(|e| format!("The camera did not accept the mode ({e}). Changes are limited to one every few seconds."))?;
        self.set_setting(if side { "camModeSide" } else { "camModePaper" }, json!(mode))?;
        diag::info("camera", format!("{} set to {w}x{h}@{f}", if side { "side camera" } else { "paper camera" }));
        Ok(json!({ "size": format!("{w}x{h}"), "fps": f }))
    }

    /// Keep each camera in the chosen (or best) mode, e.g. after the hub restarted. Never blocks drawing.
    pub(super) fn apply_camera_modes(&self) {
        for side in [false, true] {
            let url = self.cam_url(side);
            let Some(v) = paper::http_get(&url, "/api/stats", Duration::from_secs(2)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
            let Ok((w, h, f)) = Self::parse_mode(&self.cam_mode(side)) else { continue };
            if v["mode"]["size"].as_str() == Some(&format!("{w}x{h}")) && v["mode"]["fps"].as_u64() == Some(f as u64) {
                continue;
            }
            let key = if side { "modeTrySide" } else { "modeTryPaper" };
            let due = { let mut g = self.lock(); let ok = g.svc.mode_try.get(key).map(|t: &Instant| t.elapsed() > Duration::from_secs(60)).unwrap_or(true); if ok { g.svc.mode_try.insert(key, Instant::now()); } ok };
            if due {
                let mode = self.cam_mode(side);
                if let Err(e) = self.camera_set_mode(side, &mode) {
                    diag::warn("camera", format!("could not apply camera mode: {e}"));
                }
            }
        }
    }

    /// Ask a camera hub to retry its webcam after a USB hiccup.
    pub fn camera_resume(&self, side: bool) -> Result<(), String> {
        paper::http_post(&self.cam_url(side), "/api/resume", Duration::from_secs(5)).map(|_| ())
    }

    pub fn pause(&self) -> Result<(), String> {
        diag::info("draw", "pause requested");
        self.ctl.pause()
    }

    pub fn resume(&self) -> Result<(), String> {
        diag::info("draw", "resume requested");
        self.ctl.resume()
    }
}

pub(super) fn strokes_of(a: &Value) -> Result<Vec<Vec<[f64; 2]>>, String> {
    let arr = a.get("strokes").and_then(|s| s.as_array()).ok_or("strokes missing")?;
    if arr.len() > 40000 {
        return Err("too many strokes".into());
    }
    arr.iter().map(|s| s.as_array().ok_or("bad stroke".to_string()).and_then(|pts| pts.iter().map(|p| pt2(p).map_err(String::from)).collect())).collect()
}

pub(super) fn stages_of(a: &Value) -> Vec<(String, usize)> {
    a.get("stages")
        .and_then(|s| s.as_array())
        .map(|v| v.iter().filter_map(|s| Some((s.get("label")?.as_str()?.chars().take(40).collect::<String>(), s.get("count")?.as_u64()? as usize))).collect())
        .unwrap_or_default()
}

pub(super) fn profile_of(a: &Value) -> Result<FrameProfile, String> {
    let mut p: FrameProfile = serde_json::from_value(a.get("profile").cloned().ok_or("profile missing")?).map_err(|e| format!("bad profile: {e}"))?;
    p.refresh();
    Ok(p)
}

pub(super) fn pt(a: &Value, k: &str) -> Result<[f64; 2], String> {
    pt2(a.get(k).ok_or_else(|| format!("{k} missing"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_weights_progress_and_picks_the_stage() {
        let strokes: Vec<Vec<[f64; 2]>> = (0..10).map(|i| vec![[10.0, 10.0 + i as f64], [60.0, 10.0 + i as f64]]).collect();
        let p = Plan::new(&strokes, 1000.0, vec![("Ruling the page".into(), 4), ("Writing text".into(), 6)]);
        assert_eq!(p.stage_at(0), "Ruling the page");
        assert_eq!(p.stage_at(3), "Ruling the page");
        assert_eq!(p.stage_at(4), "Writing text");
        assert!(p.total_s > 10.0 * STROKE_OVERHEAD_S);
        let run = |done: usize, state: RunState| crate::ctl::RunView {
            id: "1".into(), name: "t".into(), state, total: 10, done, segment_base: 0, segment_total: 10, message: String::new(), error: None, elapsed_s: 0.0, resumable: false,
        };
        let v = job_view(Some(&p), &run(5, RunState::Running), "draw");
        let pct = v["percent"].as_f64().unwrap();
        assert!(pct > 45.0 && pct < 55.0, "{pct}");
        assert_eq!(v["stage"], "Writing text");
        assert!(v["etaS"].as_f64().unwrap() > 0.0);
        let done = job_view(Some(&p), &run(10, RunState::Completed), "draw");
        assert_eq!(done["percent"].as_f64().unwrap(), 100.0);
        assert!(done["etaS"].is_null());
        // a plan that does not match the controller's stroke count falls back to plain counting
        let other = job_view(Some(&p), &crate::ctl::RunView { total: 20, done: 5, ..run(5, RunState::Running) }, "draw");
        assert_eq!(other["percent"].as_f64().unwrap(), 25.0);
        let paused = job_view(Some(&p), &run(3, RunState::Paused), "draw");
        assert!(paused["operation"].as_str().unwrap().starts_with("Paused"));
    }

    #[test]
    fn strokes_and_stages_parse() {
        let a = json!({ "strokes": [[[1.0, 2.0], [3.0, 4.0]]], "stages": [{ "label": "Ruling the page", "count": 1 }] });
        assert_eq!(strokes_of(&a).unwrap(), vec![vec![[1.0, 2.0], [3.0, 4.0]]]);
        assert_eq!(stages_of(&a), vec![("Ruling the page".to_string(), 1)]);
        assert!(strokes_of(&json!({ "strokes": [[[1.0]]] })).is_err());
    }
}
