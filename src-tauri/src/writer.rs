//! The "Write" service: camera + calibration + controller -> one simple state, and the Write/Stop actions.
//! Shared by the desktop app (one Tauri command) and the browser service (HTTP), see `call`.

use crate::ctl::{Ctl, Plane, RunState, Sheet};
use crate::diag;
use crate::paper::{self, CalPoint, CameraCal, PaperGeom, Seen, Tracker};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod fops;
mod ops;

const DEFAULT_CAMERA: &str = "http://127.0.0.1:7792";
/// Second, view-only camera hub (no paper tracking or calibration): the side view of the plotter.
const DEFAULT_CAMERA2: &str = "http://127.0.0.1:7796";

#[derive(Clone, Debug)]
struct Failure {
    stage: String,
    message: String,
    detail: String,
    at_ms: u128,
}

#[derive(Default)]
struct Cam {
    reachable: bool,
    error: Option<String>,
    seen: Seen,
    last_ok: Option<Instant>,
    polls: u64,
    last_analysis: Option<Value>,
}

/// Service supervision bookkeeping (camera hub + controller daemon).
#[derive(Default)]
struct Svc {
    cam_down_since: Option<Instant>,
    cam_started: Option<Instant>,
    cam_tries: u32,
    cam_hub_state: Option<String>,
    cam_streaming: bool,
    ctl_down_since: Option<Instant>,
    ctl_started: Option<Instant>,
    ctl_tries: u32,
    ctl_note: Option<String>,
    last_slow: Option<Instant>,
    pose_try: Option<Instant>,
    post_done: Option<String>,
    motion_ok: Option<Instant>,
    mode_try: std::collections::HashMap<&'static str, Instant>,
    motion: Value,
    ink: Value,
}

struct Inner {
    svc: Svc,
    settings: Map<String, Value>,
    cal: Option<CameraCal>,
    cam: Cam,
    tracker: Tracker,
    preparing: bool,
    failure: Option<Failure>,
    /// job id (ctl run id) of a run this service started, and whether its end was already reported
    own_run: Option<String>,
    finished_note: Option<String>,
    last_key: Option<(String, Instant)>,
    kind: &'static str, // "write" | "marks"
    frame: Option<(Instant, String)>,
    frame2: Option<(Instant, String)>,
    /// weights of the running frame job (progress, time left, current operation)
    plan: Option<fops::Plan>,
    /// pen X/Y of the point where the contact height is being found
    probe: Option<[f64; 2]>,
}

pub struct Writer {
    pub ctl: Arc<Ctl>,
    dir: PathBuf,
    st: Mutex<Inner>,
    pub web_url: Mutex<Option<String>>,
}

fn rd(p: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}
fn wr(p: &std::path::Path, v: &Value) -> Result<(), String> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}

/// Six calibration marks around `c`, deliberately NOT a symmetric grid so the camera's orientation is unambiguous.
pub fn mark_layout(c: [f64; 2], dx: f64, dy: f64) -> Vec<[f64; 2]> {
    let u = [[-1.0, 1.0], [0.1, 0.78], [1.0, 1.0], [-0.8, -1.0], [0.35, -0.2], [0.95, -0.9]];
    u.iter().map(|p| [(c[0] + p[0] * dx).round(), (c[1] + p[1] * dy).round()]).collect()
}

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

