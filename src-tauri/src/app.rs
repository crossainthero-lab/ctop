//! Application state: device profiles, calibration, job store, persistence and
//! connection management. Independent of Tauri so it can be tested.

use crate::calibration::{Calibration, EnvelopeDraft, Issue, Margins, Severity, Verified};
use crate::gcode::{self, GcodeSettings, Program, ProgramLine, ProgramStats};
use crate::machine::{self, ActorConfig, ArmMode, Cmd, Event, EventSink, JobProgress, JobSpec, JobState, LogEntry, MachineHandle, Mode, Snapshot};
use crate::planner::{self, Plan, PlanOptions, Stroke};
use crate::safety::Limits;
use crate::serial;
use crate::sim::{Fault, SimHandle, SimSnapshot, SimTransport, SimTruth};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BAUD: u32 = 115_200;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ───────────── profiles ─────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProfileKind {
    Simulator,
    Hardware,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: ProfileKind,
    pub calibration: Calibration,
    pub serial_path: Option<String>,
    pub sim_truth: Option<SimTruth>,
    pub expected_firmware: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesFile {
    pub active: String,
    pub profiles: Vec<Profile>,
}

/// Calibration for the simulator profile, consistent with `SimTruth::default()`.
pub fn simulator_calibration() -> Calibration {
    Calibration {
        pencil_offset_x: Some(25.0),
        pencil_offset_y: Some(-10.0),
        z_down: Some(12.0),
        z_up: Some(15.0),
        z_safe: Some(30.0),
        paper_origin_x: Some(40.0),
        paper_origin_y: Some(299.0),
        paper_width: Some(210.0),
        paper_height: Some(297.0),
        flip_x: false,
        flip_y: true,
        swap_xy: false,
        z_increases_up: true,
        margins: Margins { left: 10.0, right: 10.0, top: 10.0, bottom: 10.0 },
        envelope: EnvelopeDraft { x_min: Some(5.0), x_max: Some(300.0), y_min: Some(5.0), y_max: Some(309.0), z_min: Some(12.0), z_max: Some(100.0) },
        verified: Verified { offsets: true, z: true, paper: true, envelope: true, orientation: true },
        notes: "SIMULATOR ONLY. These values describe the simulated machine and mean nothing for real hardware.".into(),
    }
}

pub fn default_profiles() -> ProfilesFile {
    ProfilesFile {
        active: "sim".into(),
        profiles: vec![
            Profile {
                id: "sim".into(),
                name: "Simulator".into(),
                kind: ProfileKind::Simulator,
                calibration: simulator_calibration(),
                serial_path: None,
                sim_truth: Some(SimTruth::default()),
                expected_firmware: "Marlin".into(),
            },
            Profile {
                id: "cr10s-pro".into(),
                name: "Creality CR-10S Pro (hardware)".into(),
                kind: ProfileKind::Hardware,
                // Deliberately empty: physical values must be measured, never invented.
                calibration: Calibration::default(),
                serial_path: Some("/dev/serial/by-id/usb-FTDI_FT232R_USB_UART_AM00NIDC-if00-port0".into()),
                sim_truth: None,
                expected_firmware: "Marlin Ver 1.60.9 (Creality3D)".into(),
            },
        ],
    }
}

// ───────────── jobs ─────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Active,
    Completed,
    Failed,
    Cancelled,
    Stopped,
    Interrupted,
}

