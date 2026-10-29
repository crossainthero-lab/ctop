//! Thin Tauri command layer over `AppState`.

use crate::app::*;
use crate::calibration::Calibration;
use crate::ctl::{self, Ctl, CtlSnapshot, PrepareReq, PreparedView, ProfileInfo, SheetInfo};
use crate::gcode::GcodeSettings;
use crate::machine::{ArmMode, Cmd, LogEntry, Snapshot};
use crate::planner::{PlanOptions, Stroke};
use crate::serial::{self, PortInfo};
use crate::sim::{Fault, SimSnapshot, SimTruth};
use std::sync::Arc;
use tauri::State;

type S<'a> = State<'a, Arc<AppState>>;
type R<T> = Result<T, String>;

// ── connection ──
#[tauri::command]
pub fn list_serial_ports() -> Vec<PortInfo> {
    serial::list_ports()
}
#[tauri::command]
pub fn check_port(s: S, path: String) -> PortCheck {
    s.port_check(&path)
}
#[tauri::command]
pub fn connect_simulation(s: S) -> R<()> {
    s.connect_simulation()
}
#[tauri::command]
pub fn connect_hardware(s: S, path: Option<String>) -> R<()> {
    s.connect_hardware(path)
}
#[tauri::command]
pub fn disconnect(s: S) -> R<()> {
    s.disconnect()
}
#[tauri::command]
pub fn machine_state(s: S) -> Snapshot {
    s.snapshot()
}
#[tauri::command]
pub fn recent_log(s: S) -> Vec<LogEntry> {
    s.recent_log()
}

// ── machine control ──
#[tauri::command]
pub fn arm(s: S, mode: ArmMode, acknowledged: bool) -> R<()> {
    s.arm(mode, acknowledged)
}
#[tauri::command]
pub fn disarm(s: S) -> R<()> {
    s.machine()?.request(Cmd::Disarm)
}
#[tauri::command]
pub fn jog(s: S, axis: usize, delta: f64, feed: f64) -> R<()> {
    s.machine()?.request(Cmd::Jog { axis, delta, feed })
}
#[tauri::command]
pub fn goto_position(s: S, x: Option<f64>, y: Option<f64>, z: Option<f64>, feed: f64) -> R<()> {
    s.machine()?.request(Cmd::Goto { x, y, z, feed })
}
#[tauri::command]
pub fn home(s: S, x: bool, y: bool, z: bool) -> R<()> {
    s.machine()?.request(Cmd::Home { axes: [x, y, z] })
}
#[tauri::command]
pub fn read_position(s: S) -> R<()> {
    s.machine()?.request(Cmd::Sync)
}
#[tauri::command]
pub fn verify_position(s: S) -> R<()> {
    s.machine()?.request(Cmd::VerifyPosition)
}
#[tauri::command]
pub fn pause_job(s: S) -> R<()> {
    s.machine()?.request(Cmd::Pause)
}
#[tauri::command]
pub fn resume_job(s: S) -> R<()> {
    s.machine()?.request(Cmd::Resume)
}
#[tauri::command]
pub fn cancel_job(s: S) -> R<()> {
    s.machine()?.request(Cmd::CancelJob)
}
#[tauri::command]
pub fn quick_stop(s: S) -> R<()> {
    s.machine()?.request(Cmd::QuickStop)
}
#[tauri::command]
pub fn emergency_stop(s: S) -> R<()> {
    // Never refuse: if the machine thread is gone there is nothing to halt, say so.
    s.machine()?.request(Cmd::Emergency)
}

// ── calibration & profiles ──
#[tauri::command]
pub fn get_profiles(s: S) -> ProfilesFile {
    s.profiles()
}
#[tauri::command]
pub fn select_profile(s: S, id: String) -> R<()> {
    s.select_profile(&id)
}
#[tauri::command]
pub fn update_profile(s: S, id: String, name: Option<String>, serial_path: Option<String>) -> R<()> {
    s.update_profile(&id, name, serial_path)
}
#[tauri::command]
pub fn set_calibration(s: S, calibration: Calibration) -> R<CalibrationStatus> {
    s.set_calibration(calibration)
}
#[tauri::command]
pub fn calibration_status(s: S) -> CalibrationStatus {
    s.calibration_status()
}

// ── planning & jobs ──
#[tauri::command]
pub fn plan_preview(s: S, strokes: Vec<Stroke>, plan: PlanOptions, settings: GcodeSettings) -> R<PlanPreview> {
    s.plan_preview(&strokes, &plan, &settings)
}
#[tauri::command]
pub fn create_job(s: S, request: JobRequest) -> R<JobSummary> {
    s.create_job(request)
}
#[tauri::command]
pub fn list_jobs(s: S) -> Vec<JobSummary> {
    s.list_jobs()
}
#[tauri::command]
pub fn job_detail(s: S, id: String) -> R<JobDetail> {
    s.job_detail(&id)
}
#[tauri::command]
pub fn delete_job(s: S, id: String) -> R<()> {
    s.delete_job(&id)
}
#[tauri::command]
pub fn job_gcode(s: S, id: String) -> R<String> {
    s.job_gcode(&id)
}
#[tauri::command]
pub fn export_job(s: S, id: String, path: String) -> R<()> {
    s.export_job(&id, &path)
}
#[tauri::command]
pub fn start_job(s: S, id: String, paper_confirmed: bool) -> R<()> {
    s.start_job(&id, paper_confirmed)
}