impl Writer {
    pub fn new(dir: PathBuf, ctl: Arc<Ctl>) -> Arc<Writer> {
        let mut settings: Map<String, Value> = rd(&dir.join("ctop-settings.json")).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        // carry over the controller location the older UI saved
        if let Some(old) = rd(&dir.join("settings.json")) {
            for k in ["controllerSocket", "controllerDir"] {
                if !settings.contains_key(k) {
                    if let Some(v) = old.get(k) {
                        settings.insert(k.into(), v.clone());
                    }
                }
            }
        }
        if let Some(z) = settings.get("travelZ").and_then(|v| v.as_f64()) {
            crate::ctl::set_travel_z(z);
        }
        let cal = rd(&dir.join("camera_cal.json")).and_then(|v| serde_json::from_value::<CameraCal>(v).ok());
        let s = |k: &str| settings.get(k).and_then(|v| v.as_str()).map(String::from);
        ctl.set_config(s("controllerSocket"), s("controllerDir"));
        if let Some(c) = &cal {
            diag::info("calibration", format!("loaded camera calibration (rms {:.2} mm, pose {:?})", c.rms_mm, c.pose));
        }
        Arc::new(Writer {
            ctl,
            dir,
            st: Mutex::new(Inner { svc: Svc::default(), settings, cal, cam: Cam::default(), tracker: Tracker::default(), preparing: false, failure: None, own_run: None, finished_note: None, last_key: None, kind: "write", frame: None, frame2: None, plan: None, probe: None }),
            web_url: Mutex::new(None),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── settings ──
    fn setting_f(&self, k: &str, d: f64) -> f64 {
        self.lock().settings.get(k).and_then(|v| v.as_f64()).unwrap_or(d)
    }
    fn camera_url(&self) -> String {
        self.lock().settings.get("cameraUrl").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_CAMERA).to_string()
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        if key.len() > 64 || key.is_empty() {
            return Err("bad setting name".into());
        }
        match key {
            "cameraUrl" => {
                paper::parse_base(value.as_str().unwrap_or(""))?;
            }
            "pageRotation" => {
                let r = value.as_f64().ok_or("rotation must be a number")?;
                if ![0.0, 90.0, 180.0, 270.0].contains(&r) {
                    return Err("rotation must be 0, 90, 180 or 270".into());
                }
            }
            "surfaceZ" => {
                let z = value.as_f64().ok_or("surface height must be a number")?;
                if !(0.0..=150.0).contains(&z) {
                    return Err("surface height out of range".into());
                }
            }
            "depth" => {
                if !(0.2..=1.0).contains(&value.as_f64().ok_or("depth must be a number")?) {
                    return Err("pen pressure depth must be 0.2 - 1.0 mm".into());
                }
            }
            "travelZ" => {
                let z = value.as_f64().ok_or("travel height must be a number")?;
                if !(18.0..=60.0).contains(&z) {
                    return Err("travel height must be 18 - 60 mm".into());
                }
                crate::ctl::set_travel_z(z);
            }
            "feed" => {
                if !(100.0..=1500.0).contains(&value.as_f64().ok_or("speed must be a number")?) {
                    return Err("writing speed must be 100 - 1500 mm/min".into());
                }
            }
            "edgeMm" => {
                if !(2.0..=40.0).contains(&value.as_f64().ok_or("edge must be a number")?) {
                    return Err("minimum edge distance must be 2 - 40 mm".into());
                }
            }
            "controllerSocket" | "controllerDir" => {
                let s = value.as_str().ok_or("must be text")?.to_string();
                let (a, b) = if key == "controllerSocket" { (Some(s), None) } else { (None, Some(s)) };
                self.ctl.set_config(a, b);
            }
            _ => {}
        }
        let snapshot = {
            let mut g = self.lock();
            g.settings.insert(key.into(), value);
            if matches!(key, "pageRotation") {
                g.tracker.reset();
            }
            Value::Object(g.settings.clone())
        };
        wr(&self.dir.join("ctop-settings.json"), &snapshot)
    }

    pub fn settings(&self) -> Value {
        Value::Object(self.lock().settings.clone())
    }

    // ── camera polling ──
    pub fn start(self: &Arc<Self>) {
        self.ctl.start_poller();
        let me = self.clone();
        std::thread::spawn(move || loop {
            me.poll_camera();
            me.supervise();
            std::thread::sleep(Duration::from_millis(500));
        });
    }

    pub fn poll_camera(&self) {
        let url = self.camera_url();
        let res = paper::http_get(&url, "/api/analysis", Duration::from_secs(3)).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| format!("camera analysis unreadable: {e}")));
        let rot = self.setting_f("pageRotation", 0.0);
        let mut g = self.lock();
        g.cam.polls += 1;
        match res {
            Ok(v) => {
                if !g.cam.reachable {
                    diag::info("camera", format!("camera service reachable at {url}"));
                }
                g.cam.reachable = true;
                g.cam.error = None;
                g.cam.last_ok = Some(Instant::now());
                g.cam.seen = paper::parse_analysis(&v);
                g.cam.last_analysis = Some(v);
                let cal = g.cal.clone();
                let seen = g.cam.seen.clone();
                g.tracker.update(&seen, cal.as_ref(), rot);
            }
            Err(e) => {
                if g.cam.reachable || g.cam.error.is_none() {
                    diag::warn("camera", e.clone());
                }
                g.cam.reachable = false;
                g.cam.error = Some(e);
                g.cam.seen = Seen::default();
                g.tracker.reset();
            }
        }
    }