impl JobStatus {
    fn from_state(s: JobState) -> JobStatus {
        match s {
            JobState::Running | JobState::Paused | JobState::Cancelling => JobStatus::Active,
            JobState::Completed => JobStatus::Completed,
            JobState::Failed => JobStatus::Failed,
            JobState::Cancelled => JobStatus::Cancelled,
            JobState::Stopped => JobStatus::Stopped,
            JobState::Interrupted => JobStatus::Interrupted,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub ended_at: Option<u64>,
    pub status: JobStatus,
    pub stats: ProgramStats,
    pub strokes: Vec<Stroke>,
    pub lines: Vec<ProgramLine>,
    pub settings: GcodeSettings,
    pub plan: PlanOptions,
    pub calibration_key: String,
    pub progress: Option<JobProgress>,
    pub error: Option<String>,
    pub paper: [f64; 2],
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub ended_at: Option<u64>,
    pub status: JobStatus,
    pub stats: ProgramStats,
    pub progress: Option<JobProgress>,
    pub error: Option<String>,
    pub paper: [f64; 2],
}

impl From<&JobRecord> for JobSummary {
    fn from(j: &JobRecord) -> JobSummary {
        JobSummary {
            id: j.id.clone(),
            name: j.name.clone(),
            created_at: j.created_at,
            ended_at: j.ended_at,
            status: j.status,
            stats: j.stats.clone(),
            progress: j.progress.clone(),
            error: j.error.clone(),
            paper: j.paper,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobDetail {
    pub summary: JobSummary,
    pub strokes: Vec<Stroke>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub name: String,
    pub strokes: Vec<Stroke>,
    #[serde(default)]
    pub plan: PlanOptions,
    #[serde(default)]
    pub settings: GcodeSettings,
    pub paper: [f64; 2],
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub plan: Plan,
    pub stats: ProgramStats,
    /// True when Z timing could not use calibrated heights.
    pub z_assumed: bool,
    pub gcode_blockers: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationStatus {
    pub issues: Vec<Issue>,
    pub ready_for_drawing: bool,
    pub reasons: Vec<String>,
    pub envelope_ok: bool,
    pub resolved: Option<crate::calibration::Resolved>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PortCheck {
    pub path: String,
    pub real_path: Option<String>,
    pub available: bool,
    pub holders: Vec<serial::Holder>,
    pub message: String,
}

pub type Emit = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

struct Inner {
    handle: Option<Arc<MachineHandle>>,
    sim: Option<SimHandle>,
    session: u64,
    profiles: ProfilesFile,
    settings: serde_json::Map<String, serde_json::Value>,
    log: VecDeque<LogEntry>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    jobs: Mutex<Vec<JobRecord>>,
    pub calib: Arc<Mutex<Calibration>>,
    dir: PathBuf,
    emit: Emit,
    pub limits: Limits,
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &std::path::Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn write_json<T: Serialize>(p: &std::path::Path, v: &T) -> Result<(), String> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}

impl AppState {
    pub fn new(dir: PathBuf, emit: Emit) -> Arc<AppState> {
        let mut profiles: ProfilesFile = read_json(&dir.join("profiles.json")).unwrap_or_else(default_profiles);
        // never lose the built-in profiles
        for d in default_profiles().profiles {
            if !profiles.profiles.iter().any(|p| p.id == d.id) {
                profiles.profiles.push(d);
            }
        }
        if !profiles.profiles.iter().any(|p| p.id == profiles.active) {
            profiles.active = "sim".into();
        }
        let settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        let active = profiles.profiles.iter().find(|p| p.id == profiles.active).unwrap().calibration.clone();
        let mut jobs: Vec<JobRecord> = read_json(&dir.join("jobs.json")).unwrap_or_default();
        // anything that was "active" when the app closed did not finish under our supervision
        for j in &mut jobs {
            if j.status == JobStatus::Active {
                j.status = JobStatus::Interrupted;
                j.error = Some("CtoP closed while this job was running".into());
            }
        }
        Arc::new(AppState {
            inner: Mutex::new(Inner { handle: None, sim: None, session: 0, profiles, settings, log: VecDeque::new() }),
            jobs: Mutex::new(jobs),
            calib: Arc::new(Mutex::new(active)),
            dir,
            emit,
            limits: Limits::default(),
        })
    }

    // ── persistence ──

    fn save_profiles(&self) -> Result<(), String> {
        let p = self.inner.lock().unwrap().profiles.clone();
        write_json(&self.dir.join("profiles.json"), &p)
    }

    fn save_jobs(&self) {
        let jobs = self.jobs.lock().unwrap().clone();
        let _ = write_json(&self.dir.join("jobs.json"), &jobs);
    }

    // ── settings ──

    pub fn settings(&self) -> serde_json::Value {
        serde_json::Value::Object(self.inner.lock().unwrap().settings.clone())
    }

    pub fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        let snapshot = {
            let mut g = self.inner.lock().unwrap();
            g.settings.insert(key.to_string(), value);
            g.settings.clone()
        };
        write_json(&self.dir.join("settings.json"), &snapshot)
    }

    fn setting_bool(&self, key: &str) -> bool {
        self.inner.lock().unwrap().settings.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    // ── profiles / calibration ──

    pub fn profiles(&self) -> ProfilesFile {
        self.inner.lock().unwrap().profiles.clone()
    }

    pub fn active_profile(&self) -> Profile {
        let g = self.inner.lock().unwrap();
        g.profiles.profiles.iter().find(|p| p.id == g.profiles.active).cloned().unwrap()
    }

    pub fn select_profile(&self, id: &str) -> Result<(), String> {
        if self.is_connected() {
            return Err("Disconnect before switching device profiles".into());
        }
        {
            let mut g = self.inner.lock().unwrap();
            let p = g.profiles.profiles.iter().find(|p| p.id == id).cloned().ok_or("No such profile")?;
            g.profiles.active = id.to_string();
            *self.calib.lock().unwrap() = p.calibration;
        }
        self.save_profiles()
    }

    pub fn set_calibration(&self, cal: Calibration) -> Result<CalibrationStatus, String> {
        if self.job_active() {
            return Err("Calibration cannot be changed while a job is running".into());
        }
        {
            let mut g = self.inner.lock().unwrap();
            let id = g.profiles.active.clone();
            if let Some(p) = g.profiles.profiles.iter_mut().find(|p| p.id == id) {
                p.calibration = cal.clone();
            }
            *self.calib.lock().unwrap() = cal;
        }
        self.save_profiles()?;
        Ok(self.calibration_status())
    }

    pub fn update_profile(&self, id: &str, name: Option<String>, serial_path: Option<String>) -> Result<(), String> {
        {
            let mut g = self.inner.lock().unwrap();
            let p = g.profiles.profiles.iter_mut().find(|p| p.id == id).ok_or("No such profile")?;
            if let Some(n) = name {
                p.name = n;
            }
            if let Some(s) = serial_path {
                p.serial_path = Some(s);
            }
        }
        self.save_profiles()
    }

    pub fn calibration_status(&self) -> CalibrationStatus {
        let cal = self.calib.lock().unwrap().clone();
        let fw = self.snapshot().fw_limits;
        let issues = cal.validate(fw.as_ref());
        let envelope_ok = cal.envelope_issues(fw.as_ref()).is_empty();
        let (ready, reasons, resolved) = match cal.ready_for_drawing(fw.as_ref()) {
            Ok(r) => (true, vec![], Some(r)),
            Err(w) => (false, w, cal.resolve().ok()),
        };
        let _ = Severity::Error;
        CalibrationStatus { issues, ready_for_drawing: ready, reasons, envelope_ok, resolved }
    }

    // ── connection ──

    pub fn snapshot(&self) -> Snapshot {
        let h = self.inner.lock().unwrap().handle.clone();
        match h {
            Some(h) => h.snapshot(),
            None => Snapshot::disconnected(self.inner.lock().unwrap().session),
        }
    }

    pub fn machine(&self) -> Result<Arc<MachineHandle>, String> {
        self.inner.lock().unwrap().handle.clone().ok_or_else(|| "Not connected".to_string())
    }

    pub fn is_connected(&self) -> bool {
        let g = self.inner.lock().unwrap();
        g.handle.as_ref().map_or(false, |h| !h.is_finished())
    }

    fn job_active(&self) -> bool {
        self.jobs.lock().unwrap().iter().any(|j| j.status == JobStatus::Active)
    }

    fn make_sink(self: &Arc<Self>) -> EventSink {
        let me = Arc::downgrade(self);
        Arc::new(move |ev| {
            let Some(app) = me.upgrade() else { return };
            match ev {
                Event::State(s) => {
                    app.on_state(&s);
                    (app.emit)("ctop://machine", serde_json::to_value(&*s).unwrap_or_default());
                }
                Event::Log(batch) => {
                    {
                        let mut g = app.inner.lock().unwrap();
                        for e in &batch {
                            g.log.push_back(e.clone());
                            if g.log.len() > 3000 {
                                g.log.pop_front();
                            }
                        }
                    }
                    (app.emit)("ctop://log", serde_json::to_value(&batch).unwrap_or_default());
                }
            }
        })
    }

    fn on_state(&self, s: &Snapshot) {
        let Some(p) = &s.job else { return };
        let mut changed_final = false;
        {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(j) = jobs.iter_mut().find(|j| j.id == p.id) {
                let status = JobStatus::from_state(p.state);
                if status != JobStatus::Active && j.status == JobStatus::Active {
                    j.ended_at = Some(now_ms());
                    changed_final = true;
                }
                j.status = status;
                j.progress = Some(p.clone());
                j.error = p.error.clone();
            }
        }
        if changed_final {
            self.save_jobs();
        }
    }

    fn begin_session(self: &Arc<Self>, transport: Box<dyn crate::transport::Transport>, mode: Mode, sim: Option<SimHandle>) -> Result<(), String> {
        let (session, old) = {
            let mut g = self.inner.lock().unwrap();
            if g.handle.as_ref().map_or(false, |h| !h.is_finished()) {
                return Err("Already connected — disconnect first".into());
            }
            g.session += 1;
            g.log.clear();
            (g.session, g.handle.take())
        };
        if let Some(o) = old {
            o.join();
        }
        let sink = self.make_sink();
        let h = machine::spawn(transport, mode, session, self.calib.clone(), ActorConfig { limits: self.limits, ..ActorConfig::default() }, sink);
        let mut g = self.inner.lock().unwrap();
        g.handle = Some(Arc::new(h));
        g.sim = sim;
        Ok(())
    }

    pub fn connect_simulation(self: &Arc<Self>) -> Result<(), String> {
        let profile = self.active_profile();
        let truth = profile.sim_truth.clone().unwrap_or_default();
        let speed = self.inner.lock().unwrap().settings.get("simSpeed").and_then(|v| v.as_f64()).unwrap_or(4.0);
        let t = SimTransport::new(truth, speed);
        let sim = t.handle.clone();
        self.begin_session(Box::new(t), Mode::Simulation, Some(sim))
    }

    /// Hardware connection. Gated: the operator must have unlocked hardware mode in Settings.
    pub fn connect_hardware(self: &Arc<Self>, path: Option<String>) -> Result<(), String> {
        // The printer is owned by the controller (daemon4, libusb). Opening a serial port would pulse DTR (board reset =>
        // homed frame lost) and compete with it. Direct serial is therefore disabled unless explicitly forced.
        if std::env::var("CTOP_ALLOW_SERIAL").ok().as_deref() != Some("1") {
            return Err("Direct serial access is disabled: the printer is owned by the controller process. Use the Controller connection (unix socket) instead. (Developer override: CTOP_ALLOW_SERIAL=1.)".into());
        }
        if !self.setting_bool("hardwareUnlocked") {
            return Err("Hardware mode is locked. Unlock it in Settings once the calibration session has released the printer and you have physically checked it.".into());
        }
        let profile = self.active_profile();
        if profile.kind != ProfileKind::Hardware {
            return Err("Select a hardware profile before connecting to a real printer".into());
        }
        let path = path.or(profile.serial_path.clone()).ok_or("No serial device selected")?;
        let port = serial::open_exclusive(&path, BAUD).map_err(|e| e.to_string())?;
        self.begin_session(Box::new(port), Mode::Hardware, None)
    }

    pub fn disconnect(&self) -> Result<(), String> {
        let h = self.inner.lock().unwrap().handle.clone();
        match h {
            None => Ok(()),
            Some(h) => {
                if h.is_finished() {
                    h.join();
                    self.inner.lock().unwrap().handle = None;
                    (self.emit)("ctop://machine", serde_json::to_value(Snapshot::disconnected(self.inner.lock().unwrap().session)).unwrap());
                    return Ok(());
                }
                h.request(Cmd::Disconnect)?;
                h.join();
                Ok(())
            }
        }
    }

    pub fn port_check(&self, path: &str) -> PortCheck {
        match serial::check_available(path) {
            Ok(real) => PortCheck { path: path.into(), real_path: Some(real.to_string_lossy().into()), available: true, holders: vec![], message: "No other process was found holding this device (checked without opening it).".into() },
            Err(serial::SerialError::InUse(h)) => PortCheck { path: path.into(), real_path: None, available: false, message: format!("In use by {} process(es). CtoP will not connect.", h.len()), holders: h },
            Err(e) => PortCheck { path: path.into(), real_path: None, available: false, holders: vec![], message: e.to_string() },
        }
    }

    pub fn recent_log(&self) -> Vec<LogEntry> {
        self.inner.lock().unwrap().log.iter().cloned().collect()
    }

    // ── simulator ──

    pub fn sim(&self) -> Result<SimHandle, String> {
        self.inner.lock().unwrap().sim.clone().ok_or_else(|| "Simulator is not running".to_string())
    }

    pub fn sim_snapshot(&self) -> Result<SimSnapshot, String> {
        Ok(self.sim()?.snapshot())
    }

    pub fn sim_fault(&self, f: Fault) -> Result<(), String> {
        self.sim()?.fault(f);
        Ok(())
    }

    // ── planning and jobs ──

    pub fn plan_preview(&self, strokes: &[Stroke], opts: &PlanOptions, settings: &GcodeSettings) -> Result<PlanPreview, String> {
        gcode::validate_settings(settings, &self.limits).map_err(|e| e.to_string())?;
        let plan = planner::plan(strokes, opts, [0.0, 0.0]);
        let cal = self.calib.lock().unwrap().clone();
        let z_gap = match (cal.z_up, cal.z_down) {
            (Some(u), Some(d)) => Some((u - d).abs()),
            _ => None,
        };
        let stats = estimate_paper_space(&plan, settings, z_gap.unwrap_or(3.0));
        let blockers = match cal.ready_for_drawing(self.snapshot().fw_limits.as_ref()) {
            Ok(_) => vec![],
            Err(w) => w,
        };
        Ok(PlanPreview { plan, stats, z_assumed: z_gap.is_none(), gcode_blockers: blockers })
    }

    /// Plan + generate G-code against the active calibration and queue the job.
    /// Calibration must at least resolve with no errors; "ready to draw" is re-checked at start.
    pub fn create_job(&self, req: JobRequest) -> Result<JobSummary, String> {
        let cal = self.calib.lock().unwrap().clone();
        let fw = self.snapshot().fw_limits;
        let issues: Vec<String> = cal.validate(fw.as_ref()).into_iter().filter(|i| i.severity == Severity::Error).map(|i| format!("{}: {}", i.field, i.message)).collect();
        if !issues.is_empty() {
            return Err(format!("Calibration incomplete or invalid — cannot generate machine G-code. {}", issues.join("; ")));
        }
        let resolved = cal.resolve().map_err(|e| e.into_iter().map(|i| i.message).collect::<Vec<_>>().join("; "))?;
        let plan = planner::plan(&req.strokes, &req.plan, [0.0, 0.0]);
        let program: Program = gcode::generate(&plan.strokes, &resolved, &req.settings, &self.limits).map_err(|e| e.to_string())?;
        let id = format!("job-{}", now_ms());
        let rec = JobRecord {
            id,
            name: if req.name.trim().is_empty() { "Untitled".into() } else { req.name.clone() },
            created_at: now_ms(),
            ended_at: None,
            status: JobStatus::Queued,
            stats: program.stats.clone(),
            strokes: plan.strokes,
            lines: program.lines,
            settings: req.settings,
            plan: req.plan,
            calibration_key: serde_json::to_string(&cal).unwrap_or_default(),
            progress: None,
            error: None,
            paper: req.paper,
        };
        let summary = JobSummary::from(&rec);
        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.push(rec);
            // keep history bounded: drop the oldest finished jobs beyond 60
            while jobs.len() > 60 {
                if let Some(i) = jobs.iter().position(|j| j.status != JobStatus::Queued && j.status != JobStatus::Active) {
                    jobs.remove(i);
                } else {
                    break;
                }
            }
        }
        self.save_jobs();
        Ok(summary)
    }

    pub fn list_jobs(&self) -> Vec<JobSummary> {
        self.jobs.lock().unwrap().iter().map(JobSummary::from).collect()
    }

    pub fn job_detail(&self, id: &str) -> Result<JobDetail, String> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs.iter().find(|j| j.id == id).ok_or("No such job")?;
        Ok(JobDetail { summary: JobSummary::from(j), strokes: j.strokes.clone() })
    }

    pub fn delete_job(&self, id: &str) -> Result<(), String> {
        {
            let mut jobs = self.jobs.lock().unwrap();
            let i = jobs.iter().position(|j| j.id == id).ok_or("No such job")?;
            if jobs[i].status == JobStatus::Active {
                return Err("Job is active — cancel it first".into());
            }
            jobs.remove(i);
        }
        self.save_jobs();
        Ok(())
    }

    pub fn job_gcode(&self, id: &str) -> Result<String, String> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs.iter().find(|j| j.id == id).ok_or("No such job")?;
        let prog = Program { lines: j.lines.clone(), stats: j.stats.clone() };
        let header = format!(
            "CtoP job '{}' ({})\nGenerated for Marlin, units mm, absolute positioning.\nNO heaters/extruder. Estimated time (ESTIMATE ONLY): {:.0} s\nDraw {:.1} mm, travel {:.1} mm, {} strokes.\nCalibration key hash-free; regenerate in CtoP if calibration changes.\nWARNING: coordinates are for THIS machine's calibration; running elsewhere may collide.",
            j.name, j.id, j.stats.estimated_seconds, j.stats.draw_length_mm, j.stats.travel_length_mm, j.stats.strokes
        );
        Ok(prog.to_text(&header))
    }

    pub fn export_job(&self, id: &str, path: &str) -> Result<(), String> {
        let text = self.job_gcode(id)?;
        fs::write(path, text).map_err(|e| e.to_string())
    }

    pub fn start_job(&self, id: &str, paper_confirmed: bool) -> Result<(), String> {
        let h = self.machine()?;
        let (spec, ) = {
            let jobs = self.jobs.lock().unwrap();
            let j = jobs.iter().find(|j| j.id == id).ok_or("No such job")?;
            if j.status != JobStatus::Queued {
                return Err("Only queued jobs can be started".into());
            }
            (JobSpec { id: j.id.clone(), name: j.name.clone(), lines: j.lines.clone(), estimate_s: j.stats.estimated_seconds, calibration_key: j.calibration_key.clone() },)
        };
        h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed })?;
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
            j.status = JobStatus::Active;
        }
        Ok(())
    }

    pub fn arm(&self, mode: ArmMode, acknowledged: bool) -> Result<(), String> {
        self.machine()?.request(Cmd::Arm { mode, acknowledged })
    }
}

/// Time/length estimate in paper space (no machine needed).
pub fn estimate_paper_space(plan: &Plan, s: &GcodeSettings, z_gap: f64) -> ProgramStats {
    let (vd, vt, vz, a) = (s.draw_feed / 60.0, s.travel_feed / 60.0, s.z_feed / 60.0, s.accel_mm_s2);
    let mv = |d: f64, v: f64| {
        if d <= 0.0 {
            0.0
        } else if d >= v * v / a {
            d / v + v / a
        } else {
            2.0 * (d / a).sqrt()
        }
    };
    let mut t = 0.0;
    let mut cur = [0.0, 0.0];
    for st in &plan.strokes {
        let d = ((st[0][0] - cur[0]).powi(2) + (st[0][1] - cur[1]).powi(2)).sqrt();
        t += mv(d, vt) + 2.0 * mv(z_gap, vz) + planner::stroke_length(st) / vd + vd / a + s.pen_settle_ms as f64 / 1000.0;
        cur = *st.last().unwrap();
    }
    ProgramStats {
        strokes: plan.strokes.len(),
        lines: 0,
        draw_length_mm: plan.draw_length_mm,
        travel_length_mm: plan.travel_length_mm,
        pen_lifts: plan.pen_lifts,
        estimated_seconds: t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<AppState> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("ctop-test-{}-{}-{}", std::process::id(), now_ms(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        AppState::new(dir, Arc::new(|_, _| {}))
    }

    fn square(x: f64, y: f64, s: f64) -> Stroke {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]]
    }

    #[test]
    fn hardware_profile_starts_with_no_invented_values() {
        let s = state();
        s.select_profile("cr10s-pro").unwrap();
        let st = s.calibration_status();
        assert!(!st.ready_for_drawing);
        assert!(st.resolved.is_none());
        let c = s.calib.lock().unwrap().clone();
        assert!(c.z_down.is_none() && c.pencil_offset_x.is_none() && c.envelope().is_none());
    }

    #[test]
    fn job_creation_is_refused_with_incomplete_calibration() {
        let s = state();
        s.select_profile("cr10s-pro").unwrap();
        let r = s.create_job(JobRequest { name: "x".into(), strokes: vec![square(20.0, 20.0, 10.0)], plan: Default::default(), settings: Default::default(), paper: [210.0, 297.0] });
        assert!(r.unwrap_err().contains("Calibration incomplete"));
    }

    #[test]
    fn hardware_connect_is_locked_by_default() {
        let s = state();
        s.select_profile("cr10s-pro").unwrap();
        let e = s.connect_hardware(None).unwrap_err();
        // direct serial is disabled outright (printer owned by the controller); the unlock gate is behind CTOP_ALLOW_SERIAL
        assert!(e.contains("Direct serial access is disabled") || e.contains("locked"), "{e}");
    }

    #[test]
    fn simulator_profile_generates_valid_job_and_exports_gcode() {
        let s = state();
        s.set_calibration(simulator_calibration()).unwrap();
        let j = s.create_job(JobRequest { name: "sq".into(), strokes: vec![square(20.0, 20.0, 30.0)], plan: Default::default(), settings: Default::default(), paper: [210.0, 297.0] }).unwrap();
        assert_eq!(j.status, JobStatus::Queued);
        let g = s.job_gcode(&j.id).unwrap();
        assert!(g.contains("G21") && g.contains("M400"));
        assert!(!g.contains("M104") && !g.contains("M109") && !g.contains("M140"));
        let p = std::env::temp_dir().join(format!("ctop-export-{}.gcode", std::process::id()));
        s.export_job(&j.id, p.to_str().unwrap()).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().starts_with("; CtoP job"));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn full_stack_sim_draw_through_app_layer() {
        let s = state();
        s.set_setting("simSpeed", 800.into()).unwrap();
        s.connect_simulation().unwrap();
        let wait = |f: &dyn Fn(&Snapshot) -> bool, what: &str| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let sn = s.snapshot();
                if f(&sn) {
                    return sn;
                }
                assert!(std::time::Instant::now() < end, "timeout: {what}: {sn:?}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        wait(&|x| x.link == crate::machine::Link::Ready, "ready");
        s.arm(ArmMode::Envelope, false).unwrap();
        let m = s.machine().unwrap();
        m.request(Cmd::Sync).unwrap();
        wait(&|x| x.position.is_some() && x.busy == crate::machine::Busy::Idle, "pos");
        m.request(Cmd::VerifyPosition).unwrap();
        let j = s.create_job(JobRequest { name: "sq".into(), strokes: vec![square(20.0, 20.0, 30.0), square(60.0, 60.0, 20.0)], plan: Default::default(), settings: Default::default(), paper: [210.0, 297.0] }).unwrap();
        s.start_job(&j.id, true).unwrap();
        wait(&|x| x.job.as_ref().map_or(false, |p| p.state == JobState::Completed), "done");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let list = s.list_jobs();
        assert_eq!(list[0].status, JobStatus::Completed);
        assert_eq!(list[0].progress.as_ref().unwrap().confirmed_strokes, list[0].progress.as_ref().unwrap().total_strokes);
        assert!(s.sim_snapshot().unwrap().collisions.is_empty());
        s.disconnect().unwrap();
    }

    #[test]
    fn estimate_matches_generator_within_reason() {
        let s = state();
        let strokes = vec![square(20.0, 20.0, 40.0), square(100.0, 100.0, 40.0)];
        let prev = s.plan_preview(&strokes, &PlanOptions::default(), &GcodeSettings::default()).unwrap();
        let cal = simulator_calibration().resolve().unwrap();
        let prog = gcode::generate(&prev.plan.strokes, &cal, &GcodeSettings::default(), &Limits::default()).unwrap();
        let (a, b) = (prev.stats.estimated_seconds, prog.stats.estimated_seconds);
        assert!((a - b).abs() / b < 0.35, "paper-space estimate {a} vs program estimate {b}");
    }
}
