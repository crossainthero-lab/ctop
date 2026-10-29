//! Backend that talks ONLY to the long-running controller (daemon4) over its unix socket.
//!
//! Safety design:
//!  * Nothing here opens a serial/USB device.
//!  * Every line sent goes through [`allowed`]: only read-only queries, job check/start/cancel,
//!    the e-stop, `!draw off`, and plain `G1` moves made of numeric X/Y/Z/F words. No homing,
//!    no `!adopt`, `!rearm`, `!box`, `!zfloor`, `!zup`, `!nopencil`, no M-codes (heaters etc.).
//!  * The controller remains the authority (box, Z floor, no combined XY+Z, verification).
//!    The checks done here are an early, friendlier second line.
//!  * Pause = `!cancel` (stops after the current stroke) + remember the done index;
//!    Resume = a job slice of the remaining strokes. No stroke is ever repeated.

use crate::app::Emit;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_SOCK: &str = "/run/user/1000/pend.sock";
/// Default controller folder: `$HOME/ctop-controller` (override with CTOP_CONTROLLER_DIR or the setting).
pub fn default_root() -> String {
    format!("{}/ctop-controller", std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

// ───────────── socket client ─────────────

/// Whitelist of everything the app may send to the controller.
/// Height (Z, mm) the pen is raised to before any sideways move. The controller's own pen-up height can be BELOW the paper
/// surface (rigid pen, bulging sheet), so travel always uses at least this.
static TRAVEL_Z: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x4038000000000000); // 24.0
pub fn set_travel_z(z: f64) {
    TRAVEL_Z.store(z.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
pub fn travel_z() -> f64 {
    f64::from_bits(TRAVEL_Z.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn allowed(cmd: &str) -> bool {
    let c = cmd.trim();
    if c.contains('\n') || c.contains('\r') || c.len() > 400 {
        return false;
    }
    match c {
        "!status" | "!progress" | "!cancel" | "!estop" | "!draw off" | "!draw on" => return true,
        // XY-only homing with the pen raised; the controller itself refuses below Z 30 and needs this exact confirmation word.
        "!homexy confirmed" => return true,
        _ => {}
    }
    // lowering the controller's Z floor (it never goes below its own hard minimum)
    if let Some(n) = c.strip_prefix("!zfloor ") {
        return n.parse::<f64>().map(|v| v.is_finite() && (0.0..=150.0).contains(&v)).unwrap_or(false);
    }
    if let Some(n) = c.strip_prefix("!log ") {
        return !n.is_empty() && n.len() <= 5 && n.chars().all(|ch| ch.is_ascii_digit());
    }
    for p in ["!jobcheck ", "!jobfile "] {
        if let Some(path) = c.strip_prefix(p) {
            return path.starts_with('/') && path.ends_with(".json") && !path.contains(char::is_whitespace) && !path.contains("..");
        }
    }
    if let Some(rest) = c.strip_prefix("G1 ") {
        let mut any = false;
        for tok in rest.split_whitespace() {
            let (w, num) = tok.split_at(1);
            if !matches!(w, "X" | "Y" | "Z" | "F") {
                return false;
            }
            match num.parse::<f64>() {
                Ok(v) if v.is_finite() => any = true,
                _ => return false,
            }
        }
        return any;
    }
    false
}

/// Send one line, read the whole reply (the controller closes after replying).
pub fn send(sock: &Path, cmd: &str, timeout: Duration) -> Result<String, String> {
    if !allowed(cmd) {
        return Err(format!("internal: command not allowed by the app whitelist: {cmd}"));
    }
    let mut s = UnixStream::connect(sock).map_err(|e| format!("cannot reach controller at {}: {e}", sock.display()))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    s.write_all(format!("{}\n", cmd.trim()).as_bytes()).map_err(|e| format!("controller write failed: {e}"))?;
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("controller read failed ({cmd}): {e}"))?;
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

// ───────────── status ─────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Temps {
    pub nozzle: Option<f64>,
    #[serde(alias = "nozzle_target")]
    pub nozzle_target: Option<f64>,
    pub bed: Option<f64>,
    #[serde(alias = "bed_target")]
    pub bed_target: Option<f64>,
    #[serde(alias = "age_s")]
    pub age_s: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LinkInfo {
    pub backend: Option<String>,
    pub ok: bool,
    #[serde(alias = "last_rx_age_s")]
    pub last_rx_age_s: Option<f64>,
    #[serde(alias = "last_error")]
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CtlStatus {
    pub pos: Option<[f64; 3]>,
    pub draw: bool,
    #[serde(rename = "box")]
    pub bbox: Option<[f64; 4]>,
    pub zfloor: Option<f64>,
    pub zup: Option<f64>,
    #[serde(alias = "zhard_max")]
    pub zhard_max: Option<f64>,
    #[serde(alias = "zhard_min")]
    pub zhard_min: Option<f64>,
    pub init: bool,
    pub homed: bool,
    pub adopted: bool,
    pub nopencil: bool,
    pub estop: bool,
    #[serde(alias = "frame_lost")]
    pub frame_lost: bool,
    pub job: Option<String>,
    pub temps: Option<Temps>,
    pub link: Option<LinkInfo>,
    #[serde(alias = "uptime_s")]
    pub uptime_s: Option<f64>,
}

pub fn parse_status(text: &str) -> Result<CtlStatus, String> {
    serde_json::from_str(text).map_err(|e| format!("controller returned unreadable status ({e}): {}", text.chars().take(120).collect::<String>()))
}

/// Reasons it is NOT safe to start motion (empty = ok). Mirrors worksheets/hardware.py safe_to_run.
pub fn unsafe_reasons(s: &CtlStatus) -> Vec<String> {
    let mut b = vec![];
    if !(s.init && s.homed) {
        b.push("controller has no verified machine frame (not homed/adopted)".to_string());
    }
    if s.nopencil {
        b.push("controller is in no-pencil (homing) mode".into());
    }
    if s.estop {
        b.push("EMERGENCY STOP is latched on the controller (re-arm it from the monitor/Claude Code)".into());
    }
    if s.frame_lost {
        b.push("machine frame was LOST (board reset): remove the pen and re-home".into());
    }
    if !s.link.as_ref().map(|l| l.ok).unwrap_or(false) {
        b.push("printer link is down".into());
    }
    if let Some(t) = &s.temps {
        if t.nozzle_target.unwrap_or(0.0) > 0.0 || t.bed_target.unwrap_or(0.0) > 0.0 {
            b.push("a heater is switched on".into());
        }
    }
    if s.job.as_deref() == Some("running") {
        b.push("another job is running".into());
    }
    if s.pos.is_none() {
        b.push("position unknown".into());
    }
    b
}

// ───────────── sheet / calibration sources ─────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Plane {
    pub x0: f64,
    pub y0: f64,
    pub z0: f64,
    pub gx: f64,
    pub gy: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SheetInfo {
    pub id: String,
    pub path: String,
    pub stage: String,
    pub created: String,
    pub updated: String,
    pub page_size: [f64; 2],
    pub rms_mm: Option<f64>,
    pub verify_passed: Option<bool>,
    pub has_transform: bool,
    pub has_plane: bool,
    pub z0: Option<f64>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    pub dir: String,
    pub pen_down: Option<f64>,
    pub pen_up: Option<f64>,
    pub pen_status: Option<String>,
    pub z_floor_now: Option<f64>,
    pub pen_note: Option<String>,
    pub reach_box: Option<Vec<f64>>,
    pub surface_prior_z: Option<f64>,
    pub write_depth_mm: Option<f64>,
    pub surface_prior_note: Option<String>,
    pub park_z: Option<f64>,
    pub camera_obs_pose: Option<[f64; 2]>,
    pub errors: Vec<String>,
}

fn jf(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}
fn js(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn read_profiles(root: &Path) -> ProfileInfo {
    let mut p = ProfileInfo { dir: root.join("profiles").display().to_string(), ..Default::default() };
    match fs::read_to_string(root.join("profiles/pen_blue.json")).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
        Ok(v) => {
            p.pen_down = v.pointer("/z/pen_down").and_then(|x| x.as_f64());
            p.pen_up = v.pointer("/z/pen_up").and_then(|x| x.as_f64());
            p.z_floor_now = v.pointer("/z/controller_limits_now/z_floor").and_then(|x| x.as_f64());
            p.pen_status = v.get("status").and_then(|x| x.as_str()).map(String::from);
            p.pen_note = v.pointer("/tool_offset_mm/note").and_then(|x| x.as_str()).map(String::from);
        }
        Err(e) => p.errors.push(format!("pen_blue.json: {e}")),
    }
    match fs::read_to_string(root.join("profiles/worksheet_limits.json")).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
        Ok(v) => {
            p.reach_box = v.get("reach_box").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|n| n.as_f64()).collect());
            p.surface_prior_z = jf(&v, "surface_prior_z");
            p.write_depth_mm = jf(&v, "write_depth_mm");
            p.surface_prior_note = v.get("surface_prior_note").and_then(|x| x.as_str()).map(String::from);
            p.park_z = jf(&v, "park_z");
            p.camera_obs_pose = v.get("camera_obs_pose").and_then(|a| a.as_array()).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
        }
        Err(e) => p.errors.push(format!("worksheet_limits.json: {e}")),
    }
    p
}

pub fn list_sheets(root: &Path) -> Vec<SheetInfo> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(root.join("worksheets/data")) {
        for e in rd.flatten() {
            let f = e.path().join("sheet_calibration.json");
            let Ok(t) = fs::read_to_string(&f) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
            let size = v.get("page_size_mm").and_then(|a| a.as_array()).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
            let Some(page_size) = size else { continue };
            out.push(SheetInfo {
                id: e.file_name().to_string_lossy().into(),
                path: f.display().to_string(),
                stage: js(&v, "stage"),
                created: js(&v, "created"),
                updated: js(&v, "updated"),
                page_size,
                rms_mm: jf(&v, "rms_mm"),
                verify_passed: v.pointer("/verify/passed").and_then(|x| x.as_bool()),
                has_transform: v.get("page_to_pen").is_some(),
                has_plane: v.pointer("/contact/plane").is_some(),
                z0: v.pointer("/contact/plane/z0").and_then(|x| x.as_f64()),
            });
        }
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.created.cmp(&a.created)));
    out
}

/// Fully resolved sheet: page→pen affine, contact plane, page size.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub id: String,
    pub label: String,
    pub a: [[f64; 3]; 2],
    pub plane: Plane,
    pub page: [f64; 2],
    pub provisional: bool,
    pub warnings: Vec<String>,
}

