//! Machine actor: owns the transport on one thread and is the *only* thing that talks to the
//! controller. It implements the connection lifecycle, explicit arming, validated manual motion,
//! and the job executor with separate "accepted" (ok) and "confirmed complete" (M400) tracking.
//!
//! Safety invariants enforced here:
//!  * nothing moves on connect; position starts UNKNOWN and every reset/loss makes it unknown again
//!  * motion requires: link ready, firmware identity confirmed, armed, axes verified
//!  * manual motion is refused while a job exists; jobs are refused unless calibration is complete
//!  * every outgoing line passes the command allow-list; programs are re-verified before starting
//!  * on any link loss, halt, reset, error or timeout we stop sending and never auto-resume

use crate::calibration::{Calibration, FirmwareLimits, Resolved};
use crate::gcode::{LineKind, ProgramLine};
use crate::marlin::{parse_rx, Rx};
use crate::safety::{check_allowed, verify_program, JogPolicy, Limits};
use crate::transport::Transport;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// ───────────────────────────── public types ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    None,
    Simulation,
    Hardware,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Link {
    Disconnected,
    Connecting,
    Ready,
    /// Firmware halted (M112 / kill). Needs a hardware reset.
    Halted,
    /// Connection dropped or controller stopped answering.
    Lost,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArmMode {
    /// Motion limited by the operator-defined safe envelope.
    Envelope,
    /// No envelope yet: firmware limits only, tiny downward Z steps.
    FirmwareOnly,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Busy {
    Idle,
    Moving,
    Homing,
    Syncing,
    Job,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Running,
    /// Application-side submission paused; already-buffered moves still run.
    Paused,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    /// Stopped by firmware quick-stop.
    Stopped,
    /// Link lost mid-job.
    Interrupted,
}

impl JobState {
    pub fn is_active(self) -> bool {
        matches!(self, JobState::Running | JobState::Paused | JobState::Cancelling)
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareInfo {
    pub name: String,
    pub machine_type: String,
    pub is_marlin: bool,
    /// From `Cap:EMERGENCY_PARSER` in M115; None if the firmware did not say.
    pub emergency_parser: Option<bool>,
    pub raw: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub id: String,
    pub name: String,
    pub state: JobState,
    pub total_lines: usize,
    /// Lines handed to the firmware.
    pub sent: usize,
    /// Lines the firmware acknowledged (queued — NOT necessarily executed).
    pub accepted: usize,
    /// Strokes proven finished by an M400.
    pub confirmed_strokes: usize,
    pub total_strokes: usize,
    pub elapsed_s: f64,
    pub estimate_s: f64,
    pub message: String,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub current_stroke: Option<usize>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub session: u64,
    pub mode: Mode,
    pub link: Link,
    pub device: String,
    pub firmware: Option<FirmwareInfo>,
    pub fw_limits: Option<FirmwareLimits>,
    pub armed: Option<ArmMode>,
    pub axes_known: [bool; 3],
    /// Believed nozzle position (last firmware report / commanded target).
    pub position: Option<[f64; 3]>,
    pub position_source: String,
    pub feed: f64,
    pub activity: String,
    pub busy: Busy,
    pub last_error: Option<String>,
    pub attention: Option<String>,
    pub pending: usize,
    pub tx_count: u64,
    pub rx_count: u64,
    pub job: Option<JobProgress>,
}

impl Snapshot {
    pub fn disconnected(session: u64) -> Snapshot {
        Snapshot {
            session,
            mode: Mode::None,
            link: Link::Disconnected,
            device: String::new(),
            firmware: None,
            fw_limits: None,
            armed: None,
            axes_known: [false; 3],
            position: None,
            position_source: "unknown".into(),
            feed: 0.0,
            activity: "Not connected".into(),
            busy: Busy::Idle,
            last_error: None,
            attention: None,
            pending: 0,
            tx_count: 0,
            rx_count: 0,
            job: None,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub t_ms: u64,
    /// tx | rx | info | warn | error
    pub dir: &'static str,
    pub text: String,
}

pub enum Event {
    State(Box<Snapshot>),
    Log(Vec<LogEntry>),
}
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ActorConfig {
    pub ack_timeout: Duration,
    pub home_timeout: Duration,
    pub banner_wait: Duration,
    pub handshake_timeout: Duration,
    pub window_bytes: usize,
    pub window_cmds: usize,
    pub sync_quiet: Duration,
    pub heartbeat: Duration,
    pub limits: Limits,
}

impl Default for ActorConfig {
    fn default() -> Self {
        ActorConfig {
            ack_timeout: Duration::from_secs(30),
            home_timeout: Duration::from_secs(180),
            banner_wait: Duration::from_secs(5),
            handshake_timeout: Duration::from_secs(20),
            window_bytes: 96,
            window_cmds: 4,
            sync_quiet: Duration::from_millis(300),
            heartbeat: Duration::from_secs(3),
            limits: Limits::default(),
        }
    }
}

pub struct JobSpec {
    pub id: String,
    pub name: String,
    pub lines: Vec<ProgramLine>,
    pub estimate_s: f64,
    /// Serialized calibration the program was generated against.
    pub calibration_key: String,
}

pub enum Cmd {
    Arm { mode: ArmMode, acknowledged: bool },
    Disarm,
    Goto { x: Option<f64>, y: Option<f64>, z: Option<f64>, feed: f64 },
    /// Relative step from the believed position (accumulates correctly for held keys).
    Jog { axis: usize, delta: f64, feed: f64 },
    Home { axes: [bool; 3] },
    /// Operator states the displayed position matches physical reality.
    VerifyPosition,
    /// Re-read the firmware position (M400 + M114).
    Sync,
    StartJob { spec: Box<JobSpec>, paper_confirmed: bool },
    Pause,
    Resume,
    CancelJob,
    QuickStop,
    Emergency,
    Disconnect,
}

type Reply = Sender<Result<(), String>>;

pub struct MachineHandle {
    tx: Sender<(Cmd, Option<Reply>)>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl MachineHandle {
    pub fn request(&self, cmd: Cmd) -> Result<(), String> {
        let (rtx, rrx) = mpsc::channel();
        self.tx.send((cmd, Some(rtx))).map_err(|_| "Machine thread is not running".to_string())?;
        rrx.recv_timeout(Duration::from_secs(5)).map_err(|_| "Machine did not respond".to_string())?
    }
    pub fn fire(&self, cmd: Cmd) {
        let _ = self.tx.send((cmd, None));
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
    pub fn is_finished(&self) -> bool {
        self.join.lock().unwrap().as_ref().map_or(true, |j| j.is_finished())
    }
    pub fn join(&self) {
        if let Some(j) = self.join.lock().unwrap().take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(
    transport: Box<dyn Transport>,
    mode: Mode,
    session: u64,
    calib: Arc<Mutex<Calibration>>,
    cfg: ActorConfig,
    sink: EventSink,
) -> MachineHandle {
    let (tx, rx) = mpsc::channel();
    let mut snap = Snapshot::disconnected(session);
    snap.mode = mode;
    snap.device = transport.describe();
    snap.link = Link::Connecting;
    snap.activity = "Opening connection".into();
    let snapshot = Arc::new(Mutex::new(snap.clone()));
    let mut actor = Actor::new(transport, rx, snapshot.clone(), snap, calib, cfg, sink);
    let join = thread::Builder::new().name("machine".into()).spawn(move || actor.run()).expect("spawn machine");
    MachineHandle { tx, snapshot, join: Mutex::new(Some(join)) }
}

// ───────────────────────────── actor internals ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Purpose {
    Job(usize),
    Manual,
    SyncWait,
    SyncReport,
    Home([bool; 3]),
    FinalCheck,
    CancelLift,
    CancelWait,
    Stop,
    Heartbeat,
}

struct Pending {
    len: usize,
    purpose: Purpose,
}

struct Job {
    spec: JobSpec,
    cal: Resolved,
    state: JobState,
    cursor: usize,
    accepted: usize,
    confirmed_strokes: usize,
    started: Instant,
    ended: Option<Instant>,
    message: String,
    error: Option<String>,
    warnings: Vec<String>,
    final_check: bool,
    cancel_sent: bool,
    total_strokes: usize,
    current_stroke: Option<usize>,
}

struct Actor {
    t: Box<dyn Transport>,
    rx: Receiver<(Cmd, Option<Reply>)>,
    shared: Arc<Mutex<Snapshot>>,
    s: Snapshot,
    calib: Arc<Mutex<Calibration>>,
    cfg: ActorConfig,
    sink: EventSink,
    alive: bool,
    t0: Instant,
    pending: VecDeque<Pending>,
    last_rx: Instant,
    last_emit: Instant,
    last_beat: Instant,
    log: Vec<LogEntry>,
    pos_cmd: [f64; 3],
    last_report: Option<[f64; 3]>,
    sync_due: Option<Instant>,
    job: Option<Job>,
    z_increases_up: bool,
}

impl Actor {
    fn new(
        t: Box<dyn Transport>,
        rx: Receiver<(Cmd, Option<Reply>)>,
        shared: Arc<Mutex<Snapshot>>,
        s: Snapshot,
        calib: Arc<Mutex<Calibration>>,
        cfg: ActorConfig,
        sink: EventSink,
    ) -> Actor {
        let now = Instant::now();
        Actor {
            t,
            rx,
            shared,
            s,
            calib,
            cfg,
            sink,
            alive: true,
            t0: now,
            pending: VecDeque::new(),
            last_rx: now,
            last_emit: now - Duration::from_secs(1),
            last_beat: now,
            log: Vec::new(),
            pos_cmd: [0.0; 3],
            last_report: None,
            sync_due: None,
            job: None,
            z_increases_up: true,
        }
    }

    // ── logging / state publishing ──

    fn log(&mut self, dir: &'static str, text: impl Into<String>) {
        let e = LogEntry { t_ms: self.t0.elapsed().as_millis() as u64, dir, text: text.into() };
        self.log.push(e);
    }

    fn publish(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_emit) < Duration::from_millis(90) {
            return;
        }
        self.last_emit = now;
        self.s.pending = self.pending.len();
        self.s.job = self.job.as_ref().map(|j| self.job_progress(j));
        *self.shared.lock().unwrap() = self.s.clone();
        (self.sink)(Event::State(Box::new(self.s.clone())));
        if !self.log.is_empty() {
            let batch = std::mem::take(&mut self.log);
            (self.sink)(Event::Log(batch));
        }
    }

    fn job_progress(&self, j: &Job) -> JobProgress {
        let end = j.ended.unwrap_or_else(Instant::now);
        JobProgress {
            id: j.spec.id.clone(),
            name: j.spec.name.clone(),
            state: j.state,
            total_lines: j.spec.lines.len(),
            sent: j.cursor,
            accepted: j.accepted,
            confirmed_strokes: j.confirmed_strokes,
            total_strokes: j.total_strokes,
            elapsed_s: end.duration_since(j.started).as_secs_f64(),
            estimate_s: j.spec.estimate_s,
            message: j.message.clone(),
            error: j.error.clone(),
            warnings: j.warnings.clone(),
            current_stroke: j.current_stroke,
        }
    }

    fn set_activity(&mut self, a: impl Into<String>) {
        self.s.activity = a.into();
    }

    // ── main loop ──

    fn run(&mut self) {
        self.publish(true);
        self.handshake();
        while self.alive {
            self.drain_commands();
            self.poll_rx(Duration::from_millis(6));
            self.pump_job();
            self.tick();
            self.publish(false);
        }
        self.t.close();
        if self.s.link != Link::Halted && self.s.link != Link::Lost {
            self.s.link = Link::Disconnected;
            self.s.activity = "Disconnected".into();
        }
        self.s.armed = None;
        self.publish(true);
    }

    fn handshake(&mut self) {
        self.s.link = Link::Connecting;
        self.set_activity("Waiting for firmware to start (opening a serial port usually resets the controller)");
        self.publish(true);
        // 1. banner
        let deadline = Instant::now() + self.cfg.banner_wait;
        let mut saw_start = false;
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline && self.alive {
            if self.check_disconnect_only() {
                return;
            }
            match self.t.read_line(Duration::from_millis(50)) {
                Ok(Some(l)) => {
                    quiet_since = Instant::now();
                    self.s.rx_count += 1;
                    self.log("rx", l.clone());
                    if parse_rx(&l) == Rx::Start {
                        saw_start = true;
                    }
                }
                Ok(None) => {
                    if saw_start && quiet_since.elapsed() > Duration::from_millis(600) {
                        break;
                    }
                }
                Err(e) => return self.link_lost(format!("Connection failed during startup: {e}")),
            }
            self.publish(false);
        }
        // 2. identify
        self.set_activity("Confirming firmware identity (M115)");
        self.publish(true);
        let hs_deadline = Instant::now() + self.cfg.handshake_timeout;
        let mut info: Option<FirmwareInfo> = None;
        let mut caps: Vec<(String, bool)> = Vec::new();
        let mut attempts = 0;
        while info.is_none() && Instant::now() < hs_deadline && self.alive {
            if self.check_disconnect_only() {
                return;
            }
            attempts += 1;
            if let Err(e) = self.raw_write("M115") {
                return self.link_lost(format!("Write failed: {e}"));
            }
            let try_until = Instant::now() + Duration::from_millis(2500);
            let mut got_ok = false;
            while Instant::now() < try_until && !got_ok && self.alive {
                match self.t.read_line(Duration::from_millis(50)) {
                    Ok(Some(l)) => {
                        self.s.rx_count += 1;
                        self.log("rx", l.clone());
                        match parse_rx(&l) {
                            Rx::FwInfo(m) => {
                                let name = m.get("FIRMWARE_NAME").cloned().unwrap_or_default();
                                info = Some(FirmwareInfo {
                                    is_marlin: name.to_ascii_lowercase().contains("marlin"),
                                    machine_type: m.get("MACHINE_TYPE").cloned().unwrap_or_default(),
                                    emergency_parser: None,
                                    name,
                                    raw: l.clone(),
                                });
                            }
                            Rx::Cap(k, v) => caps.push((k, v)),
                            Rx::Ok => got_ok = true,
                            _ => {}
                        }
                    }
                    Ok(None) => {}
                    Err(e) => return self.link_lost(format!("Connection lost: {e}")),
                }
            }
            let _ = attempts;
        }
        let info = info.map(|mut i| {
            i.emergency_parser = caps.iter().find(|c| c.0 == "EMERGENCY_PARSER").map(|c| c.1);
            i
        });
        let Some(info) = info else {
            return self.link_lost("The controller never answered M115 — is this a Marlin printer on the right port/baud?".into());
        };
        // let the trailing ok drain, then read soft limits
        if let Err(e) = self.raw_write("M211") {
            return self.link_lost(format!("Write failed: {e}"));
        }
        let until = Instant::now() + Duration::from_millis(2000);
        while Instant::now() < until {
            match self.t.read_line(Duration::from_millis(50)) {
                Ok(Some(l)) => {
                    self.s.rx_count += 1;
                    self.log("rx", l.clone());
                    match parse_rx(&l) {
                        Rx::SoftLimits(lim) => self.s.fw_limits = Some(lim),
                        Rx::Ok if self.s.fw_limits.is_some() => break,
                        _ => {}
                    }
                }
                Ok(None) => {}
                Err(e) => return self.link_lost(format!("Connection lost: {e}")),
            }
        }
        // drain stray ok from M115 if it came after the info line
        self.log("info", format!("Firmware: {} ({})", info.name, info.machine_type));
        if !info.is_marlin {
            self.s.attention = Some("Firmware does not identify as Marlin — arming is disabled.".into());
        }
        self.s.firmware = Some(info);
        self.s.link = Link::Ready;
        self.s.axes_known = [false; 3];
        self.s.position = None;
        self.s.position_source = "unknown".into();
        self.s.attention.get_or_insert_with(|| {
            "Position is UNKNOWN after connecting. Nothing has moved. Home or verify the position before moving.".to_string()
        });
        self.set_activity("Connected — idle, disarmed");
        self.last_rx = Instant::now();
        self.drain_stray_input();
        self.publish(true);
    }

    fn drain_stray_input(&mut self) {
        // swallow any late `ok`/echo from the handshake commands before we start counting acks
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            match self.t.read_line(Duration::from_millis(20)) {
                Ok(Some(l)) => {
                    self.s.rx_count += 1;
                    self.log("rx", l);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    }

    /// During handshake, only Disconnect is honoured. Returns true if we should abort.
    fn check_disconnect_only(&mut self) -> bool {
        while let Ok((cmd, reply)) = self.rx.try_recv() {
            match cmd {
                Cmd::Disconnect => {
                    if let Some(r) = reply {
                        let _ = r.send(Ok(()));
                    }
                    self.alive = false;
                    return true;
                }
                _ => {
                    if let Some(r) = reply {
                        let _ = r.send(Err("Still connecting — wait for the firmware handshake".into()));
                    }
                }
            }
        }
        false
    }

    fn drain_commands(&mut self) {
        while let Ok((cmd, reply)) = self.rx.try_recv() {
            let res = self.handle(cmd);
            if let Err(e) = &res {
                self.log("warn", format!("Refused: {e}"));
            }
            if let Some(r) = reply {
                let _ = r.send(res);
            }
            self.publish(true);
        }
    }

    // ── low-level io ──

    fn raw_write(&mut self, line: &str) -> std::io::Result<()> {
        self.log("tx", line.to_string());
        self.s.tx_count += 1;
        self.t.write_line(line)
    }

    /// Validated send that expects an `ok`.
    fn send(&mut self, line: &str, purpose: Purpose) -> Result<(), String> {
        check_allowed(line)?;
        if self.s.link != Link::Ready {
            return Err("Link is not ready".into());
        }
        match self.raw_write(line) {
            Ok(()) => {
                if self.pending.is_empty() {
                    self.last_rx = Instant::now();
                }
                self.pending.push_back(Pending { len: line.len() + 1, purpose });
                Ok(())
            }
            Err(e) => {
                self.link_lost(format!("Write failed: {e}"));
                Err("Connection lost".into())
            }
        }
    }

    fn poll_rx(&mut self, first: Duration) {
        let mut wait = first;
        for _ in 0..200 {
            match self.t.read_line(wait) {
                Ok(Some(l)) => {
                    wait = Duration::from_millis(0);
                    self.last_rx = Instant::now();
                    self.s.rx_count += 1;
                    self.on_rx(&l);
                    if !self.alive {
                        return;
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    if self.s.link == Link::Ready || self.s.link == Link::Halted {
                        self.link_lost(format!("Connection lost: {e}"));
                    }
                    return;
                }
            }
        }
    }

    fn on_rx(&mut self, line: &str) {
        let rx = parse_rx(line);
        match rx {
            Rx::Ok => {
                self.log("rx", line);
                match self.pending.pop_front() {
                    Some(p) => self.on_ack(p),
                    None => self.log("warn", "Unexpected 'ok' with no command outstanding"),
                }
            }
            Rx::Busy => self.log("rx", line),
            Rx::Position(p) => {
                self.log("rx", line);
                self.last_report = Some(p);
            }
            Rx::Start => {
                self.log("error", "Controller RESET (banner seen) — position lost");
                self.pending.clear();
                self.s.axes_known = [false; 3];
                self.s.armed = None;
                self.s.position = None;
                self.s.position_source = "unknown".into();
                self.s.attention = Some("The controller reset. Positions are invalid; re-home or verify before moving. Not resuming.".into());
                self.fail_job(JobState::Failed, "Controller reset during job".into());
                self.set_activity("Controller reset — disarmed");
            }
            Rx::Halted(m) => {
                self.log("error", line);
                self.on_halted(format!("Firmware halted: {m}"));
            }
            Rx::Error(m) => {
                self.log("error", line);
                self.s.last_error = Some(m.clone());
                self.fail_job(JobState::Failed, format!("Firmware error: {m}"));
                self.s.attention = Some(format!("Firmware error: {m}"));
            }
            Rx::UnknownCommand(m) => {
                self.log("error", line);
                self.s.last_error = Some(m.clone());
                self.fail_job(JobState::Failed, format!("Firmware did not understand a command: {m}"));
            }
            Rx::Resend(n) => {
                self.log("error", line);
                self.fail_job(JobState::Failed, format!("Firmware asked to resend line {n} (link corruption)"));
            }
            Rx::SoftLimits(l) => {
                self.log("rx", line);
                self.s.fw_limits = Some(l);
            }
            Rx::FwInfo(_) | Rx::Echo(_) | Rx::Cap(..) | Rx::Other(_) => self.log("rx", line),
        }
    }

    fn on_halted(&mut self, why: String) {
        self.s.link = Link::Halted;
        self.s.armed = None;
        self.s.axes_known = [false; 3];
        self.s.position = None;
        self.s.position_source = "unknown".into();
        self.s.last_error = Some(why.clone());
        self.s.attention = Some(format!("{why}. The controller needs a hardware reset: disconnect and reconnect (this resets most boards) or cycle printer power."));
        self.pending.clear();
        self.fail_job(JobState::Failed, why);
        self.set_activity("HALTED");
        self.publish(true);
    }

    fn link_lost(&mut self, why: String) {
        if matches!(self.s.link, Link::Lost) {
            return;
        }
        self.log("error", why.clone());
        self.s.link = Link::Lost;
        self.s.armed = None;
        self.s.axes_known = [false; 3];
        self.s.position = None;
        self.s.position_source = "unknown".into();
        self.s.last_error = Some(why.clone());
        self.s.attention = Some(format!("{why}. Position is now unknown. CtoP will not reconnect or resume on its own."));
        self.pending.clear();
        if let Some(j) = &mut self.job {
            if j.state.is_active() {
                j.state = JobState::Interrupted;
                j.error = Some(why.clone());
                j.message = format!("Link lost after {} of {} strokes were confirmed. The pencil may be down on the paper.", j.confirmed_strokes, j.total_strokes);
                j.ended = Some(Instant::now());
            }
        }
        self.set_activity("Connection lost");
        self.alive = false; // thread ends; controller keeps the Lost snapshot
        self.t.close();
        self.publish(true);
    }

    // ── acknowledgements ──

    fn on_ack(&mut self, p: Pending) {
        match p.purpose {
            Purpose::Job(idx) => self.on_job_ack(idx),
            Purpose::Manual => {}
            Purpose::Heartbeat => {
                if let Some(r) = self.last_report.take() {
                    if !self.job_active() {
                        self.commit_report(r);
                    }
                }
            }
            Purpose::SyncWait => {}
            Purpose::SyncReport => {
                if let Some(r) = self.last_report.take() {
                    self.commit_report(r);
                }
                if !self.job_active() {
                    self.s.busy = Busy::Idle;
                    self.set_activity("Idle");
                }
            }
            Purpose::Home(axes) => {
                for i in 0..3 {
                    if axes[i] {
                        self.s.axes_known[i] = true;
                        self.pos_cmd[i] = 0.0;
                    }
                }
                self.log("info", "Homing finished — reading position");
                self.s.attention = None;
                self.request_sync();
            }
            Purpose::FinalCheck => {
                let expected = self.job.as_ref().and_then(|j| j.spec.lines.last().and_then(|l| l.target));
                let rep = self.last_report.take();
                if let (Some(e), Some(r)) = (expected, rep) {
                    let d = ((e[0] - r[0]).powi(2) + (e[1] - r[1]).powi(2) + (e[2] - r[2]).powi(2)).sqrt();
                    if d > 0.1 {
                        if let Some(j) = &mut self.job {
                            j.warnings.push(format!("Final position differs from expected by {d:.2} mm (possible lost steps)"));
                        }
                    }
                    self.commit_report(r);
                }
                self.finish_job(JobState::Completed, "All strokes confirmed complete (M400).".into());
            }
            Purpose::CancelLift => {}
            Purpose::CancelWait => {
                self.finish_job(JobState::Cancelled, "Cancelled. Pencil lifted to safe clearance.".into());
                self.request_sync();
            }
            Purpose::Stop => {
                self.s.attention = Some("Quick stop executed. Position may no longer match the firmware's belief. Lift the pencil if needed and verify the position before moving.".into());
                self.request_sync();
            }
        }
    }

    fn on_job_ack(&mut self, idx: usize) {
        let Some(j) = &mut self.job else { return };
        j.accepted = j.accepted.max(idx + 1);
        let line = j.spec.lines.get(idx).cloned();
        let last = idx + 1 == j.spec.lines.len();
        if let Some(l) = line {
            if let Some(t) = l.target {
                self.pos_cmd = t;
            }
            if l.kind == LineKind::Checkpoint {
                if let Some(s) = l.stroke {
                    j.confirmed_strokes = j.confirmed_strokes.max(s + 1);
                }
                if let Some(t) = l.target {
                    self.s.position = Some(t);
                    self.s.position_source = "confirmed at M400 checkpoint".into();
                }
            }
            j.current_stroke = l.stroke.or(j.current_stroke);
        }
        if last && j.state.is_active() && j.state != JobState::Cancelling {
            j.final_check = true;
            let _ = self.send("M114", Purpose::FinalCheck);
        }
    }

    fn commit_report(&mut self, r: [f64; 3]) {
        let all_known = self.s.axes_known.iter().all(|k| *k);
        let d = ((self.pos_cmd[0] - r[0]).powi(2) + (self.pos_cmd[1] - r[1]).powi(2) + (self.pos_cmd[2] - r[2]).powi(2)).sqrt();
        if all_known && d > 0.05 {
            self.log("warn", format!("Firmware reports {:.2},{:.2},{:.2} but we expected {:.2},{:.2},{:.2}", r[0], r[1], r[2], self.pos_cmd[0], self.pos_cmd[1], self.pos_cmd[2]));
        }
        self.pos_cmd = r;
        self.s.position = Some(r);
        self.s.position_source = if all_known {
            "reported by firmware (M114, after M400)".into()
        } else {
            "reported by firmware but NOT VERIFIED".into()
        };
    }

    fn request_sync(&mut self) {
        if self.job.as_ref().map_or(false, |j| j.state.is_active()) || self.s.link != Link::Ready {
            return;
        }
        self.s.busy = Busy::Syncing;
        self.set_activity("Syncing position (M400 + M114)");
        let _ = self.send("M400", Purpose::SyncWait);
        let _ = self.send("M114", Purpose::SyncReport);
    }

    // ── periodic work ──

    fn tick(&mut self) {
        let now = Instant::now();
        if self.s.link != Link::Ready {
            return;
        }
        // ack timeout
        if let Some(front) = self.pending.front() {
            let limit = if matches!(front.purpose, Purpose::Home(_)) { self.cfg.home_timeout } else { self.cfg.ack_timeout };
            if now.duration_since(self.last_rx) > limit {
                let secs = limit.as_secs_f64();
                self.link_lost(format!("No response from the controller for {secs:.1} s; stopped sending"));
                return;
            }
        }
        // quiet-period sync after manual moves
        if let Some(due) = self.sync_due {
            if now >= due && self.pending.iter().all(|p| p.purpose == Purpose::Manual) {
                self.sync_due = None;
                if !self.job_active() {
                    self.request_sync();
                }
            }
        }
        if !self.job_active() && self.pending.is_empty() && self.sync_due.is_none() {
            if self.s.busy != Busy::Idle {
                self.s.busy = Busy::Idle;
            }
            if now.duration_since(self.last_beat) > self.cfg.heartbeat {
                self.last_beat = now;
                let _ = self.send("M114", Purpose::Heartbeat);
            }
        }
    }

    // ── job execution ──

    fn pump_job(&mut self) {
        if self.s.link != Link::Ready {
            return;
        }
        let Some(j) = &mut self.job else { return };
        // cancel sequence once the window drains
        if j.state == JobState::Cancelling {
            if self.pending.is_empty() && !j.cancel_sent {
                j.cancel_sent = true;
                let z = j.cal.z_safe;
                let zf = self.cfg.limits.max_z_feed.min(600.0);
                let cmds = [format!("G1 Z{z} F{zf}"), "M400".to_string()];
                self.log("info", "Cancelling: lifting pencil to safe clearance");
                let _ = self.send(&cmds[0], Purpose::CancelLift);
                let _ = self.send(&cmds[1], Purpose::CancelWait);
            }
            return;
        }
        if j.state != JobState::Running {
            return;
        }
        loop {
            let Some(j) = &mut self.job else { return };
            if j.cursor >= j.spec.lines.len() {
                break;
            }
            let text = j.spec.lines[j.cursor].text.clone();
            let bytes: usize = self.pending.iter().map(|p| p.len).sum();
            let inflight = self.pending.len();
            if inflight >= self.cfg.window_cmds || bytes + text.len() + 1 > self.cfg.window_bytes {
                break;
            }
            let idx = j.cursor;
            j.cursor += 1;
            j.current_stroke = j.spec.lines[idx].stroke;
            if self.send(&text, Purpose::Job(idx)).is_err() {
                return;
            }
        }
    }

    fn fail_job(&mut self, state: JobState, why: String) {
        if let Some(j) = &mut self.job {
            if j.state.is_active() {
                j.state = state;
                j.error = Some(why.clone());
                j.message = format!("{why}. {} of {} strokes were confirmed complete. Check whether the pencil is down.", j.confirmed_strokes, j.total_strokes);
                j.ended = Some(Instant::now());
                self.log("error", format!("Job failed: {why}"));
                self.s.busy = Busy::Idle;
                self.s.axes_known = [false; 3];
                self.s.position_source = "uncertain after failure".into();
                self.s.attention = Some(format!("Job stopped: {why}. Verify the position before moving again."));
            }
        }
    }

    fn finish_job(&mut self, state: JobState, msg: String) {
        if let Some(j) = &mut self.job {
            j.state = state;
            j.message = msg;
            j.ended = Some(Instant::now());
            self.s.busy = Busy::Idle;
            self.log("info", format!("Job finished: {:?}", state));
        }
        self.set_activity("Idle");
    }

    // ── command handling ──

    fn require_ready(&self) -> Result<(), String> {
        match self.s.link {
            Link::Ready => Ok(()),
            Link::Connecting => Err("Still connecting".into()),
            Link::Halted => Err("Controller is halted; it must be reset (reconnect or cycle power)".into()),
            Link::Lost => Err("Connection lost".into()),
            Link::Disconnected => Err("Not connected".into()),
        }
    }

    fn emergency_parser_note(&self) -> String {
        match self.s.firmware.as_ref().and_then(|f| f.emergency_parser) {
            Some(true) => "Firmware reports an emergency parser, so this is handled immediately.".into(),
            Some(false) => "Firmware reports NO emergency parser: this command waits behind everything already queued (including M400), so it is NOT immediate.".into(),
            None => "Firmware did not report whether it has an emergency parser; do not assume this is immediate.".into(),
        }
    }

    fn job_active(&self) -> bool {
        self.job.as_ref().map_or(false, |j| j.state.is_active())
    }

    fn jog_policy(&self) -> Result<JogPolicy, String> {
        let arm = self.s.armed.ok_or("Machine is not armed")?;
        let cal = self.calib.lock().unwrap().clone();
        match arm {
            ArmMode::Envelope => {
                let issues = cal.envelope_issues(self.s.fw_limits.as_ref());
                if !issues.is_empty() {
                    return Err(format!("Safe envelope invalid: {}", issues[0].message));
                }
                let env = cal.envelope().ok_or("Safe envelope not defined")?;
                let z_floor = if cal.verified.z { cal.z_down } else { None };
                Ok(JogPolicy::Envelope { env, z_floor })
            }
            ArmMode::FirmwareOnly => Ok(JogPolicy::FirmwareOnly { fw: self.s.fw_limits.ok_or("Firmware limits unknown")? }),
        }
    }

    fn handle(&mut self, cmd: Cmd) -> Result<(), String> {
        match cmd {
            Cmd::Disconnect => {
                if self.job_active() {
                    return Err("A job is active. Cancel or stop it first (the pencil may be on the paper).".into());
                }
                self.alive = false;
                Ok(())
            }
            Cmd::Emergency => {
                // Bypass everything: M112 goes out immediately.
                let r = self.raw_write("M112");
                self.on_halted("EMERGENCY HALT sent (M112)".into());
                let ep = self.emergency_parser_note();
                self.s.attention = Some(format!("Emergency halt sent (M112). {ep} If anything is colliding, CUT PRINTER POWER. To recover, power-cycle the printer or reconnect (which resets most boards)."));
                r.map_err(|e| format!("M112 write failed: {e}"))
            }
            Cmd::QuickStop => {
                self.require_ready()?;
                let r = self.raw_write("M410");
                self.pending.push_back(Pending { len: 5, purpose: Purpose::Stop });
                if self.job_active() {
                    if let Some(j) = &mut self.job {
                        j.state = JobState::Stopped;
                        j.message = "Firmware quick-stop sent (M410). Commands already in the firmware's 4-slot queue still ran first.".into();
                        j.error = Some("Stopped by operator (quick stop)".into());
                        j.ended = Some(Instant::now());
                    }
                }
                self.s.axes_known = [false; 3];
                self.s.armed = None;
                self.s.busy = Busy::Idle;
                let ep = self.emergency_parser_note();
                self.s.attention = Some(format!("Quick stop (M410) sent. {ep} Up to 4 commands already in the firmware's command queue may still execute. Verify the position before moving."));
                r.map_err(|e| format!("write failed: {e}"))
            }
            Cmd::Disarm => {
                if self.job_active() {
                    return Err("Cannot disarm during a job — use Cancel or Stop".into());
                }
                self.s.armed = None;
                self.set_activity("Disarmed");
                Ok(())
            }
            Cmd::Arm { mode, acknowledged } => {
                self.require_ready()?;
                if self.job_active() {
                    return Err("A job is active".into());
                }
                let fw = self.s.firmware.as_ref().ok_or("Firmware not identified")?;
                if !fw.is_marlin {
                    return Err("Firmware did not identify as Marlin; refusing to arm".into());
                }
                let cal = self.calib.lock().unwrap().clone();
                self.z_increases_up = cal.z_increases_up;
                match mode {
                    ArmMode::Envelope => {
                        let issues = cal.envelope_issues(self.s.fw_limits.as_ref());
                        if let Some(i) = issues.first() {
                            return Err(format!("Safe envelope is not valid ({}: {}). Define it in Calibration, or arm with firmware limits only.", i.field, i.message));
                        }
                    }
                    ArmMode::FirmwareOnly => {
                        if self.s.fw_limits.is_none() {
                            return Err("Firmware limits unknown (M211 did not answer)".into());
                        }
                        if !acknowledged {
                            return Err("You must acknowledge that no calibrated envelope protects this machine".into());
                        }
                    }
                }
                self.s.armed = Some(mode);
                self.log("info", format!("ARMED ({mode:?})"));
                self.set_activity("Armed — motion enabled");
                Ok(())
            }
            Cmd::Sync => {
                self.require_ready()?;
                if self.job_active() {
                    return Err("A job is active".into());
                }
                self.request_sync();
                Ok(())
            }
            Cmd::VerifyPosition => {
                self.require_ready()?;
                if self.job_active() {
                    return Err("A job is active".into());
                }
                let Some(r) = self.s.position else {
                    return Err("No position reading yet — press Read position first".into());
                };
                // operator asserts that the displayed coordinates match the physical carriage
                self.s.axes_known = [true; 3];
                self.pos_cmd = r;
                self.s.position_source = "verified by operator".into();
                self.s.attention = None;
                self.log("info", format!("Operator verified position X{:.2} Y{:.2} Z{:.2}", r[0], r[1], r[2]));
                Ok(())
            }
            Cmd::Home { axes } => {
                self.require_ready()?;
                if self.job_active() {
                    return Err("A job is active".into());
                }
                if !axes.iter().any(|a| *a) {
                    return Err("No axis selected".into());
                }
                self.s.armed.ok_or("Machine is not armed")?;
                if !self.pending.is_empty() {
                    return Err("Controller busy — wait for the current motion".into());
                }
                let mut cmd = String::from("G28");
                for (i, c) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if axes[i] {
                        cmd.push(' ');
                        cmd.push(*c);
                    }
                }
                self.s.busy = Busy::Homing;
                self.set_activity(format!("Homing: {cmd}"));
                self.send(&cmd, Purpose::Home(axes))
            }
            Cmd::Goto { x, y, z, feed } => self.cmd_goto(x, y, z, feed),
            Cmd::Jog { axis, delta, feed } => {
                if axis > 2 || !delta.is_finite() {
                    return Err("Bad jog request".into());
                }
                let mut t = [None, None, None];
                t[axis] = Some(self.pos_cmd[axis] + delta);
                self.cmd_goto(t[0], t[1], t[2], feed)
            }
            Cmd::StartJob { spec, paper_confirmed } => self.cmd_start_job(*spec, paper_confirmed),
            Cmd::Pause => {
                let j = self.job.as_mut().ok_or("No job")?;
                if j.state != JobState::Running {
                    return Err("Job is not running".into());
                }
                j.state = JobState::Paused;
                j.message = "Command submission paused. Moves already buffered in the firmware (up to ~16) will still finish — this is NOT an instant stop.".into();
                self.log("info", "Submission paused");
                Ok(())
            }
            Cmd::Resume => {
                self.require_ready()?;
                let j = self.job.as_mut().ok_or("No job")?;
                if j.state != JobState::Paused {
                    return Err("Job is not paused".into());
                }
                j.state = JobState::Running;
                j.message = "Running".into();
                Ok(())
            }
            Cmd::CancelJob => {
                self.require_ready()?;
                let j = self.job.as_mut().ok_or("No job")?;
                if !j.state.is_active() {
                    return Err("Job is not active".into());
                }
                j.state = JobState::Cancelling;
                j.message = "Cancelling: unsent commands discarded; waiting for buffered moves, then lifting the pencil.".into();
                Ok(())
            }
        }
    }

    fn cmd_goto(&mut self, x: Option<f64>, y: Option<f64>, z: Option<f64>, feed: f64) -> Result<(), String> {
        self.require_ready()?;
        if self.job_active() {
            return Err("Manual motion is locked while a job is active".into());
        }
        let policy = self.jog_policy()?;
        if self.s.axes_known.iter().any(|k| !*k) {
            return Err("Position is not verified. Home the axes or read and verify the position first.".into());
        }
        if !(feed > 0.0 && feed <= self.cfg.limits.max_jog_feed) {
            return Err(format!("Feedrate must be 0–{} mm/min", self.cfg.limits.max_jog_feed));
        }
        if self.pending.len() > 6 {
            return Err("Too many motion commands queued".into());
        }
        let cur = self.pos_cmd;
        let to = [x.unwrap_or(cur[0]), y.unwrap_or(cur[1]), z.unwrap_or(cur[2])];
        policy.check_target(cur, to, self.z_increases_up)?;
        let mut line = String::from("G1");
        for (v, c) in [(x, 'X'), (y, 'Y'), (z, 'Z')] {
            if let Some(v) = v {
                line.push_str(&format!(" {c}{v:.3}"));
            }
        }
        line.push_str(&format!(" F{feed:.0}"));
        self.send("G90", Purpose::Manual)?;
        self.send(&line, Purpose::Manual)?;
        self.pos_cmd = to;
        self.s.position = Some(to);
        self.s.position_source = "commanded (not yet confirmed)".into();
        self.s.feed = feed;
        self.s.busy = Busy::Moving;
        self.set_activity(format!("Moving: {line}"));
        self.sync_due = Some(Instant::now() + self.cfg.sync_quiet);
        Ok(())
    }

    fn cmd_start_job(&mut self, spec: JobSpec, paper_confirmed: bool) -> Result<(), String> {
        self.require_ready()?;
        if self.job_active() {
            return Err("Another job is already active".into());
        }
        if self.s.armed != Some(ArmMode::Envelope) {
            return Err("Drawing requires the machine to be armed with a calibrated envelope".into());
        }
        if !paper_confirmed {
            return Err("Operator must confirm paper is loaded and the area is clear".into());
        }
        if self.s.axes_known.iter().any(|k| !*k) {
            return Err("Position is not verified".into());
        }
        if !self.pending.is_empty() {
            return Err("Controller is busy".into());
        }
        let cal = self.calib.lock().unwrap().clone();
        let key = serde_json::to_string(&cal).unwrap_or_default();
        if key != spec.calibration_key {
            return Err("Calibration changed after this job was generated — regenerate the job".into());
        }
        let resolved = cal
            .ready_for_drawing(self.s.fw_limits.as_ref())
            .map_err(|w| format!("Calibration incomplete — drawing disabled: {}", w.join("; ")))?;
        verify_program(spec.lines.iter().map(|l| l.text.as_str()), &resolved, &self.cfg.limits).map_err(|v| format!("Program rejected by safety check: {v}"))?;
        // starting Z must not already be on the lowered side of clearance with unknown XY: we require verified axes above.
        let total_strokes = spec.lines.iter().filter_map(|l| l.stroke).max().map_or(0, |m| m + 1);
        self.log("info", format!("Job '{}' started: {} lines, {} strokes", spec.name, spec.lines.len(), total_strokes));
        self.job = Some(Job {
            cal: resolved,
            state: JobState::Running,
            cursor: 0,
            accepted: 0,
            confirmed_strokes: 0,
            started: Instant::now(),
            ended: None,
            message: "Running".into(),
            error: None,
            warnings: Vec::new(),
            final_check: false,
            cancel_sent: false,
            total_strokes,
            current_stroke: None,
            spec,
        });
        self.s.busy = Busy::Job;
        self.sync_due = None;
        self.set_activity("Drawing");
        Ok(())
    }
}

#[cfg(test)]
mod tests;