// ── simulator ──
#[tauri::command]
pub fn sim_state(s: S) -> R<SimSnapshot> {
    s.sim_snapshot()
}
#[tauri::command]
pub fn sim_trail(s: S, from: usize) -> R<(usize, Vec<[f32; 4]>)> {
    Ok(s.sim()?.trail_since(from))
}
#[tauri::command]
pub fn sim_fault(s: S, fault: Fault) -> R<()> {
    s.sim_fault(fault)
}
#[tauri::command]
pub fn sim_speed(s: S, speed: f64) -> R<()> {
    s.set_setting("simSpeed", speed.into())?;
    if let Ok(sim) = s.sim() {
        sim.set_speed(speed);
    }
    Ok(())
}
#[tauri::command]
pub fn sim_clear(s: S) -> R<()> {
    s.sim()?.clear_trail();
    Ok(())
}
#[tauri::command]
pub fn sim_set_truth(s: S, truth: SimTruth) -> R<()> {
    s.sim()?.set_truth(truth);
    Ok(())
}

// ── settings & files ──
#[tauri::command]
pub fn get_settings(s: S) -> serde_json::Value {
    s.settings()
}
#[tauri::command]
pub fn set_setting(s: S, key: String, value: serde_json::Value) -> R<()> {
    s.set_setting(&key, value)
}

#[tauri::command]
pub fn read_text_file(path: String) -> R<String> {
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}
#[tauri::command]
pub fn write_text_file(path: String, content: String) -> R<()> {
    std::fs::write(&path, content).map_err(|e| format!("{path}: {e}"))
}
#[tauri::command]
pub fn read_binary_file(path: String) -> R<tauri::ipc::Response> {
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("{path}: {e}"))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontFile {
    pub family: String,
    pub style: String,
    pub path: String,
}

#[tauri::command]
pub fn list_system_fonts() -> Vec<FontFile> {
    let out = std::process::Command::new("fc-list").args(["--format", "%{family[0]}|%{style[0]}|%{file}\n"]).output();
    let mut v: Vec<FontFile> = Vec::new();
    if let Ok(o) = out {
        for l in String::from_utf8_lossy(&o.stdout).lines() {
            let p: Vec<&str> = l.splitn(3, '|').collect();
            if p.len() == 3 && (p[2].ends_with(".ttf") || p[2].ends_with(".otf")) {
                v.push(FontFile { family: p[0].into(), style: p[1].into(), path: p[2].into() });
            }
        }
    }
    v.sort_by(|a, b| a.family.cmp(&b.family).then(a.style.cmp(&b.style)));
    v.dedup_by(|a, b| a.family == b.family && a.style == b.style);
    v
}

// ── controller (unix socket) backend ──
type C<'a> = State<'a, Arc<Ctl>>;

#[tauri::command]
pub fn ctl_snapshot(c: C) -> CtlSnapshot {
    c.snapshot()
}
#[tauri::command]
pub fn ctl_set_config(c: C, sock: Option<String>, root: Option<String>) {
    c.set_config(sock, root)
}
#[tauri::command(async)]
pub fn ctl_sheets(c: C) -> Vec<SheetInfo> {
    ctl::list_sheets(&c.root())
}
#[tauri::command(async)]
pub fn ctl_profiles(c: C) -> ProfileInfo {
    ctl::read_profiles(&c.root())
}
#[tauri::command]
pub fn ctl_confirm_sheet(c: C, sheet_id: String, confirmed: bool) -> R<()> {
    c.confirm_sheet(&sheet_id, confirmed)
}
#[tauri::command(async)]
pub fn ctl_prepare(c: C, request: PrepareReq) -> R<PreparedView> {
    c.prepare(request)
}
#[tauri::command(async)]
pub fn ctl_start(c: C) -> R<()> {
    c.start()
}
#[tauri::command(async)]
pub fn ctl_pause(c: C) -> R<()> {
    c.pause()
}
#[tauri::command(async)]
pub fn ctl_resume(c: C) -> R<()> {
    c.resume()
}
#[tauri::command(async)]
pub fn ctl_stop(c: C) -> R<()> {
    c.stop()
}
#[tauri::command(async)]
pub fn ctl_estop(c: C) -> R<String> {
    c.estop()
}
#[tauri::command]
pub fn ctl_discard_run(c: C) {
    c.discard_run()
}
#[tauri::command(async)]
pub fn ctl_jog(c: C, dx: f64, dy: f64) -> R<String> {
    c.jog_xy(dx, dy)
}
#[tauri::command(async)]
pub fn ctl_goto(c: C, x: f64, y: f64) -> R<String> {
    c.goto_xy(x, y)
}
#[tauri::command(async)]
pub fn ctl_pen_up(c: C) -> R<String> {
    c.pen_up()
}
#[tauri::command(async)]
pub fn ctl_z_step(c: C, dz: f64, plane_z: Option<f64>) -> R<String> {
    c.z_step(dz, plane_z)
}

// ── companion services ──
#[tauri::command(async)]
pub fn service_up(ports: Vec<u16>) -> Vec<bool> {
    ports.into_iter().map(|p| matches!(p, 7791..=7793 | 7794) && ctl::port_open(p)).collect()
}
#[tauri::command(async)]
pub fn start_worksheets(c: C, s: S) -> R<String> {
    let _ = &s;
    let log = std::env::temp_dir().join("ctop-logs");
    ctl::start_worksheets(&c.root(), &log)
}