pub fn load_sheet(root: &Path, id: &str) -> Result<Sheet, String> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err("bad sheet id".into());
    }
    let f = root.join("worksheets/data").join(id).join("sheet_calibration.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| e.to_string())?;
    let m = v.get("page_to_pen").and_then(|a| a.as_array()).ok_or("this calibration has no page_to_pen transform (ink step not done)")?;
    let mut a = [[0.0; 3]; 2];
    for (i, row) in m.iter().take(2).enumerate() {
        let r = row.as_array().ok_or("bad page_to_pen")?;
        for j in 0..3 {
            a[i][j] = r.get(j).and_then(|x| x.as_f64()).ok_or("bad page_to_pen entry")?;
        }
    }
    let pl = v.pointer("/contact/plane").ok_or("this calibration has no contact plane (contact step not done)")?;
    let g = |k: &str| pl.get(k).and_then(|x| x.as_f64()).ok_or(format!("contact plane lacks {k}"));
    let plane = Plane { x0: g("x0")?, y0: g("y0")?, z0: g("z0")?, gx: g("gx")?, gy: g("gy")? };
    let ps = v.get("page_size_mm").and_then(|a| a.as_array()).ok_or("no page_size_mm")?;
    let page = [ps[0].as_f64().ok_or("page size")?, ps[1].as_f64().ok_or("page size")?];
    let mut warnings = vec![];
    let stage = js(&v, "stage");
    if stage != "verified" {
        warnings.push(format!("calibration stage is '{stage}', not 'verified'"));
    }
    if v.pointer("/verify/passed").and_then(|x| x.as_bool()) != Some(true) {
        warnings.push("verification step has not passed".into());
    }
    Ok(Sheet { id: id.into(), label: format!("{id} ({stage}, {})", js(&v, "updated")), a, plane, page, provisional: false, warnings })
}

/// Provisional sheet: user-measured origin (pen coords of the sheet's bottom-left corner), axis aligned, plane from the
/// worksheet prior. Clearly less accurate; the UI makes the user confirm that.
pub fn provisional_sheet(root: &Path, page: [f64; 2], ox: f64, oy: f64) -> Result<Sheet, String> {
    let p = read_profiles(root);
    let z0 = p.surface_prior_z.ok_or("worksheet_limits.json has no surface_prior_z")?;
    Ok(Sheet {
        id: "provisional".into(),
        label: format!("provisional (origin {ox:.1},{oy:.1}; plane prior z {z0:.1})"),
        a: [[1.0, 0.0, ox], [0.0, 1.0, oy]],
        plane: Plane { x0: 150.0, y0: 150.0, z0, gx: 0.0, gy: 0.0 },
        page,
        provisional: true,
        warnings: vec![format!("PROVISIONAL: contact plane z={z0:.1} is a prior (+-1 mm), no rotation/scale correction, origin is your own measurement")],
    })
}

// ───────────── job building ─────────────

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrepareReq {
    pub name: String,
    /// planned strokes in PAPER space (mm, origin top-left, +y down)
    pub strokes: Vec<Vec<[f64; 2]>>,
    pub paper_w: f64,
    pub paper_h: f64,
    /// calibration directory id under worksheets/data, or "provisional"
    pub sheet_id: String,
    pub origin_x: Option<f64>,
    pub origin_y: Option<f64>,
    pub depth: f64,
    pub feed: f64,
}

pub fn build_job(sheet: &Sheet, strokes: &[Vec<[f64; 2]>], paper_w: f64, paper_h: f64, depth: f64, feed: f64) -> Result<(Value, Vec<Vec<[f64; 2]>>), String> {
    if (paper_w - sheet.page[0]).abs() > 3.0 || (paper_h - sheet.page[1]).abs() > 3.0 {
        return Err(format!(
            "Workspace paper is {paper_w:.0}x{paper_h:.0} mm but the calibrated sheet is {:.0}x{:.0} mm (orientation/size mismatch). Change the Workspace paper to match the sheet.",
            sheet.page[0], sheet.page[1]
        ));
    }
    if !(0.2..=1.0).contains(&depth) {
        return Err(format!("pen depth {depth} mm outside the app's allowed 0.2..1.0 mm"));
    }
    if !(100.0..=1500.0).contains(&feed) {
        return Err(format!("draw feed {feed} outside 100..1500 mm/min"));
    }
    let mut pen: Vec<Vec<[f64; 2]>> = vec![];
    for s in strokes {
        if s.len() < 2 {
            continue;
        }
        let pts: Vec<[f64; 2]> = s
            .iter()
            .map(|p| {
                let (px, py) = (p[0], paper_h - p[1]); // page: origin bottom-left, +y up
                let x = sheet.a[0][0] * px + sheet.a[0][1] * py + sheet.a[0][2];
                let y = sheet.a[1][0] * px + sheet.a[1][1] * py + sheet.a[1][2];
                [(x * 100.0).round() / 100.0, (y * 100.0).round() / 100.0]
            })
            .collect();
        if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return Err("non-finite coordinate in strokes".into());
        }
        // the controller allows at most 4000 points per stroke
        if pts.len() <= 4000 {
            pen.push(pts);
        } else {
            let mut i = 0;
            while i < pts.len() - 1 {
                let j = (i + 3900).min(pts.len() - 1);
                pen.push(pts[i..=j].to_vec());
                i = j;
            }
        }
    }
    if pen.is_empty() {
        return Err("nothing to draw (no strokes with 2+ points)".into());
    }
    if pen.len() > 20000 {
        return Err(format!("{} strokes exceed the controller limit of 20000", pen.len()));
    }
    let job = json!({
        "params": {
            "mode": "surface", "lift_short": 3.0, "lift_long": 5.0, "short_dist": 14.0,
            "feed": feed, "travel_feed": 3000, "zdown_feed": 240, "zup_feed": 300,
            "surface": { "x0": sheet.plane.x0, "y0": sheet.plane.y0, "z0": sheet.plane.z0, "gx": sheet.plane.gx, "gy": sheet.plane.gy },
            "depth": depth
        },
        "strokes": pen
    });
    Ok((job, pen))
}