    /// Latest JPEG as base64. `side` picks the second (view-only) camera instead of the paper-tracking one.
    pub fn frame(&self, side: bool) -> Result<Value, String> {
        {
            let g = self.lock();
            if let Some((t, d)) = if side { &g.frame2 } else { &g.frame } {
                if t.elapsed() < Duration::from_millis(120) {
                    return Ok(json!({ "jpeg": d }));
                }
            }
        }
        let url = if side {
            self.lock().settings.get("camera2Url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_CAMERA2).to_string()
        } else {
            self.camera_url()
        };
        let bytes = paper::http_get(&url, "/frame.jpg", Duration::from_secs(4))?;
        if bytes.len() < 200 || bytes[0] != 0xFF {
            return Err("camera returned no image".into());
        }
        let d = b64(&bytes);
        let mut g = self.lock();
        if side { g.frame2 = Some((Instant::now(), d.clone())) } else { g.frame = Some((Instant::now(), d.clone())) }
        Ok(json!({ "jpeg": d }))
    }

    // ── state ──
    fn current_run(&self) -> Option<crate::ctl::RunView> {
        self.ctl.snapshot().run
    }

    pub fn state(&self) -> Value {
        let cs = self.ctl.snapshot();
        let prof = crate::ctl::read_profiles(&self.ctl.root());
        let mut blockers: Vec<String> = vec![];
        let mut setup: Vec<&str> = vec![];

        // controller
        let ctl_ready = cs.reachable && cs.unsafe_reasons.is_empty();
        if !cs.reachable {
            setup.push("controller");
            blockers.push(format!("The plotter controller is not reachable ({}).", cs.error.clone().unwrap_or_default()));
        } else if !cs.unsafe_reasons.is_empty() {
            blockers.push(format!("Plotter not ready: {}", cs.unsafe_reasons.join("; ")));
        }

        let frame_json = self.frame_json(cs.status.as_ref().and_then(|s| s.bbox));
        let frame_ready = frame_json["ready"].as_bool().unwrap_or(false);
        let mut g = self.lock();
        // camera
        if !g.cam.reachable {
            setup.push("camera");
            blockers.push("The camera is not available.".into());
        }
        // calibration + pose
        let pos = cs.status.as_ref().and_then(|s| s.pos);
        let cal_json = g.cal.as_ref().map(|c| {
            let delta = pos.map(|p| c.pose_delta([p[0], p[1]]));
            json!({ "rmsMm": c.rms_mm, "looMm": c.loo_mm, "pose": c.pose, "created": c.created, "poseDeltaMm": delta, "poseOk": delta.map(|d| d <= paper::POSE_TOL_MM).unwrap_or(false) })
        });
        let pose_ok = cal_json.as_ref().and_then(|c| c["poseOk"].as_bool()).unwrap_or(false);
        let default_pose = prof.camera_obs_pose.unwrap_or([270.0, 305.0]);
        if g.cal.is_none() {
            setup.push("calibration");
            blockers.push("The camera has not been calibrated yet.".into());
        } else if !pose_ok && cs.reachable {
            blockers.push("The plotter is not in its camera position. Move it there to see the paper.".into());
        }
        // paper
        let paper_state;
        let mut paper_json = Value::Null;
        if g.cal.is_none() {
            paper_state = "uncalibrated";
        } else if !g.cam.reachable {
            paper_state = "nocamera";
        } else if !g.cam.seen.found {
            paper_state = "none";
        } else if let Some(e) = &g.tracker.last_err {
            paper_state = "invalid";
            blockers.push(format!("Paper not usable: {e}"));
        } else if g.tracker.locked.is_some() {
            paper_state = "ready";
        } else {
            paper_state = "moving";
        }
        if paper_state == "none" {
            blockers.push("No paper detected under the camera.".into());
        } else if paper_state == "moving" {
            blockers.push("Paper is moving or not fully visible. Hold still…".into());
        }
        if let Some(l) = &g.tracker.locked {
            paper_json = serde_json::to_value(l).unwrap_or(Value::Null);
        }
        let edge = g.settings.get("edgeMm").and_then(|v| v.as_f64()).unwrap_or(6.0);
        let lock_id = g.tracker.lock_id;

        // job phase
        let run = cs.run.clone();
        let own = g.own_run.clone();
        let mut phase = "ready";
        let mut message = String::from("Ready.");
        let mut progress = Value::Null;
        if g.preparing {
            phase = "preparing";
            message = "Preparing paper…".into();
        } else if let Some(r) = run.as_ref().filter(|r| Some(&r.id) == own.as_ref() || r.state == RunState::Running) {
            progress = json!({ "done": r.done, "total": r.total });
            match r.state {
                RunState::Running | RunState::Pausing | RunState::Stopping => {
                    phase = "writing";
                    message = if r.state == RunState::Stopping { "Stopping…".into() } else { "Writing…".into() };
                }
                RunState::Completed => {
                    phase = "finished";
                    message = match g.kind {
                        "marks" => "Marks drawn.".into(),
                        "test" => "Test pattern drawn.".into(),
                        _ => "Finished.".into(),
                    };
                }
                RunState::Paused => {
                    phase = "paused";
                    message = "Paused.".into();
                }
                RunState::Stopped => message = "Stopped.".into(),
                RunState::Failed => {
                    if g.failure.is_none() {
                        g.failure = Some(Failure { stage: "writing".into(), message: "The plotter stopped unexpectedly.".into(), detail: r.error.clone().unwrap_or_else(|| r.message.clone()), at_ms: diag::now_ms() });
                        diag::error("job", format!("run failed: {:?} {}", r.error, r.message));
                    }
                }
            }
        }
        let job_json = run.as_ref().filter(|r| Some(&r.id) == own.as_ref() || r.state == RunState::Running).map(|r| fops::job_view(g.plan.as_ref(), r, g.kind)).unwrap_or(Value::Null);
        let failure = g.failure.clone();
        if let Some(f) = &failure {
            phase = "error";
            message = f.message.clone();
        }
        let ack_note = g.finished_note.clone();
        let cam_json = json!({
            "url": g.settings.get("cameraUrl").cloned().unwrap_or(json!(DEFAULT_CAMERA)),
            "reachable": g.cam.reachable, "error": g.cam.error, "streaming": g.svc.cam_streaming, "hubState": g.svc.cam_hub_state, "paperSeen": g.cam.seen.found,
            "lastOkMs": g.cam.last_ok.map(|t| t.elapsed().as_millis() as u64),
        });
        let services = json!({ "cameraHub": g.svc.cam_hub_state, "cameraStreaming": g.svc.cam_streaming, "cameraStartTries": g.svc.cam_tries, "controllerStartTries": g.svc.ctl_tries,
            "controllerNote": g.svc.ctl_note, "motionCheck": g.svc.motion, "motionVerifiedSecsAgo": g.svc.motion_ok.map(|t| t.elapsed().as_secs()), "completionCheck": g.svc.ink });
        if g.cam.reachable && !g.svc.cam_streaming && g.svc.cam_hub_state.is_some() {
            blockers.push(format!("The camera is not delivering images ({}). Another program may be using it.", g.svc.cam_hub_state.clone().unwrap_or_default()));
        }
        let settings = Value::Object(g.settings.clone());
        let has_cal = g.cal.is_some();
        drop(g);

        let busy = matches!(phase, "preparing" | "writing") || cs.run.as_ref().map(|r| r.state == RunState::Running || r.state == RunState::Stopping || r.state == RunState::Pausing).unwrap_or(false);
        let can_write = ctl_ready && has_cal && pose_ok && paper_state == "ready" && !busy && phase != "error";
        // drawing from the stored frame needs the plotter and a verified calibration, never the camera
        let mut draw_blockers: Vec<String> = vec![];
        if !cs.reachable {
            draw_blockers.push(format!("The plotter controller is not reachable ({}).", cs.error.clone().unwrap_or_default()));
        } else if !cs.unsafe_reasons.is_empty() {
            draw_blockers.push(format!("Plotter not ready: {}", cs.unsafe_reasons.join("; ")));
        }
        if frame_json["active"].is_null() {
            draw_blockers.push("The frame is not calibrated yet. Open Calibration and set it up once.".into());
        } else if !frame_ready {
            draw_blockers.push("The frame calibration has not been verified with the test pattern yet.".into());
        }
        if busy || phase == "paused" {
            draw_blockers.push("A drawing is in progress.".into());
        }
        let can_draw = draw_blockers.is_empty() && phase != "error";
        json!({
            "phase": phase, "message": message, "progress": progress, "note": ack_note,
            "error": failure.map(|f| json!({ "stage": f.stage, "message": f.message, "detail": diag::redact(&f.detail), "atMs": f.at_ms })),
            "controller": { "reachable": cs.reachable, "error": cs.error, "ready": ctl_ready, "blockers": cs.unsafe_reasons, "pos": pos,
                "box": cs.status.as_ref().and_then(|s| s.bbox), "estop": cs.status.as_ref().map(|s| s.estop).unwrap_or(false), "sock": cs.sock },
            "camera": cam_json,
            "cal": cal_json, "defaultPose": default_pose,
            "paper": { "state": paper_state, "geom": paper_json, "lockId": lock_id },
            "blind": { "raw": serde_json::to_value(self.blind_geom(false)).unwrap_or(Value::Null), "shifted": serde_json::to_value(self.blind_geom(true)).unwrap_or(Value::Null) },
            "services": services, "setup": setup, "blockers": blockers, "canWrite": can_write, "edgeMm": edge,
            "frame": frame_json, "job": job_json, "canDraw": can_draw, "drawBlockers": draw_blockers,
            "settings": settings, "web": self.web_url.lock().unwrap().clone(),
            "surfaceZ": self.surface_z(&prof), "version": env!("CARGO_PKG_VERSION"),
        })
    }

    fn surface_z(&self, prof: &crate::ctl::ProfileInfo) -> Value {
        let s = self.lock().settings.get("surfaceZ").and_then(|v| v.as_f64());
        json!({ "value": s.or(prof.surface_prior_z), "fromSetting": s.is_some() })
    }

    fn sheet_for(&self, g: &PaperGeom) -> Result<Sheet, String> {
        let prof = crate::ctl::read_profiles(&self.ctl.root());
        let z0 = self.lock().settings.get("surfaceZ").and_then(|v| v.as_f64()).or(prof.surface_prior_z).ok_or("The paper surface height is not set (Setup → Pen height).")?;
        Ok(Sheet {
            id: "camera".into(),
            label: format!("camera-detected paper {:.0}x{:.0} mm", g.width_mm, g.height_mm),
            a: g.page_to_pen,
            plane: Plane { x0: 150.0, y0: 150.0, z0, gx: 0.0, gy: 0.0 },
            page: [g.width_mm, g.height_mm],
            provisional: true,
            warnings: vec![format!("pen-down height comes from the surface setting ({z0:.1} mm), not a measured plane")],
        })
    }

    fn fail(&self, stage: &str, message: &str, detail: impl Into<String>) -> String {
        let detail = detail.into();
        diag::error(stage, format!("{message} :: {detail}"));
        let mut g = self.lock();
        g.preparing = false;
        g.failure = Some(Failure { stage: stage.into(), message: message.into(), detail: detail.clone(), at_ms: diag::now_ms() });
        format!("{message} ({detail})")
    }

    /// Clear a finished/failed result so the screen goes back to Ready.
    pub fn dismiss(&self) {
        let mut g = self.lock();
        g.failure = None;
        g.finished_note = None;
        g.own_run = None;
        drop(g);
        if self.current_run().map(|r| !matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
            self.ctl.discard_run();
        }
    }

    // ── Write ──
    /// Validate, build and start a job from strokes in paper space. The click on Write is the confirmation.
    /// Runs the slow parts (controller job check) on its own thread; poll `state` for progress.
    pub fn write(self: &Arc<Self>, strokes: Vec<Vec<[f64; 2]>>, key: String, lock_id: u64, name: String) -> Result<(), String> {
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
            if g.tracker.lock_id != lock_id {
                return Err("The paper moved since the preview was made. Check the preview and press Write again.".into());
            }
            g.failure = None;
            g.finished_note = None;
            g.kind = "write";
            g.preparing = true;
        }
        let me = self.clone();
        let spawn = std::thread::Builder::new().name("write-prepare".into()).spawn(move || {
            if let Err(e) = me.write_inner(strokes, key, lock_id, name) {
                me.fail("prepare", "Could not start writing.", e);
            }
        });
        if let Err(e) = spawn {
            self.lock().preparing = false;
            return Err(e.to_string());
        }
        Ok(())
    }

