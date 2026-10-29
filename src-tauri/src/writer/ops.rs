//! Automatic operation: service supervision, return to the camera position, physical-motion verification and
//! completion verification. Child module of `writer` (it uses the writer's private state).

use super::*;

const POLL_SLOW: Duration = Duration::from_secs(3);
/// How far the bed is moved for the motion check (mm). Big enough for the camera to see (~1 px/mm), small enough to be harmless.
const MOTION_STEP_MM: f64 = 8.0;
/// A passed motion check is trusted for this long.
const MOTION_TRUST: Duration = Duration::from_secs(10 * 60);

fn backoff(tries: u32, base: u64) -> Duration {
    Duration::from_secs((base << tries.min(4)).min(300))
}

/// PIDs whose command line contains `needle` (our own process excluded).
fn find_procs(needle: &str) -> Vec<u32> {
    let me = std::process::id();
    let mut v = vec![];
    if let Ok(rd) = fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            if pid == me {
                continue;
            }
            if let Ok(c) = fs::read(e.path().join("cmdline")) {
                if String::from_utf8_lossy(&c).replace('\0', " ").contains(needle) {
                    v.push(pid);
                }
            }
        }
    }
    v
}

/// Mean of the paper's four corners in pen mm, from the live (not locked) camera geometry.
fn paper_centre(g: &Inner) -> Option<[f64; 2]> {
    let l = g.tracker.live.as_ref()?;
    let c = &l.corners_pen;
    Some([c.iter().map(|p| p[0]).sum::<f64>() / 4.0, c.iter().map(|p| p[1]).sum::<f64>() / 4.0])
}

impl Writer {
    fn setting_on(&self, k: &str) -> bool {
        self.setting_or(k, true)
    }

    /// Settings that move hardware or start camera programs are opt-in: the camera is optional equipment.
    fn setting_or(&self, k: &str, default: bool) -> bool {
        self.lock().settings.get(k).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// Called every 500 ms from the camera thread. Cheap; the slow work runs every few seconds.
    pub(super) fn supervise(self: &Arc<Self>) {
        {
            let mut g = self.lock();
            if g.svc.last_slow.map(|t| t.elapsed() < POLL_SLOW).unwrap_or(false) {
                return;
            }
            g.svc.last_slow = Some(Instant::now());
        }
        self.supervise_camera();
        self.supervise_controller();
        self.auto_pose();
        self.post_job();
    }

    fn supervise_camera(&self) {
        let url = self.camera_url();
        // what the hub says about the webcam itself (the hub can be up while the camera is busy/unplugged)
        let stats = paper::http_get(&url, "/api/stats", Duration::from_secs(2)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let auto = self.setting_or("cameraAuto", false);
        let reachable = self.lock().cam.reachable;
        let mut start = false;
        {
            let mut g = self.lock();
            match &stats {
                Some(s) => {
                    let age = s["camera"]["frame_age_s"].as_f64();
                    g.svc.cam_streaming = age.map(|a| a < 3.0).unwrap_or(false);
                    g.svc.cam_hub_state = s["camera"]["state"].as_str().map(String::from);
                }
                None => {
                    g.svc.cam_streaming = false;
                    g.svc.cam_hub_state = None;
                }
            }
            if g.svc.cam_streaming {
                g.svc.cam_tries = 0;
            }
            if reachable || stats.is_some() {
                g.svc.cam_down_since = None;
            } else {
                let since = *g.svc.cam_down_since.get_or_insert_with(Instant::now);
                let due = g.svc.cam_started.map(|t| t.elapsed() >= backoff(g.svc.cam_tries, 10)).unwrap_or(true);
                start = auto && since.elapsed() >= Duration::from_secs(3) && due;
                if start {
                    g.svc.cam_started = Some(Instant::now());
                    g.svc.cam_tries += 1;
                }
            }
        }
        if start {
            match self.start_camera() {
                Ok(m) => diag::info("supervisor", format!("camera service was down: {m}")),
                Err(e) => diag::warn("supervisor", format!("could not start the camera service: {e}")),
            }
        }
    }

    fn supervise_controller(&self) {
        let cs = self.ctl.snapshot();
        let auto = self.setting_on("autoStart");
        let launch;
        {
            let mut g = self.lock();
            if cs.reachable {
                g.svc.ctl_down_since = None;
                g.svc.ctl_tries = 0;
                g.svc.ctl_note = None;
                return;
            }
            let since = *g.svc.ctl_down_since.get_or_insert_with(Instant::now);
            let due = g.svc.ctl_started.map(|t| t.elapsed() >= backoff(g.svc.ctl_tries, 20)).unwrap_or(true);
            launch = auto && since.elapsed() >= Duration::from_secs(6) && due && g.svc.ctl_tries < 6;
            if launch {
                g.svc.ctl_started = Some(Instant::now());
                g.svc.ctl_tries += 1;
            }
        }
        if launch {
            if let Err(e) = self.start_controller() {
                diag::warn("supervisor", format!("controller not started: {e}"));
                self.lock().svc.ctl_note = Some(e);
            }
        }
    }

    /// Launch the controller daemon (`daemon4.py --usb`) if it is not already running. It adopts the board's position itself and
    /// refuses if the board reports a different one; it never homes and never moves the machine at start.
    pub fn start_controller(&self) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let root = self.ctl.root();
        let script = root.join("daemon4.py");
        if !script.exists() {
            return Err(format!("{} not found", script.display()));
        }
        if !find_procs("daemon4.py").is_empty() {
            return Err("the controller process is running but does not answer on its socket".into());
        }
        let venv = root.join(".venv-usb/bin/python");
        let py = if venv.exists() { venv } else { PathBuf::from("python3") };
        fs::create_dir_all(&self.dir).ok();
        let log = fs::OpenOptions::new().create(true).append(true).open(self.dir.join("controller.log")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        Command::new(py)
            .args(["-I", "daemon4.py", "--usb", "--sock"])
            .arg(self.ctl.sock())
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("could not start the controller: {e}"))?;
        diag::info("supervisor", "controller was not reachable: started daemon4.py --usb");
        Ok(())
    }

    /// When idle and ready, keep the head parked at the camera position so the paper is always in view
    /// (pen is raised first; no writing happens). Also what brings the head back after a job.
    fn auto_pose(self: &Arc<Self>) {
        if !self.setting_or("autoMove", false) {
            return;
        }
        let cs = self.ctl.snapshot();
        let Some(st) = cs.status.as_ref() else { return };
        if !cs.reachable || !cs.unsafe_reasons.is_empty() || cs.run.as_ref().map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
            return;
        }
        let pos = match st.pos {
            Some(p) => p,
            None => return,
        };
        let go = {
            let mut g = self.lock();
            let Some(cal) = g.cal.as_ref() else { return };
            if g.preparing || g.failure.is_some() || cal.pose_delta([pos[0], pos[1]]) <= paper::POSE_TOL_MM {
                return;
            }
            if g.svc.pose_try.map(|t| t.elapsed() < Duration::from_secs(30)).unwrap_or(false) {
                return;
            }
            g.svc.pose_try = Some(Instant::now());
            true
        };
        if go {
            let me = self.clone();
            std::thread::spawn(move || {
                let pose = me.lock().cal.as_ref().map(|c| c.pose);
                let Some(pose) = pose else { return };
                diag::info("camera", format!("parking at the camera position {pose:?}"));
                let r = me.ctl.pen_up().and_then(|_| me.ctl.goto_xy(pose[0], pose[1]));
                if let Err(e) = r {
                    diag::warn("camera", format!("could not park at the camera position: {e}"));
                }
            });
        }
    }