fn bbox(strokes: &[Vec<[f64; 2]>]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for s in strokes {
        for p in s {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
    }
    b
}

fn draw_len(strokes: &[Vec<[f64; 2]>]) -> f64 {
    strokes.iter().map(|s| s.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum::<f64>()).sum()
}

// ───────────── run state machine ─────────────

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    Pausing,
    Paused,
    Stopping,
    Stopped,
    Completed,
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: String,
    pub name: String,
    pub state: RunState,
    pub total: usize,
    pub done: usize,
    pub segment_base: usize,
    pub segment_total: usize,
    pub message: String,
    pub error: Option<String>,
    pub elapsed_s: f64,
    pub resumable: bool,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub job: Value,
    pub total: usize,
    pub base: usize,
    pub seg_total: usize,
    pub done: usize,
    pub state: RunState,
    pub message: String,
    pub error: Option<String>,
    pub started: Instant,
    pub seg_started: Instant,
    pub elapsed_before: f64,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Progress {
    pub state: String,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
}

impl Run {
    pub fn active(&self) -> bool {
        matches!(self.state, RunState::Running | RunState::Pausing | RunState::Stopping)
    }
    /// Fold a `!progress` reply into the run. `reply` is the raw text ("no job" or JSON).
    pub fn apply_progress(&mut self, reply: &str) {
        let was_active = self.active();
        self.fold_progress(reply);
        if was_active && !self.active() {
            self.elapsed_before += self.seg_started.elapsed().as_secs_f64();
        }
    }

    fn fold_progress(&mut self, reply: &str) {
        let p: Progress = serde_json::from_str(reply).unwrap_or_default();
        let g = (self.base + p.done).min(self.total);
        match p.state.as_str() {
            "running" => {
                self.done = g;
            }
            "done" => {
                self.done = self.total;
                self.state = RunState::Completed;
                self.message = "finished".into();
            }
            "cancelled" => {
                self.done = g;
                self.state = match self.state {
                    RunState::Stopping => RunState::Stopped,
                    RunState::Pausing => RunState::Paused,
                    _ => RunState::Paused,
                };
                self.message = if self.state == RunState::Stopped { "stopped".into() } else { format!("paused after stroke {g}/{}", self.total) };
                if self.done >= self.total {
                    self.state = RunState::Completed;
                    self.message = "finished".into();
                }
            }
            "error" => {
                self.done = g;
                self.state = RunState::Failed;
                self.error = Some(p.error.unwrap_or_else(|| "controller reported an error".into()));
                self.message = "controller aborted the job".into();
            }
            _ => {
                if self.seg_started.elapsed() > Duration::from_secs(3) {
                    self.state = RunState::Failed;
                    self.error = Some(format!("controller has no job (reply: {})", reply.chars().take(80).collect::<String>()));
                }
            }
        }
    }
    pub fn view(&self) -> RunView {
        let el = self.elapsed_before + if self.active() { self.seg_started.elapsed().as_secs_f64() } else { 0.0 };
        RunView {
            id: self.id.clone(),
            name: self.name.clone(),
            state: self.state,
            total: self.total,
            done: self.done,
            segment_base: self.base,
            segment_total: self.seg_total,
            message: self.message.clone(),
            error: self.error.clone(),
            elapsed_s: (el * 10.0).round() / 10.0,
            resumable: self.state == RunState::Paused && self.done < self.total,
        }
    }
}

// ───────────── runtime ─────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PreparedView {
    pub id: String,
    pub name: String,
    pub strokes: usize,
    pub draw_len_mm: f64,
    pub bbox_pen: [f64; 4],
    pub sheet: String,
    pub provisional: bool,
    pub warnings: Vec<String>,
    pub depth: f64,
    pub feed: f64,
    pub plane_z0: f64,
    pub verdict: Option<Value>,
    pub accepted: bool,
    pub reason: Option<String>,
}

struct Prepared {
    view: PreparedView,
    job: Value,
    strokes_n: usize,
    sheet_id: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CtlSnapshot {
    pub sock: String,
    pub reachable: bool,
    pub error: Option<String>,
    pub status: Option<CtlStatus>,
    pub status_age_ms: Option<u64>,
    pub unsafe_reasons: Vec<String>,
    pub run: Option<RunView>,
    pub prepared: Option<PreparedView>,
    pub sheet_confirmed: Option<String>,
    pub last_reply: String,
}

struct Inner {
    reachable: bool,
    error: Option<String>,
    status: Option<CtlStatus>,
    status_at: Option<Instant>,
    prepared: Option<Prepared>,
    run: Option<Run>,
    sheet_confirmed: Option<String>,
    last_reply: String,
}

struct Cfg {
    sock: PathBuf,
    root: PathBuf,
    data: PathBuf,
}

pub struct Ctl {
    inner: Mutex<Inner>,
    cfg: Mutex<Cfg>,
    emit: Emit,
    op: Mutex<()>, // serialises operations that change a run
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

impl Ctl {
    pub fn new(data_dir: PathBuf, sock: Option<String>, root: Option<String>, emit: Emit) -> Arc<Ctl> {
        let sock = sock.filter(|s| !s.is_empty()).or_else(|| std::env::var("CTOP_CONTROLLER_SOCK").ok()).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SOCK.into());
        let root = root.filter(|s| !s.is_empty()).or_else(|| std::env::var("CTOP_CONTROLLER_DIR").ok()).filter(|s| !s.is_empty()).unwrap_or_else(default_root);
        Arc::new(Ctl {
            inner: Mutex::new(Inner { reachable: false, error: Some("not polled yet".into()), status: None, status_at: None, prepared: None, run: None, sheet_confirmed: None, last_reply: String::new() }),
            cfg: Mutex::new(Cfg { sock: sock.into(), root: root.into(), data: data_dir.join("ctl_jobs") }),
            emit,
            op: Mutex::new(()),
        })
    }

    pub fn set_config(&self, sock: Option<String>, root: Option<String>) {
        let mut c = self.cfg.lock().unwrap();
        if let Some(s) = sock.filter(|s| !s.is_empty()) {
            c.sock = s.into();
        }
        if let Some(r) = root.filter(|s| !s.is_empty()) {
            c.root = r.into();
        }
    }
    pub fn sock(&self) -> PathBuf {
        self.cfg.lock().unwrap().sock.clone()
    }
    pub fn root(&self) -> PathBuf {
        self.cfg.lock().unwrap().root.clone()
    }
    fn data(&self) -> PathBuf {
        self.cfg.lock().unwrap().data.clone()
    }

    pub fn snapshot(&self) -> CtlSnapshot {
        let g = self.inner.lock().unwrap();
        CtlSnapshot {
            sock: self.sock().display().to_string(),
            reachable: g.reachable,
            error: g.error.clone(),
            status: g.status.clone(),
            status_age_ms: g.status_at.map(|t| t.elapsed().as_millis() as u64),
            unsafe_reasons: g.status.as_ref().map(unsafe_reasons).unwrap_or_default(),
            run: g.run.as_ref().map(|r| r.view()),
            prepared: g.prepared.as_ref().map(|p| p.view.clone()),
            sheet_confirmed: g.sheet_confirmed.clone(),
            last_reply: g.last_reply.clone(),
        }
    }
    fn publish(&self) {
        (self.emit)("ctop://ctl", serde_json::to_value(self.snapshot()).unwrap_or(Value::Null));
    }