    fn write_inner(&self, strokes: Vec<Vec<[f64; 2]>>, key: String, lock_id: u64, name: String) -> Result<(), String> {
        diag::info("write", format!("write requested: {} strokes", strokes.len()));
        // re-check everything now, not when the button was drawn
        let cs = self.ctl.snapshot();
        let st = self.ctl.fresh_status()?;
        let why = crate::ctl::unsafe_reasons(&st);
        if !why.is_empty() {
            return Err(format!("plotter not ready: {}", why.join("; ")));
        }
        let (geom, cal) = {
            let g = self.lock();
            (g.tracker.locked.clone().ok_or("no stable paper is detected")?, g.cal.clone().ok_or("camera is not calibrated")?)
        };
        {
            // the lock must agree with what the camera sees right now (a lock can lag a moving sheet by a moment)
            let g = self.lock();
            let agrees = g.tracker.live.as_ref().map(|l| paper::quad_shift(&l.corners_pen, &geom.corners_pen) <= paper::RELOCK_SHIFT_MM).unwrap_or(false);
            if !agrees || !g.tracker.stable {
                return Err("the paper is still moving; try again in a moment".into());
            }
        }
        let pos = st.pos.ok_or("plotter position unknown")?;
        if cal.pose_delta([pos[0], pos[1]]) > paper::POSE_TOL_MM {
            return Err("the plotter is not in its camera position".into());
        }
        if cs.run.as_ref().map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
            return Err("a job is already running".into());
        }
        let reach = st.bbox.ok_or("the plotter reports no working area")?;
        let edge = self.setting_f("edgeMm", 6.0);
        paper::validate_strokes(&strokes, &geom, edge, reach)?;
        // the controller accepting G-code proves nothing: have the camera confirm the machine really moves
        self.motion_check().map_err(|e| format!("movement not confirmed: {e}"))?;
        let geom = self.wait_relock(&geom, lock_id)?;
        let steady = self.lock().tracker.stable;
        if !steady {
            return Err("the paper is not steady after the movement check".into());
        }
        paper::validate_strokes(&strokes, &geom, edge, reach)?;
        self.arm_ink_check();
        let sheet = self.sheet_for(&geom)?;
        let depth = self.setting_f("depth", crate::ctl::read_profiles(&self.ctl.root()).write_depth_mm.unwrap_or(0.5));
        let feed = self.setting_f("feed", 1000.0);
        let view = self.ctl.prepare_sheet(name, &sheet, &strokes, geom.width_mm, geom.height_mm, depth, feed)?;
        if !view.accepted {
            return Err(format!("the plotter's safety check rejected the job: {}", view.reason.unwrap_or_default()));
        }
        diag::info("write", format!("job {} checked: {} strokes, {:.0} mm, pen bbox {:?}", view.id, view.strokes, view.draw_len_mm, view.bbox_pen));
        self.ctl.confirm_sheet("camera", true)?; // pressing Write is the explicit confirmation
        self.ctl.start()?;
        let mut g = self.lock();
        g.preparing = false;
        g.own_run = Some(view.id.clone());
        g.last_key = Some((key, Instant::now()));
        diag::info("write", format!("job {} started", view.id));
        Ok(())
    }

    /// Fixed paper placement used when the camera is not involved (settings blindX/blindY/blindW/blindH, pen mm, page BL corner).
    /// `shifted` moves it left by `blindLeftMm`.
    fn blind_geom(&self, shifted: bool) -> PaperGeom {
        let (w, h) = (self.setting_f("blindW", 160.0).clamp(40.0, 400.0), self.setting_f("blindH", 160.0).clamp(40.0, 400.0));
        let x0 = self.setting_f("blindX", 40.0) - if shifted { self.setting_f("blindLeftMm", 25.0) } else { 0.0 };
        let y0 = self.setting_f("blindY", 95.0);
        PaperGeom { corners_img: [[0.0; 2]; 4], corners_pen: [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]], width_mm: w, height_mm: h, page_to_pen: [[1.0, 0.0, x0], [0.0, 1.0, y0]] }
    }

    /// Write without the camera: fixed paper placement, no paper detection, no motion/ink checks. The plotter's own job check still runs.
    pub fn write_blind(self: &Arc<Self>, strokes: Vec<Vec<[f64; 2]>>, key: String, name: String, shifted: bool) -> Result<(), String> {
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
            g.kind = "write";
            g.preparing = true;
        }
        let me = self.clone();
        let spawn = std::thread::Builder::new().name("write-blind".into()).spawn(move || {
            if let Err(e) = me.write_blind_inner(strokes, key, name, shifted) {
                me.fail("prepare", "Could not start writing.", e);
            }
        });
        if let Err(e) = spawn {
            self.lock().preparing = false;
            return Err(e.to_string());
        }
        Ok(())
    }

    fn write_blind_inner(&self, strokes: Vec<Vec<[f64; 2]>>, key: String, name: String, shifted: bool) -> Result<(), String> {
        diag::info("write", format!("blind write requested: {} strokes, shifted={shifted}", strokes.len()));
        let cs = self.ctl.snapshot();
        let st = self.ctl.fresh_status()?;
        let why = crate::ctl::unsafe_reasons(&st);
        if !why.is_empty() {
            return Err(format!("plotter not ready: {}", why.join("; ")));
        }
        if cs.run.as_ref().map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
            return Err("a job is already running".into());
        }
        let geom = self.blind_geom(shifted);
        let reach = st.bbox.ok_or("the plotter reports no working area")?;
        paper::validate_strokes(&strokes, &geom, self.setting_f("edgeMm", 6.0), reach)?;
        let mut sheet = self.sheet_for(&geom)?;
        sheet.label = format!("fixed placement {:.0}x{:.0} mm (no camera)", geom.width_mm, geom.height_mm);
        sheet.id = "blind".into();
        let depth = self.setting_f("depth", crate::ctl::read_profiles(&self.ctl.root()).write_depth_mm.unwrap_or(0.5));
        let feed = self.setting_f("feed", 1000.0);
        let view = self.ctl.prepare_sheet(name, &sheet, &strokes, geom.width_mm, geom.height_mm, depth, feed)?;
        if !view.accepted {
            return Err(format!("the plotter's safety check rejected the job: {}", view.reason.unwrap_or_default()));
        }
        self.ctl.confirm_sheet("blind", true)?;
        self.ctl.start()?;
        let mut g = self.lock();
        g.preparing = false;
        g.own_run = Some(view.id.clone());
        g.last_key = Some((key, Instant::now()));
        diag::info("write", format!("blind job {} started", view.id));
        Ok(())
    }

    pub fn stop(&self) -> Result<(), String> {
        diag::info("write", "stop requested");
        match self.ctl.stop() {
            Ok(()) => Ok(()),
            Err(e) => {
                diag::warn("write", format!("stop: {e}"));
                Err(e)
            }
        }
    }

    pub fn estop(&self) -> Result<String, String> {
        diag::warn("write", "EMERGENCY STOP requested");
        self.ctl.estop()
    }

    // ── camera position ──
    /// Where the head parks so the camera can see the paper: the calibrated pose, else the position the user saved, else the profile default.
    fn view_pose(&self) -> Option<[f64; 2]> {
        let g = self.lock();
        g.cal.as_ref().map(|c| c.pose).or_else(|| g.settings.get("viewPose").and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok()))
    }

    /// Remember the plotter's current X/Y as the camera position (used for the next calibration and every later "look at paper").
    pub fn save_view_pose(&self) -> Result<Value, String> {
        let p = self.ctl.fresh_status()?.pos.ok_or("position unknown")?;
        self.set_setting("viewPose", json!([p[0], p[1]]))?;
        Ok(json!([p[0], p[1]]))
    }

    pub fn goto_camera_pose(&self) -> Result<(), String> {
        let pose = self.view_pose().or_else(|| crate::ctl::read_profiles(&self.ctl.root()).camera_obs_pose).unwrap_or([270.0, 305.0]);
        if let Ok(st) = self.ctl.fresh_status() {
            if let Some(b) = st.bbox {
                if !(pose[0] >= b[0] && pose[0] <= b[1] && pose[1] >= b[2] && pose[1] <= b[3]) {
                    return Err(self.fail("move", "The camera position is outside the plotter's reach.", format!("{pose:?} is outside {b:?}; move the plotter where the camera sees the whole sheet and use 'Use this as camera position'")));
                }
            }
        }
        diag::info("camera", format!("moving to camera position {pose:?}"));
        self.ctl.pen_up().map_err(|e| self.fail("move", "Could not lift the pen.", e))?;
        self.ctl.goto_xy(pose[0], pose[1]).map(|_| ()).map_err(|e| self.fail("move", "Could not move to the camera position.", e))
    }

    // ── camera calibration ──
    /// Mark positions in pen mm, centred in the plotter's working area.
    pub fn cal_marks_plan(&self) -> Result<Vec<[f64; 2]>, String> {
        let b = self.ctl.fresh_status()?.bbox.ok_or("the plotter reports no working area")?;
        let (cx, cy) = ((b[0] + b[1]) / 2.0, (b[2] + b[3]) / 2.0);
        Ok(mark_layout([cx, cy], (((b[1] - b[0]) * 0.28).min(60.0)).max(20.0), (((b[3] - b[2]) * 0.2).min(50.0)).max(20.0)))
    }

    pub fn cal_marks_draw(self: &Arc<Self>) -> Result<Vec<[f64; 2]>, String> {
        let marks = self.cal_marks_plan()?;
        let pos = self.ctl.fresh_status()?.pos.ok_or("plotter position unknown")?;
        {
            let mut g = self.lock();
            if g.preparing {
                return Err("busy".into());
            }
            g.failure = None;
            g.kind = "marks";
            g.preparing = true;
            let _ = pos;
        }
        let me = self.clone();
        let m2 = marks.clone();
        std::thread::spawn(move || {
            let r = (|| -> Result<(), String> {
                let st = me.ctl.fresh_status()?;
                let why = crate::ctl::unsafe_reasons(&st);
                if !why.is_empty() {
                    return Err(format!("plotter not ready: {}", why.join("; ")));
                }
                // identity "sheet": strokes are already in pen mm
                let h = 1000.0;
                let prof = crate::ctl::read_profiles(&me.ctl.root());
                let z0 = me.active_frame().map(|f| f.surface_z).or_else(|| me.lock().settings.get("surfaceZ").and_then(|v| v.as_f64())).or(prof.surface_prior_z).ok_or("surface height is not set")?;
                let sheet = Sheet { id: "marks".into(), label: "calibration marks".into(), a: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], plane: Plane { x0: 150.0, y0: 150.0, z0, gx: 0.0, gy: 0.0 }, page: [h, h], provisional: true, warnings: vec![] };
                let mut strokes = vec![];
                for m in &m2 {
                    strokes.push(vec![[m[0] - 3.0, h - m[1]], [m[0] + 3.0, h - m[1]]]);
                    strokes.push(vec![[m[0], h - (m[1] - 3.0)], [m[0], h - (m[1] + 3.0)]]);
                }
                let reach = st.bbox.ok_or("no working area")?;
                for m in &m2 {
                    if m[0] < reach[0] + 4.0 || m[0] > reach[1] - 4.0 || m[1] < reach[2] + 4.0 || m[1] > reach[3] - 4.0 {
                        return Err("calibration marks would leave the working area".into());
                    }
                }
                let depth = me.setting_f("depth", prof.write_depth_mm.unwrap_or(0.5));
                let view = me.ctl.prepare_sheet("camera calibration marks".into(), &sheet, &strokes, h, h, depth, me.setting_f("feed", 1000.0))?;
                if !view.accepted {
                    return Err(format!("the plotter's safety check rejected the marks: {}", view.reason.unwrap_or_default()));
                }
                me.ctl.confirm_sheet("marks", true)?;
                me.ctl.start()?;
                let mut g = me.lock();
                g.preparing = false;
                g.own_run = Some(view.id);
                Ok(())
            })();
            if let Err(e) = r {
                me.fail("calibration", "Could not draw the calibration marks.", e);
            }
        });
        Ok(marks)
    }

    /// Clicked mark positions (normalised, any order) matched to the planned marks.
    pub fn cal_solve_clicks(&self, clicks: Vec<[f64; 2]>) -> Result<Value, String> {
        let pens = self.cal_marks_plan()?;
        let pose = self.ctl.fresh_status()?.pos.map(|p| [p[0], p[1]]).ok_or("plotter position unknown")?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let cal = CameraCal::solve_unordered(&clicks, &pens, pose, now).map_err(|e| {
            diag::warn("calibration", e.clone());
            e
        })?;
        self.store_cal(cal)
    }

    fn store_cal(&self, cal: CameraCal) -> Result<Value, String> {
        wr(&self.dir.join("camera_cal.json"), &serde_json::to_value(&cal).map_err(|e| e.to_string())?)?;
        diag::info("calibration", format!("camera calibrated: rms {:.2} mm, loo {:?}, pose {:?}", cal.rms_mm, cal.loo_mm, cal.pose));
        let mut g = self.lock();
        let out = json!({ "rmsMm": cal.rms_mm, "looMm": cal.loo_mm });
        g.cal = Some(cal);
        g.tracker.reset();
        Ok(out)
    }

    /// `points`: [{img:[u,v], pen:[x,y]}] — clicked mark positions (normalised) with the pen coordinates they were drawn at.
    pub fn cal_solve(&self, points: Vec<CalPoint>) -> Result<Value, String> {
        let st = self.ctl.fresh_status().ok();
        let pose = st.and_then(|s| s.pos).map(|p| [p[0], p[1]]).ok_or("The plotter must be at its camera position to calibrate.")?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let cal = CameraCal::solve(points, pose, now).map_err(|e| {
            diag::warn("calibration", e.clone());
            e
        })?;
        self.store_cal(cal)
    }

    pub fn cal_clear(&self) {
        let mut g = self.lock();
        g.cal = None;
        g.tracker.reset();
        let _ = fs::remove_file(self.dir.join("camera_cal.json"));
        diag::info("calibration", "camera calibration cleared");
    }

    pub fn start_camera(&self) -> Result<String, String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let root = self.ctl.root();
        let run = root.join("camview/run.sh");
        if !run.exists() {
            return Err(format!("{} not found", run.display()));
        }
        if paper::http_get(&self.camera_url(), "/api/stats", Duration::from_secs(1)).is_ok() {
            return Ok("The camera service is already running.".into());
        }
        fs::create_dir_all(&self.dir).ok();
        let log = fs::OpenOptions::new().create(true).append(true).open(self.dir.join("camera.log")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let roi = self.lock().settings.get("paperRoi").and_then(|v| serde_json::from_value::<[f64; 4]>(v.clone()).ok());
        let mut cmd = Command::new("/bin/sh");
        if let Some(r) = roi {
            cmd.env("CAMVIEW_PAPER_ROI", format!("{},{},{},{}", r[0], r[1], r[2], r[3]));
        }
        cmd.arg(&run).current_dir(root.join("camview")).stdin(Stdio::null()).stdout(log).stderr(log2).process_group(0).spawn().map_err(|e| e.to_string())?;
        diag::info("camera", "camera service started");
        Ok("Camera service starting…".into())
    }

    // ── diagnostics ──
    pub fn diagnostics(&self, client_notes: &str) -> String {
        let st = self.state();
        let cs = self.ctl.snapshot();
        let mut o = String::new();
        o.push_str("# CtoP diagnostics\n");
        o.push_str(&format!("version: {}\nos: {} {}\ntime(ms): {}\n\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH, diag::now_ms()));
        o.push_str(&format!("## Status\nphase: {}\nmessage: {}\n", st["phase"], st["message"]));
        if !st["error"].is_null() {
            o.push_str(&format!("\n## Error\nstage: {}\nmessage: {}\ndetail:\n{}\n", st["error"]["stage"], st["error"]["message"], st["error"]["detail"].as_str().unwrap_or("")));
        }
        o.push_str(&format!("\nblockers: {}\nsetup needed: {}\n", st["blockers"], st["setup"]));
        let sect = |name: &str, v: &Value| format!("\n## {name}\n{}\n", serde_json::to_string(&diag::redact_json(v)).unwrap_or_default());
        o.push_str(&sect("Controller (hardware)", &st["controller"]));
        o.push_str(&sect("Controller raw status", &serde_json::to_value(&cs.status).unwrap_or(Value::Null)));
        o.push_str(&sect("Camera", &st["camera"]));
        o.push_str(&sect("Services and verification", &st["services"]));
        o.push_str(&sect("Camera calibration", &st["cal"]));
        o.push_str(&sect("Paper", &st["paper"]));
        o.push_str(&sect("Job stage", &json!({ "run": cs.run, "prepared": cs.prepared, "lastControllerReply": cs.last_reply, "progress": st["progress"] })));
        o.push_str(&sect("Settings", &st["settings"]));
        if !client_notes.is_empty() {
            o.push_str(&format!("\n## Interface notes\n{}\n", diag::redact(client_notes)));
        }
        o.push_str("\n## Log (newest last)\n");
        for e in diag::recent(200) {
            o.push_str(&format!("{} [{}] {}: {}\n", e.t_ms, e.level, e.src, diag::redact(&e.msg)));
        }
        diag::redact(&o)
    }
}

// ───────────── command dispatch (shared by Tauri and HTTP) ─────────────

fn strokes_arg(a: &Value) -> Result<Vec<Vec<[f64; 2]>>, String> {
    let arr = a.get("strokes").and_then(|s| s.as_array()).ok_or("strokes missing")?;
    if arr.len() > 40000 {
        return Err("too many strokes".into());
    }
    let mut out = Vec::with_capacity(arr.len());
    for s in arr {
        let pts = s.as_array().ok_or("bad stroke")?;
        let mut v = Vec::with_capacity(pts.len());
        for p in pts {
            let (x, y) = (p.get(0).and_then(|x| x.as_f64()), p.get(1).and_then(|x| x.as_f64()));
            match (x, y) {
                (Some(x), Some(y)) => v.push([x, y]),
                _ => return Err("bad point".into()),
            }
        }
        out.push(v);
    }
    Ok(out)
}

pub fn call(w: &Arc<Writer>, cmd: &str, args: Value) -> Result<Value, String> {
    let ok = |_: ()| Ok(Value::Null);
    match cmd {
        "state" => Ok(w.state()),
        "cameras" => Ok(w.cameras()),
        "camera_set_mode" => w.camera_set_mode(args["side"].as_bool().unwrap_or(false), args["mode"].as_str().ok_or("mode missing")?),
        "camera_snapshot" => w.camera_snapshot(args["side"].as_bool().unwrap_or(false), args["label"].as_str().unwrap_or("")).map(Value::String),
        "camera_resume" => w.camera_resume(args["side"].as_bool().unwrap_or(false)).and_then(ok),
        "frame" => w.frame(args["side"].as_bool().unwrap_or(false)),
        "write" => {
            let strokes = strokes_arg(&args)?;
            let key = args["key"].as_str().unwrap_or("").to_string();
            let lock_id = args["lockId"].as_u64().ok_or("lockId missing")?;
            let name = args["name"].as_str().unwrap_or("CtoP text").chars().take(80).collect();
            w.write(strokes, key, lock_id, name).map(|_| Value::Null)
        }
        "write_blind" => {
            let strokes = strokes_arg(&args)?;
            let key = args["key"].as_str().unwrap_or("").to_string();
            let name = args["name"].as_str().unwrap_or("CtoP text").chars().take(80).collect();
            w.write_blind(strokes, key, name, args["shifted"].as_bool().unwrap_or(true)).map(|_| Value::Null)
        }
        "draw" => {
            let strokes = fops::strokes_of(&args)?;
            let key = args["key"].as_str().unwrap_or("").to_string();
            let name = args["name"].as_str().unwrap_or("CtoP drawing").chars().take(80).collect();
            w.draw(strokes, key, name, fops::stages_of(&args)).map(|_| Value::Null)
        }
        "pause" => w.pause().and_then(ok),
        "resume" => w.resume().and_then(ok),
        "frame_list" => Ok(w.frame_list()),
        "frame_save" => w.frame_save(fops::profile_of(&args)?, args["activate"].as_bool().unwrap_or(true)),
        "frame_activate" => w.frame_activate(args["name"].as_str().ok_or("name missing")?).and_then(ok),
        "frame_delete" => w.frame_delete(args["name"].as_str().ok_or("name missing")?).and_then(ok),
        "frame_check" => w.frame_check(&fops::profile_of(&args)?),
        "frame_propose" => w.frame_propose(),
        "frame_adjust" => w.frame_adjust(&fops::profile_of(&args)?, args["inset"].as_f64().ok_or("inset missing")?, fops::pt(&args, "d1")?, fops::pt(&args, "d2")?),
        "frame_test" => w.frame_test(fops::profile_of(&args)?),
        "home_xy" => w.home_xy(args["confirmed"].as_bool().unwrap_or(false)).map(Value::String),
        "probe_begin" => w.probe_begin(&fops::profile_of(&args)?, args["confirmed"].as_bool().unwrap_or(false)),
        "probe_step" => w.probe_step(args["dz"].as_f64().ok_or("dz missing")?),
        "probe_finish" => w.probe_finish(args["record"].as_bool().unwrap_or(false)),
        "stop" => w.stop().and_then(ok),
        "estop" => w.estop().map(Value::String),
        "dismiss" => {
            w.dismiss();
            Ok(Value::Null)
        }
        "get_settings" => Ok(w.settings()),
        "set_setting" => w.set_setting(args["key"].as_str().ok_or("key missing")?, args.get("value").cloned().unwrap_or(Value::Null)).and_then(ok),
        "set_view_pose" => w.save_view_pose(),
        "goto_camera" => w.goto_camera_pose().and_then(ok),
        "cal_marks_plan" => w.cal_marks_plan().map(|m| json!(m)),
        "cal_marks_draw" => w.cal_marks_draw().map(|m| json!(m)),
        "cal_solve" => {
            let pts: Vec<CalPoint> = serde_json::from_value(args.get("points").cloned().unwrap_or(Value::Null)).map_err(|e| format!("bad points: {e}"))?;
            w.cal_solve(pts)
        }
        "cal_solve_clicks" => {
            let clicks: Vec<[f64; 2]> = serde_json::from_value(args.get("clicks").cloned().unwrap_or(Value::Null)).map_err(|e| format!("bad clicks: {e}"))?;
            w.cal_solve_clicks(clicks)
        }
        "cal_clear" => {
            w.cal_clear();
            Ok(Value::Null)
        }
        "verify_motion" => w.verify_motion(),
        "fonts" => Ok(json!(crate::fonts::list())),
        "font_bytes" => crate::fonts::read(args["id"].as_str().ok_or("id missing")?).map(|b| Value::String(b64(&b))),
        "start_camera" => w.start_camera().map(Value::String),
        "diagnostics" => Ok(Value::String(w.diagnostics(args["notes"].as_str().unwrap_or("")))),
        "log_client" => {
            diag::log("warn", "ui", args["msg"].as_str().unwrap_or("").chars().take(2000).collect::<String>());
            Ok(Value::Null)
        }
        "recent_log" => Ok(json!(diag::recent(200).into_iter().map(|e| json!({ "tMs": e.t_ms, "level": e.level, "src": e.src, "msg": diag::redact(&e.msg) })).collect::<Vec<_>>())),
        other => Err(format!("unknown command: {other}")),
    }
}