    /// After a job completes: park, look at the paper and report whether the camera saw ink.
    fn post_job(self: &Arc<Self>) {
        let run = match self.ctl.snapshot().run {
            Some(r) if r.state == RunState::Completed => r,
            _ => return,
        };
        {
            let mut g = self.lock();
            if g.own_run.as_deref() != Some(run.id.as_str()) || g.svc.post_done.as_deref() == Some(run.id.as_str()) || g.kind != "write" {
                return;
            }
            g.svc.post_done = Some(run.id.clone());
        }
        let me = self.clone();
        std::thread::spawn(move || {
            me.ctl.tick();
            if me.setting_or("autoMove", false) {
                if let Some(pose) = me.lock().cal.as_ref().map(|c| c.pose) {
                    let _ = me.ctl.pen_up().and_then(|_| me.ctl.goto_xy(pose[0], pose[1]));
                }
            }
            std::thread::sleep(Duration::from_secs(5));
            let analysis = paper::http_get(&me.camera_url(), "/api/analysis", Duration::from_secs(3)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let items = analysis.as_ref().and_then(|a| a["marks"]["items"].as_array()).map(|a| a.len());
            let ink = match (analysis.as_ref().map(|a| a["marks"]["state"].as_str().unwrap_or("").to_string()), items) {
                (Some(state), Some(n)) if !state.starts_with("paused") && !state.contains("no reference") => json!({ "runId": run.id, "seen": n > 0, "marks": n, "state": state }),
                _ => json!({ "runId": run.id, "seen": null }),
            };
            diag::info("verify", format!("completion check: {ink}"));
            let mut g = me.lock();
            g.finished_note = match ink["seen"].as_bool() {
                Some(true) => Some("The camera sees the new marks on the paper.".into()),
                Some(false) => Some("The plotter reported success, but the camera does not see new marks. Check that the pen touches the paper.".into()),
                None => None,
            };
            g.svc.ink = ink;
        });
    }

    // ── physical motion verification ──

    /// Jog the bed a few mm and watch the paper move in the camera. A controller that acknowledges G-code does NOT prove the
    /// machine moved (no motor power, disabled steppers, slipped belt): the camera is the only independent witness we have.
    /// Returns Ok(report) when motion was seen, Err(message) when commands were acknowledged but nothing moved.
    pub(super) fn motion_check(&self) -> Result<Value, String> {
        if !self.setting_on("motionCheck") {
            return Ok(json!({ "skipped": "disabled" }));
        }
        if self.lock().svc.motion_ok.map(|t| t.elapsed() < MOTION_TRUST).unwrap_or(false) {
            return Ok(json!({ "skipped": "recently verified" }));
        }
        let st = self.ctl.fresh_status()?;
        let pos = st.pos.ok_or("position unknown")?;
        let b = st.bbox.ok_or("no working area")?;
        let mut tried = vec![];
        for (axis, lo, hi, p) in [("Y", b[2], b[3], pos[1]), ("X", b[0], b[1], pos[0])] {
            let dir = if p + MOTION_STEP_MM <= hi - 1.0 { 1.0 } else if p - MOTION_STEP_MM >= lo + 1.0 { -1.0 } else { continue };
            let before = self.settled_centre()?;
            let d = dir * MOTION_STEP_MM;
            let (dx, dy) = if axis == "X" { (d, 0.0) } else { (0.0, d) };
            self.ctl.jog_xy(dx, dy).map_err(|e| format!("movement check could not move {axis}: {e}"))?;
            std::thread::sleep(Duration::from_millis(1500));
            let polls = self.lock().cam.polls;
            self.wait_polls(polls + 3, Duration::from_secs(4));
            let after = self.lock().pipe(paper_centre);
            self.ctl.jog_xy(-dx, -dy).map_err(|e| format!("movement check could not move {axis} back: {e}"))?;
            let shift = after.map(|a| ((a[0] - before[0]).powi(2) + (a[1] - before[1]).powi(2)).sqrt());
            diag::info("verify", format!("movement check axis {axis}: commanded {MOTION_STEP_MM} mm, camera saw {shift:?} mm"));
            tried.push(json!({ "axis": axis, "commandedMm": MOTION_STEP_MM, "seenMm": shift }));
            if shift.map(|s| s >= MOTION_STEP_MM * 0.5).unwrap_or(false) {
                self.lock().svc.motion_ok = Some(Instant::now());
                let rep = json!({ "ok": true, "axis": axis, "seenMm": shift });
                self.lock().svc.motion = rep.clone();
                return Ok(rep);
            }
        }
        self.lock().svc.motion = json!({ "ok": false, "tried": tried });
        Err(format!(
            "The controller accepted the movement commands, but the camera saw nothing move ({}). Check that the printer's main power switch is on and the motors are powered (the USB cable alone does not power them).",
            tried.iter().map(|t| format!("{} axis: {} mm seen", t["axis"].as_str().unwrap_or("?"), t["seenMm"].as_f64().map(|s| format!("{s:.1}")).unwrap_or("none".into()))).collect::<Vec<_>>().join(", ")
        ))
    }

    /// On-demand movement check (Technical view). Moves the bed a few mm and back, pen up.
    pub fn verify_motion(&self) -> Result<Value, String> {
        {
            let mut g = self.lock();
            if g.preparing || self.ctl.snapshot().run.map(|r| matches!(r.state, RunState::Running | RunState::Pausing | RunState::Stopping)).unwrap_or(false) {
                return Err("A job is running.".into());
            }
            g.svc.motion_ok = None;
            g.preparing = true;
        }
        let before = self.lock().tracker.locked.clone();
        let id = self.lock().tracker.lock_id;
        let r = self.motion_check().and_then(|rep| match &before {
            Some(b) => self.wait_relock(b, id).map(|_| rep),
            None => Ok(rep),
        });
        self.lock().preparing = false;
        r
    }

    fn wait_polls(&self, target: u64, timeout: Duration) {
        let t0 = Instant::now();
        while t0.elapsed() < timeout && self.lock().cam.polls < target {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Paper centre (pen mm) once the live geometry has been steady for a moment.
    fn settled_centre(&self) -> Result<[f64; 2], String> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(8) {
            if let Some(c) = self.lock().pipe(|g| if g.tracker.stable { paper_centre(g) } else { None }) {
                return Ok(c);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err("the paper is not steady in the camera view".into())
    }

    /// After the movement check the paper geometry is re-locked by the tracker: wait for it and check it is where it was.
    pub(super) fn wait_relock(&self, old: &PaperGeom, old_id: u64) -> Result<PaperGeom, String> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(12) {
            let cur = self.lock().tracker.locked.clone();
            if let Some(l) = cur {
                if paper::quad_shift(&l.corners_pen, &old.corners_pen) <= 2.0 {
                    // same paper, same place: the preview made before the check is still valid
                    self.lock().tracker.restore(l.clone(), old_id);
                    return Ok(l);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err("the paper was not found in its original place after the movement check".into())
    }

    /// Ask the hub to remember the clean paper so it can report new marks after the job.
    pub(super) fn arm_ink_check(&self) {
        let _ = paper::http_post(&self.camera_url(), "/api/clean_ref", Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2500));
    }
}

impl Inner {
    fn pipe<T>(&self, f: impl FnOnce(&Inner) -> Option<T>) -> Option<T> {
        f(self)
    }
}