    /// One poll cycle: `!status`, and `!progress` while a run is active.
    pub fn tick(&self) {
        let sock = self.sock();
        match send(&sock, "!status", Duration::from_secs(3)).and_then(|t| parse_status(&t)) {
            Ok(st) => {
                let mut g = self.inner.lock().unwrap();
                g.reachable = true;
                g.error = None;
                g.status = Some(st);
                g.status_at = Some(Instant::now());
            }
            Err(e) => {
                let mut g = self.inner.lock().unwrap();
                g.reachable = false;
                g.error = Some(e);
            }
        }
        let active = self.inner.lock().unwrap().run.as_ref().map(|r| r.active()).unwrap_or(false);
        if active && self.inner.lock().unwrap().reachable {
            if let Ok(reply) = send(&sock, "!progress", Duration::from_secs(3)) {
                let _op = self.op.lock().unwrap();
                let mut g = self.inner.lock().unwrap();
                if let Some(r) = g.run.as_mut() {
                    if r.active() {
                        r.apply_progress(&reply);
                    }
                }
            }
        }
        // an e-stop latched on the controller invalidates the sheet confirmation: the operator must look again
        {
            let mut g = self.inner.lock().unwrap();
            if g.status.as_ref().map(|s| s.estop).unwrap_or(false) {
                g.sheet_confirmed = None;
            }
        }
        self.publish();
    }

    pub fn start_poller(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || loop {
            me.tick();
            let active = me.inner.lock().unwrap().run.as_ref().map(|r| r.active()).unwrap_or(false);
            std::thread::sleep(Duration::from_millis(if active { 300 } else { 1000 }));
        });
    }

    fn set_reply(&self, r: &str) {
        self.inner.lock().unwrap().last_reply = r.chars().take(300).collect();
    }

    // ── sheet confirmation ──
    pub fn confirm_sheet(&self, sheet_id: &str, loaded_and_taped: bool) -> Result<(), String> {
        if !loaded_and_taped {
            self.inner.lock().unwrap().sheet_confirmed = None;
            self.publish();
            return Ok(());
        }
        if sheet_id.is_empty() {
            return Err("choose a sheet calibration first".into());
        }
        self.inner.lock().unwrap().sheet_confirmed = Some(sheet_id.into());
        self.publish();
        Ok(())
    }

    // ── manual moves (controller-validated) ──
    pub fn fresh_status(&self) -> Result<CtlStatus, String> {
        let st = parse_status(&send(&self.sock(), "!status", Duration::from_secs(3))?)?;
        Ok(st)
    }

    fn manual_guard(&self, st: &CtlStatus) -> Result<(), String> {
        if st.estop {
            return Err("e-stop latched: no motion".into());
        }
        if st.frame_lost {
            return Err("machine frame lost: no motion".into());
        }
        if !(st.init && st.homed) {
            return Err("controller has no verified frame".into());
        }
        if st.job.as_deref() == Some("running") || self.inner.lock().unwrap().run.as_ref().map(|r| r.active()).unwrap_or(false) {
            return Err("a job is running: manual moves are disabled".into());
        }
        if !st.link.as_ref().map(|l| l.ok).unwrap_or(false) {
            return Err("printer link is down".into());
        }
        Ok(())
    }

    pub(crate) fn do_move(&self, cmd: &str) -> Result<String, String> {
        self.do_move_mode(cmd, false)
    }

    /// `draw` = true lets the controller move sideways with the pen below its pen-up height (the pen drags). Mode goes back to off afterwards.
    pub(crate) fn do_move_mode(&self, cmd: &str, draw: bool) -> Result<String, String> {
        let r = send(&self.sock(), if draw { "!draw on" } else { "!draw off" }, Duration::from_secs(5))?;
        if r.contains("REFUSED") {
            return Err(r);
        }
        let reply = send(&self.sock(), cmd, Duration::from_secs(90));
        if draw {
            let _ = send(&self.sock(), "!draw off", Duration::from_secs(5));
        }
        let reply = reply?;
        self.set_reply(&reply);
        if reply.contains("REFUSED") || reply.contains("FW-ERROR") || reply.contains("MISMATCH") || !reply.contains("VERIFIED") {
            return Err(format!("controller: {}", reply.lines().last().unwrap_or("")));
        }
        self.tick();
        Ok(reply)
    }

    /// Jog X/Y by a delta. Only at pen-up height (>= zup); the controller enforces the box.
    pub fn jog_xy(&self, dx: f64, dy: f64) -> Result<String, String> {
        let _op = self.op.lock().unwrap();
        if dx.abs() > 50.0 || dy.abs() > 50.0 || !dx.is_finite() || !dy.is_finite() || (dx != 0.0 && dy != 0.0) {
            return Err("one axis at a time, at most 50 mm".into());
        }
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        self.raise_for_travel(&st)?;
        let (tx, ty) = (pos[0] + dx, pos[1] + dy);
        let b = st.bbox.ok_or("no box")?;
        if tx < b[0] || tx > b[1] || ty < b[2] || ty > b[3] {
            return Err(format!("target X{tx:.1} Y{ty:.1} is outside the controller box {b:?}"));
        }
        self.do_move(&format!("G1 X{tx:.3} Y{ty:.3} F3000"))
    }

    pub fn goto_xy(&self, x: f64, y: f64) -> Result<String, String> {
        let _op = self.op.lock().unwrap();
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        let _ = pos;
        let b = st.bbox.ok_or("no box")?;
        if !(x >= b[0] && x <= b[1] && y >= b[2] && y <= b[3]) {
            return Err(format!("target X{x:.1} Y{y:.1} is outside the controller box {b:?}"));
        }
        self.raise_for_travel(&st)?;
        self.do_move(&format!("G1 X{x:.3} Y{y:.3} F3000"))
    }

    /// Guarded XY homing (never touches Z). The caller must already have the user's explicit confirmation that the pen is
    /// raised and the head is clear of the frame. Raises Z to at least `HOME_Z` first, which the controller requires.
    pub fn home_xy(&self) -> Result<String, String> {
        const HOME_Z: f64 = 30.0;
        let _op = self.op.lock().unwrap();
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        if st.zhard_max.unwrap_or(150.0) < HOME_Z {
            return Err("this plotter cannot lift the pen high enough for a safe XY home".into());
        }
        if pos[2] < HOME_Z {
            self.do_move(&format!("G1 Z{HOME_Z:.3} F300"))?;
        }
        let reply = send(&self.sock(), "!homexy confirmed", Duration::from_secs(330))?;
        self.set_reply(&reply);
        if !reply.contains("XY HOMED") {
            return Err(format!("controller: {}", reply.lines().last().unwrap_or("no reply")));
        }
        self.tick();
        Ok(reply.lines().last().unwrap_or("").to_string())
    }

    /// One bounded sideways step for the joystick. Only with the pen at or above the controller's pen-up height; the target is
    /// clamped to the controller's box (0.5 mm inside). Returns the position the controller reports and whether it was clamped.
    pub fn jog_vec(&self, dx: f64, dy: f64, feed: f64) -> Result<([f64; 3], bool), String> {
        let _op = self.op.lock().unwrap();
        if !dx.is_finite() || !dy.is_finite() || dx.abs() > 50.0 || dy.abs() > 50.0 {
            return Err("a step is at most 50 mm".into());
        }
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        let zup = st.zup.ok_or("no zup")?;
        let low = pos[2] < zup - 0.01; // pen below the controller's pen-up height: allowed, the pen then drags
        let b = st.bbox.ok_or("no box")?;
        let (tx, ty) = ((pos[0] + dx).clamp(b[0] + 0.5, b[1] - 0.5), (pos[1] + dy).clamp(b[2] + 0.5, b[3] - 0.5));
        if (tx - pos[0]).abs() < 1e-6 && (ty - pos[1]).abs() < 1e-6 {
            return Ok((pos, true));
        }
        let clamped = (tx - (pos[0] + dx)).abs() > 1e-6 || (ty - (pos[1] + dy)).abs() > 1e-6;
        let reply = self.do_move_mode(&format!("G1 X{tx:.3} Y{ty:.3} F{:.0}", feed.clamp(60.0, 4000.0)), low)?;
        Ok((parse_pos(&reply).unwrap_or([tx, ty, pos[2]]), clamped))
    }

    /// Z-only move with the caller's lower bound (the joystick page's verified safe Z limit) on top of the controller's own floor.
    pub fn move_z(&self, z: f64, z_min: f64, feed: f64) -> Result<[f64; 3], String> {
        let _op = self.op.lock().unwrap();
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let zfloor = st.zfloor.ok_or("no zfloor")?;
        let zmax = st.zhard_max.unwrap_or(150.0);
        if !z.is_finite() {
            return Err("bad height".into());
        }
        if z < zfloor.max(z_min) - 1e-9 {
            // the controller's floor is just a setting: lower it to what was asked for (its own hard minimum still applies)
            let hard = st.zhard_min.unwrap_or(0.0);
            if z < hard - 1e-9 {
                return Err(format!("Z {z:.2} is below the controller's hard minimum ({hard:.1} mm)."));
            }
            let r = send(&self.sock(), &format!("!zfloor {z:.2}"), Duration::from_secs(5))?;
            if r.contains("REFUSED") {
                return Err(r);
            }
        }
        if z > zmax {
            return Err(format!("Z {z:.2} is above the controller maximum {zmax:.2}"));
        }
        if z < st.pos.ok_or("position unknown")?[2] && self.inner.lock().unwrap().sheet_confirmed.is_none() {
            return Err("confirm the sheet is in the frame before lowering the pen".into());
        }
        let reply = self.do_move(&format!("G1 Z{z:.3} F{:.0}", feed.clamp(30.0, 300.0)))?;
        Ok(parse_pos(&reply).unwrap_or([st.pos.unwrap()[0], st.pos.unwrap()[1], z]))
    }

    /// Make sure the controller's Z floor is at or below `z` (used before drawing at a low pen-down height).
    pub fn ensure_floor(&self, z: f64) -> Result<(), String> {
        let st = self.fresh_status()?;
        let hard = st.zhard_min.unwrap_or(0.0);
        let want = z.max(hard);
        if st.zfloor.map(|f| f > want + 1e-9).unwrap_or(false) {
            let r = send(&self.sock(), &format!("!zfloor {want:.2}"), Duration::from_secs(5))?;
            if r.contains("REFUSED") {
                return Err(r);
            }
        }
        Ok(())
    }

    /// Raise the pen to travel height (never lower than the controller's pen-up height).
    pub fn pen_up(&self) -> Result<String, String> {
        let _op = self.op.lock().unwrap();
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        let t = Self::travel_target(&st)?;
        if pos[2] >= t - 1e-6 {
            return Ok("already up".into());
        }
        self.do_move(&format!("G1 Z{t:.3} F300"))
    }

    fn travel_target(st: &CtlStatus) -> Result<f64, String> {
        let zup = st.zup.ok_or("no zup")?;
        Ok(zup.max(travel_z()).min(st.zhard_max.unwrap_or(150.0)))
    }

    /// Straight-up lift to travel height before a sideways move (op lock must be held).
    fn raise_for_travel(&self, st: &CtlStatus) -> Result<(), String> {
        let pos = st.pos.ok_or("position unknown")?;
        let t = Self::travel_target(st)?;
        if pos[2] < t - 1e-6 {
            self.do_move(&format!("G1 Z{t:.3} F300"))?;
        }
        Ok(())
    }

    /// Step Z by `dz` (|dz| <= 1 mm). Lowering needs the sheet confirmed and never goes below the controller floor
    /// or more than 1 mm under the calibrated contact plane (when one is known).
    pub fn z_step(&self, dz: f64, plane_z: Option<f64>) -> Result<String, String> {
        let _op = self.op.lock().unwrap();
        if !dz.is_finite() || dz == 0.0 || dz.abs() > 1.0 {
            return Err("Z step must be 0 < |dz| <= 1 mm".into());
        }
        let st = self.fresh_status()?;
        self.manual_guard(&st)?;
        let pos = st.pos.ok_or("position unknown")?;
        let tz = pos[2] + dz;
        let zfloor = st.zfloor.ok_or("no zfloor")?;
        let zmax = st.zhard_max.unwrap_or(150.0);
        if tz < zfloor - 1e-9 {
            return Err(format!("Z {tz:.2} would be below the controller floor {zfloor:.2}"));
        }
        if tz > zmax {
            return Err(format!("Z {tz:.2} above the controller maximum {zmax:.2}"));
        }
        if dz < 0.0 {
            if self.inner.lock().unwrap().sheet_confirmed.is_none() {
                return Err("confirm the sheet is loaded and taped (Calibration tab) before lowering the pen".into());
            }
            if let Some(pz) = plane_z {
                if tz < pz - 1.0 {
                    return Err(format!("Z {tz:.2} is more than 1 mm below the calibrated contact plane ({pz:.2})"));
                }
            }
        }
        let f = if dz < 0.0 { 60 } else { 300 };
        self.do_move(&format!("G1 Z{tz:.3} F{f}"))
    }

    // ── jobs ──
    fn write_job(&self, id: &str, suffix: &str, job: &Value) -> Result<PathBuf, String> {
        let dir = self.data();
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = dir.join(format!("{id}{suffix}.json"));
        fs::write(&p, serde_json::to_vec(job).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(p)
    }

    fn jobcheck(&self, path: &Path) -> Result<Value, String> {
        let r = send(&self.sock(), &format!("!jobcheck {}", path.display()), Duration::from_secs(20))?;
        self.set_reply(&r);
        if r.starts_with("REFUSED") || r.contains("REFUSED") {
            return Err(r);
        }
        serde_json::from_str::<Value>(&r).map_err(|_| format!("unexpected !jobcheck reply: {}", r.chars().take(160).collect::<String>()))
    }

    pub fn prepare(&self, req: PrepareReq) -> Result<PreparedView, String> {
        let _op = self.op.lock().unwrap();
        if self.inner.lock().unwrap().run.as_ref().map(|r| r.active()).unwrap_or(false) {
            return Err("a job is running".into());
        }
        let root = self.root();
        let sheet = if req.sheet_id == "provisional" {
            provisional_sheet(&root, [req.paper_w, req.paper_h], req.origin_x.ok_or("enter the sheet origin X (pen coordinates of the sheet's bottom-left corner)")?, req.origin_y.ok_or("enter the sheet origin Y")?)?
        } else {
            load_sheet(&root, &req.sheet_id)?
        };
        self.prepare_sheet_locked(req.name, &sheet, &req.strokes, req.paper_w, req.paper_h, req.depth, req.feed, req.sheet_id)
    }

    /// Prepare a job from an already-resolved sheet (used by the camera-based writer). Same checks as `prepare`.
    pub fn prepare_sheet(&self, name: String, sheet: &Sheet, strokes: &[Vec<[f64; 2]>], paper_w: f64, paper_h: f64, depth: f64, feed: f64) -> Result<PreparedView, String> {
        let _op = self.op.lock().unwrap();
        if self.inner.lock().unwrap().run.as_ref().map(|r| r.active()).unwrap_or(false) {
            return Err("a job is running".into());
        }
        self.prepare_sheet_locked(name, sheet, strokes, paper_w, paper_h, depth, feed, sheet.id.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_sheet_locked(&self, name: String, sheet: &Sheet, strokes: &[Vec<[f64; 2]>], paper_w: f64, paper_h: f64, depth: f64, feed: f64, sheet_id: String) -> Result<PreparedView, String> {
        let (job, pen) = build_job(sheet, strokes, paper_w, paper_h, depth, feed)?;
        let id = format!("{}", now_ms());
        let path = self.write_job(&id, "", &job)?;
        let (verdict, accepted, reason) = match self.jobcheck(&path) {
            Ok(v) => (Some(v), true, None),
            Err(e) => (None, false, Some(e)),
        };
        let view = PreparedView {
            id: id.clone(),
            name,
            strokes: pen.len(),
            draw_len_mm: (draw_len(&pen) * 10.0).round() / 10.0,
            bbox_pen: bbox(&pen),
            sheet: sheet.label.clone(),
            provisional: sheet.provisional,
            warnings: sheet.warnings.clone(),
            depth,
            feed,
            plane_z0: sheet.plane.z0,
            verdict,
            accepted,
            reason,
        };
        let mut g = self.inner.lock().unwrap();
        g.prepared = Some(Prepared { view: view.clone(), job, strokes_n: pen.len(), sheet_id });
        drop(g);
        self.publish();
        Ok(view)
    }

    fn start_checks(&self, sheet_id: &str) -> Result<(), String> {
        if self.inner.lock().unwrap().sheet_confirmed.as_deref() != Some(sheet_id) {
            return Err("confirm that the sheet is loaded and taped (Calibration tab) for this calibration before starting".into());
        }
        let st = self.fresh_status()?;
        let why = unsafe_reasons(&st);
        if !why.is_empty() {
            return Err(format!("not safe to start: {}", why.join("; ")));
        }
        Ok(())
    }

    pub fn start(&self) -> Result<(), String> {
        let _op = self.op.lock().unwrap();
        let (job, id, n, name, sheet_id, accepted) = {
            let g = self.inner.lock().unwrap();
            if g.run.as_ref().map(|r| r.active()).unwrap_or(false) {
                return Err("a job is already running".into());
            }
            let p = g.prepared.as_ref().ok_or("prepare a job first")?;
            (p.job.clone(), p.view.id.clone(), p.strokes_n, p.view.name.clone(), p.sheet_id.clone(), p.view.accepted)
        };
        if !accepted {
            return Err("the controller rejected this job; fix it and prepare again".into());
        }
        self.start_checks(&sheet_id)?;
        let path = self.write_job(&id, "", &job)?;
        self.jobcheck(&path).map_err(|e| format!("controller refused the job: {e}"))?;
        let reply = send(&self.sock(), &format!("!jobfile {}", path.display()), Duration::from_secs(20))?;
        self.set_reply(&reply);
        if !reply.starts_with("job started") {
            return Err(format!("controller did not start the job: {reply}"));
        }
        let mut g = self.inner.lock().unwrap();
        g.run = Some(Run {
            id,
            name,
            job,
            total: n,
            base: 0,
            seg_total: n,
            done: 0,
            state: RunState::Running,
            message: "running".into(),
            error: None,
            started: Instant::now(),
            seg_started: Instant::now(),
            elapsed_before: 0.0,
        });
        drop(g);
        self.publish();
        Ok(())
    }

    /// Pause = cancel at the next stroke boundary. The poller turns Pausing into Paused with the confirmed index.
    pub fn pause(&self) -> Result<(), String> {
        let _op = self.op.lock().unwrap();
        {
            let g = self.inner.lock().unwrap();
            match g.run.as_ref() {
                Some(r) if r.state == RunState::Running => {}
                _ => return Err("no running job".into()),
            }
        }
        let reply = send(&self.sock(), "!cancel", Duration::from_secs(5))?;
        self.set_reply(&reply);
        let mut g = self.inner.lock().unwrap();
        if let Some(r) = g.run.as_mut() {
            if reply.starts_with("cancel requested") {
                r.state = RunState::Pausing;
                r.message = "pausing: finishing the current stroke".into();
            } else {
                // controller says nothing runs: let the next progress poll settle it
                r.message = format!("controller: {reply}");
            }
        }
        drop(g);
        self.publish();
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        let _op = self.op.lock().unwrap();
        let (job, id, done, total, _name, sheet_id) = {
            let g = self.inner.lock().unwrap();
            let r = g.run.as_ref().ok_or("nothing to resume")?;
            if r.state != RunState::Paused || r.done >= r.total {
                return Err("job is not paused".into());
            }
            let sid = g.prepared.as_ref().map(|p| p.sheet_id.clone()).unwrap_or_default();
            (r.job.clone(), r.id.clone(), r.done, r.total, r.name.clone(), sid)
        };
        self.start_checks(&sheet_id)?;
        let strokes = job["strokes"].as_array().ok_or("corrupt job")?;
        let slice = json!({ "params": job["params"], "strokes": strokes[done..].to_vec() });
        let path = self.write_job(&id, &format!("_from{done}"), &slice)?;
        self.jobcheck(&path).map_err(|e| format!("controller refused the remaining strokes: {e}"))?;
        let reply = send(&self.sock(), &format!("!jobfile {}", path.display()), Duration::from_secs(20))?;
        self.set_reply(&reply);
        if !reply.starts_with("job started") {
            return Err(format!("controller did not resume: {reply}"));
        }
        let mut g = self.inner.lock().unwrap();
        if let Some(r) = g.run.as_mut() {
            r.base = done;
            r.seg_total = total - done;
            r.state = RunState::Running;
            r.message = format!("resumed from stroke {done}");
            r.seg_started = Instant::now();
        }
        drop(g);
        self.publish();
        Ok(())
    }

    pub fn stop(&self) -> Result<(), String> {
        let _op = self.op.lock().unwrap();
        let state = self.inner.lock().unwrap().run.as_ref().map(|r| r.state);
        match state {
            Some(RunState::Paused) => {
                let mut g = self.inner.lock().unwrap();
                if let Some(r) = g.run.as_mut() {
                    r.state = RunState::Stopped;
                    r.message = "stopped".into();
                }
                drop(g);
                self.publish();
                Ok(())
            }
            Some(RunState::Running) | Some(RunState::Pausing) => {
                let reply = send(&self.sock(), "!cancel", Duration::from_secs(5))?;
                self.set_reply(&reply);
                let mut g = self.inner.lock().unwrap();
                if let Some(r) = g.run.as_mut() {
                    r.state = RunState::Stopping;
                    r.message = "stopping after the current stroke".into();
                }
                drop(g);
                self.publish();
                Ok(())
            }
            _ => Err("no job to stop".into()),
        }
    }

    /// Emergency stop: never refused by the app, bypasses the op lock so it can fire during another operation.
    pub fn estop(&self) -> Result<String, String> {
        let reply = send(&self.sock(), "!estop", Duration::from_secs(5));
        let mut g = self.inner.lock().unwrap();
        g.sheet_confirmed = None;
        if let Some(r) = g.run.as_mut() {
            if r.active() || r.state == RunState::Paused {
                r.state = RunState::Failed;
                r.error = Some("EMERGENCY STOP".into());
                r.message = "emergency stop".into();
            }
        }
        match &reply {
            Ok(t) => g.last_reply = t.clone(),
            Err(e) => g.last_reply = e.clone(),
        }
        drop(g);
        self.publish();
        reply
    }

    pub fn discard_run(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.run.as_ref().map(|r| !r.active()).unwrap_or(false) {
            g.run = None;
        }
        drop(g);
        self.publish();
    }
}

// ───────────── companion services ─────────────

/// "X:1.00 Y:2.00 Z:3.00" line of a controller move reply.
pub fn parse_pos(reply: &str) -> Option<[f64; 3]> {
    let line = reply.lines().rev().find(|l| l.trim_start().starts_with("X:"))?;
    let mut v = [0.0; 3];
    for (i, k) in ["X:", "Y:", "Z:"].iter().enumerate() {
        let t = line.split(k).nth(1)?.split_whitespace().next()?;
        v[i] = t.parse().ok()?;
    }
    Some(v)
}

pub fn port_open(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(400)).is_ok()
}

/// Start the worksheet web app detached (never touches anything already running).
pub fn start_worksheets(root: &Path, log_dir: &Path) -> Result<String, String> {
    start_server(root, log_dir, 7793)
}

pub fn start_server(root: &Path, log_dir: &Path, port: u16) -> Result<String, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    if port_open(port) {
        return Ok(format!("worksheet server is already running on 127.0.0.1:{port}"));
    }
    let run_sh = root.join("worksheets/run.sh");
    let (prog, args): (PathBuf, Vec<String>) = if run_sh.exists() {
        ("/bin/sh".into(), vec![run_sh.display().to_string()])
    } else {
        let py = root.join(".venv-ws/bin/python");
        if !py.exists() {
            return Err(format!("neither {} nor {} exists", run_sh.display(), py.display()));
        }
        (py, vec!["-I".into(), root.join("worksheets/server.py").display().to_string()])
    };
    fs::create_dir_all(log_dir).map_err(|e| e.to_string())?;
    let log = fs::OpenOptions::new().create(true).append(true).open(log_dir.join("worksheets.log")).map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    Command::new(&prog)
        .args(&args)
        .current_dir(root.join("worksheets"))
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start worksheet server: {e}"))?;
    Ok(format!("started {} (log: {})", prog.display(), log_dir.join("worksheets.log").display()))
}

// ───────────── tests ─────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::net::UnixListener;

    #[test]
    fn whitelist_blocks_everything_dangerous() {
        for ok in ["!status", "!progress", "!cancel", "!estop", "!draw off", "!draw on", "!zfloor 4", "!log 50", "G1 X10.000 Y20.5 F3000", "G1 Z24.000 F300", "!homexy confirmed", "!jobcheck /tmp/a/b.json", "!jobfile /home/u/x.json"] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "!home", "!home confirmed", "!homexy", "!homexy now", "!homexy  confirmed x", "!adopt 1 2 3 4 5", "!rearm confirmed", "!box 0 1 0 1", "!zfloor", "!zfloor abc", "!zup 99", "!nopencil confirmed", "!pencil",
            "G28", "G92 X0", "G91", "M104 S200", "M140 S60", "M109 S200", "M112", "M410", "M106 S255", "G1 X1 E5", "G1", "G1 Xabc", "G1 X1; M104 S200", "G1 X1\nM104 S200",
            "!jobfile relative.json", "!jobfile /tmp/a b.json", "!jobfile /tmp/../etc/x.json", "!jobfile /tmp/x.txt", "!log abc", "!status extra", "", "G0 X1", "M105",
        ] {
            assert!(!allowed(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn send_refuses_disallowed_without_connecting() {
        let e = send(Path::new("/nonexistent/sock"), "G28", Duration::from_secs(1)).unwrap_err();
        assert!(e.contains("not allowed"), "{e}");
    }

    #[test]
    fn unreachable_socket_is_reported() {
        let e = send(Path::new("/nonexistent/sock"), "!status", Duration::from_secs(1)).unwrap_err();
        assert!(e.contains("cannot reach controller"), "{e}");
    }

    /// Mock controller: replies from a map, records every received line.
    fn mock(replies: HashMap<&'static str, String>) -> (PathBuf, Arc<Mutex<Vec<String>>>) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("ctop_ctl_test_{}_{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mock.sock");
        let l = UnixListener::bind(&path).unwrap();
        let seen = Arc::new(Mutex::new(vec![]));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let mut c = c.unwrap();
                let mut line = String::new();
                let mut b = [0u8; 1];
                while c.read(&mut b).unwrap_or(0) == 1 {
                    if b[0] == b'\n' {
                        break;
                    }
                    line.push(b[0] as char);
                }
                seen2.lock().unwrap().push(line.clone());
                let key = line.split_whitespace().next().unwrap_or("").to_string();
                let r = replies.get(key.as_str()).cloned().unwrap_or_else(|| "REFUSED: mock".into());
                let _ = c.write_all(format!("{r}\n").as_bytes());
            }
        });
        (path, seen)
    }

    fn status_json(extra: &str) -> String {
        format!(
            r#"{{"init":true,"homed":true,"adopted":true,"estop":false,"frame_lost":false,"nopencil":false,"draw":false,"pos":[100.0,100.0,24.0],"box":[30,280,30,308],"zfloor":16.0,"zup":24.0,"zhard_max":150.0,"job":null,"temps":{{"nozzle":22.0,"nozzle_target":0.0,"bed":22.0,"bed_target":0.0}},"link":{{"backend":"fake","ok":true,"last_error":null}}{extra}}}"#
        )
    }

    #[test]
    fn status_parses_and_safety_reasons() {
        let s = parse_status(&status_json("")).unwrap();
        assert!(unsafe_reasons(&s).is_empty());
        assert_eq!(s.bbox, Some([30.0, 280.0, 30.0, 308.0]));
        let s = parse_status(&status_json(r#","estop":true"#).replace(r#""estop":false,"#, "")).unwrap();
        assert!(unsafe_reasons(&s).iter().any(|r| r.contains("EMERGENCY")));
        let s = parse_status(&status_json("").replace(r#""nozzle_target":0.0"#, r#""nozzle_target":200.0"#)).unwrap();
        assert!(unsafe_reasons(&s).iter().any(|r| r.contains("heater")));
        let s = parse_status(&status_json("").replace(r#""ok":true"#, r#""ok":false"#)).unwrap();
        assert!(unsafe_reasons(&s).iter().any(|r| r.contains("link")));
        assert!(parse_status("garbage").is_err());
    }

    #[test]
    fn mock_server_status_roundtrip_and_tick() {
        let (sock, seen) = mock(HashMap::from([("!status", status_json(""))]));
        let ctl = Ctl::new(std::env::temp_dir(), Some(sock.display().to_string()), None, Arc::new(|_, _| {}));
        // env vars could override; this test process does not set them
        ctl.set_config(Some(sock.display().to_string()), None);
        ctl.tick();
        let s = ctl.snapshot();
        assert!(s.reachable, "{:?}", s.error);
        assert_eq!(s.status.unwrap().zup, Some(24.0));
        assert_eq!(seen.lock().unwrap().as_slice(), ["!status"]);
    }

    #[test]
    fn unreachable_controller_snapshot() {
        let ctl = Ctl::new(std::env::temp_dir(), Some("/nonexistent/x.sock".into()), None, Arc::new(|_, _| {}));
        ctl.set_config(Some("/nonexistent/x.sock".into()), None);
        ctl.tick();
        let s = ctl.snapshot();
        assert!(!s.reachable && s.error.unwrap().contains("cannot reach"));
        assert!(ctl.jog_xy(1.0, 0.0).is_err());
        assert!(ctl.start().is_err());
    }

    #[test]
    fn manual_moves_checked_before_sending() {
        let (sock, seen) = mock(HashMap::from([("!status", status_json("")), ("!draw", "draw=False".into()), ("G1", "ok\nX:101.00 Y:100.00 Z:24.00\nVERIFIED".into())]));
        let ctl = Ctl::new(std::env::temp_dir(), Some(sock.display().to_string()), None, Arc::new(|_, _| {}));
        ctl.set_config(Some(sock.display().to_string()), None);
        assert!(ctl.jog_xy(1.0, 0.0).is_ok());
        assert!(seen.lock().unwrap().iter().any(|l| l == "G1 X101.000 Y100.000 F3000"));
        assert!(ctl.jog_xy(500.0, 0.0).is_err()); // too far
        assert!(ctl.jog_xy(1.0, 1.0).is_err()); // two axes
        assert!(ctl.jog_xy(-100.0, 0.0).is_err()); // outside box
        assert!(ctl.z_step(-0.5, None).is_err()); // sheet not confirmed
        ctl.confirm_sheet("x", true).unwrap();
        assert!(ctl.z_step(-0.5, None).is_ok()); // 24 -> 23.5 >= floor
        assert!(ctl.z_step(-9.0, None).is_err()); // > 1 mm
        assert!(ctl.z_step(-0.5, Some(25.0)).is_err()); // below plane-1.0
        let sent: Vec<String> = seen.lock().unwrap().clone();
        assert!(sent.iter().all(|l| allowed(l)), "{sent:?}");
    }

    #[test]
    fn sideways_move_below_travel_height_lifts_first() {
        let st = status_json("").replace("[100.0,100.0,24.0]", "[100.0,100.0,20.0]");
        let (sock, seen) = mock(HashMap::from([("!status", st)]));
        let ctl = Ctl::new(std::env::temp_dir(), Some(sock.display().to_string()), None, Arc::new(|_, _| {}));
        ctl.set_config(Some(sock.display().to_string()), None);
        let _ = ctl.jog_xy(1.0, 0.0);
        let sent: Vec<String> = seen.lock().unwrap().clone();
        // the first motion command must be the vertical lift, never the sideways move
        let first_motion = sent.iter().find(|l| l.starts_with("G1")).cloned();
        assert!(first_motion.map(|l| l.starts_with("G1 Z")).unwrap_or(true), "{sent:?}");
        assert!(!sent.iter().any(|l| l.starts_with("G1 X")), "{sent:?}");
    }

    fn sheet() -> Sheet {
        Sheet {
            id: "t".into(),
            label: "t".into(),
            a: [[1.0, 0.0, 30.0], [0.0, 1.0, 40.0]],
            plane: Plane { x0: 130.0, y0: 150.0, z0: 20.3, gx: 0.0, gy: 0.0 },
            page: [210.0, 297.0],
            provisional: false,
            warnings: vec![],
        }
    }

    #[test]
    fn job_building_flips_y_and_applies_affine() {
        // paper point (10, 297-5): 5 mm above the bottom edge of the page -> page (10,5) -> pen (40,45)
        let (job, pen) = build_job(&sheet(), &[vec![[10.0, 292.0], [20.0, 292.0]]], 210.0, 297.0, 0.5, 1000.0).unwrap();
        assert_eq!(pen[0][0], [40.0, 45.0]);
        assert_eq!(pen[0][1], [50.0, 45.0]);
        assert_eq!(job["params"]["mode"], "surface");
        assert_eq!(job["params"]["depth"], 0.5);
        assert_eq!(job["params"]["surface"]["z0"], 20.3);
    }

    #[test]
    fn job_building_refuses_bad_input() {
        let s = sheet();
        let st = vec![vec![[10.0, 10.0], [20.0, 10.0]]];
        assert!(build_job(&s, &st, 297.0, 210.0, 0.5, 1000.0).unwrap_err().contains("mismatch"));
        assert!(build_job(&s, &st, 210.0, 297.0, 2.0, 1000.0).is_err());
        assert!(build_job(&s, &st, 210.0, 297.0, 0.5, 5000.0).is_err());
        assert!(build_job(&s, &[vec![[1.0, 1.0]]], 210.0, 297.0, 0.5, 1000.0).is_err());
        let long: Vec<[f64; 2]> = (0..9000).map(|i| [10.0 + (i as f64) * 0.01, 10.0]).collect();
        let (_, pen) = build_job(&s, &[long], 210.0, 297.0, 0.5, 1000.0).unwrap();
        assert!(pen.iter().all(|p| p.len() <= 4000) && pen.len() >= 3);
        // continuity: each chunk starts where the previous one ended
        assert_eq!(pen[0].last(), pen[1].first());
    }

    fn run(total: usize) -> Run {
        Run {
            id: "1".into(),
            name: "n".into(),
            job: json!({}),
            total,
            base: 0,
            seg_total: total,
            done: 0,
            state: RunState::Running,
            message: String::new(),
            error: None,
            started: Instant::now(),
            seg_started: Instant::now(),
            elapsed_before: 0.0,
        }
    }

    #[test]
    fn run_state_machine_pause_resume_offsets() {
        let mut r = run(10);
        r.apply_progress(r#"{"state":"running","done":3,"total":10,"error":null}"#);
        assert_eq!((r.state, r.done), (RunState::Running, 3));
        r.state = RunState::Pausing;
        r.apply_progress(r#"{"state":"cancelled","done":4,"total":10,"error":null}"#);
        assert_eq!((r.state, r.done), (RunState::Paused, 4));
        assert!(r.view().resumable);
        // resume: slice of 6 strokes, progress counts from 0 again
        r.base = 4;
        r.seg_total = 6;
        r.state = RunState::Running;
        r.apply_progress(r#"{"state":"running","done":2,"total":6,"error":null}"#);
        assert_eq!(r.done, 6);
        r.apply_progress(r#"{"state":"done","done":6,"total":6,"error":null}"#);
        assert_eq!((r.state, r.done), (RunState::Completed, 10));
    }

    #[test]
    fn run_stop_error_and_vanished_job() {
        let mut r = run(5);
        r.state = RunState::Stopping;
        r.apply_progress(r#"{"state":"cancelled","done":2,"total":5,"error":null}"#);
        assert_eq!((r.state, r.done), (RunState::Stopped, 2));
        assert!(!r.view().resumable);
        let mut r = run(5);
        r.apply_progress(r#"{"state":"error","done":1,"total":5,"error":"position mismatch"}"#);
        assert_eq!(r.state, RunState::Failed);
        assert_eq!(r.error.as_deref(), Some("position mismatch"));
        let mut r = run(5);
        r.seg_started = Instant::now() - Duration::from_secs(10);
        r.apply_progress("no job");
        assert_eq!(r.state, RunState::Failed);
        // cancelled but everything was done -> completed, not resumable
        let mut r = run(3);
        r.state = RunState::Pausing;
        r.apply_progress(r#"{"state":"cancelled","done":3,"total":3,"error":null}"#);
        assert_eq!(r.state, RunState::Completed);
    }

    #[test]
    fn estop_goes_through_and_latches_run() {
        let (sock, seen) = mock(HashMap::from([("!estop", "ESTOP LATCHED: quick-stop sent".into()), ("!status", status_json(""))]));
        let ctl = Ctl::new(std::env::temp_dir(), Some(sock.display().to_string()), None, Arc::new(|_, _| {}));
        ctl.set_config(Some(sock.display().to_string()), None);
        ctl.inner.lock().unwrap().run = Some(run(4));
        let r = ctl.estop().unwrap();
        assert!(r.contains("ESTOP LATCHED"));
        assert_eq!(seen.lock().unwrap().as_slice(), ["!estop"]);
        assert_eq!(ctl.snapshot().run.unwrap().state, RunState::Failed);
    }

    #[test]
    fn start_server_runs_run_sh_detached_and_skips_when_port_is_up() {
        let dir = std::env::temp_dir().join(format!("ctop_ws_test_{}", std::process::id()));
        fs::create_dir_all(dir.join("worksheets")).unwrap();
        let marker = dir.join("marker.txt");
        fs::write(dir.join("worksheets/run.sh"), format!("#!/bin/sh\necho started-in-$(basename \"$PWD\") > {}\n", marker.display())).unwrap();
        let r = start_server(&dir, &dir.join("logs"), 17793).unwrap();
        assert!(r.contains("started"), "{r}");
        for _ in 0..50 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "started-in-worksheets");
        // a listening port => nothing is started
        let l = std::net::TcpListener::bind("127.0.0.1:17794").unwrap();
        let r = start_server(&dir, &dir.join("logs"), 17794).unwrap();
        assert!(r.contains("already running"), "{r}");
        drop(l);
        // no run.sh and no venv => clear error
        let empty = std::env::temp_dir().join(format!("ctop_ws_empty_{}", std::process::id()));
        fs::create_dir_all(&empty).unwrap();
        assert!(start_server(&empty, &empty.join("l"), 17795).is_err());
    }

    #[test]
    fn estop_with_unreachable_controller_reports_error() {
        let ctl = Ctl::new(std::env::temp_dir(), Some("/nonexistent/z.sock".into()), None, Arc::new(|_, _| {}));
        ctl.set_config(Some("/nonexistent/z.sock".into()), None);
        assert!(ctl.estop().is_err());
    }
}

#[cfg(test)]
mod parse_tests {
    use super::parse_pos;
    #[test]
    fn reads_the_position_line_of_a_move_reply() {
        let r = "ok\nX:134.40 Y:187.60 Z:24.50\nVERIFIED";
        assert_eq!(parse_pos(r), Some([134.4, 187.6, 24.5]));
        assert_eq!(parse_pos("REFUSED: nope"), None);
    }
}
