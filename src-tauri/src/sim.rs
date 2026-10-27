//! Simulated Marlin controller (behaves like Marlin 1.1.x over a serial line).
//!
//! It models what matters for safety testing:
//!  * `ok` is sent when a command has been *queued* (planner depth 16), not when it finished,
//!    so M400 is the only proof of completion — exactly like the real firmware.
//!  * After a reset the firmware's logical position is 0,0,0 while the carriage stays where it
//!    physically is, so coordinates become wrong until homing. (Marlin does the same.)
//!  * Software endstops clamp targets silently.
//!  * A hidden "truth" (where the paper really is, where the pencil really touches) lets the sim
//!    detect and report simulated collisions caused by a wrong calibration.
//!  * Fault injection: disconnect, reset, halt, stalled acks, error replies, lost steps.
//! No acceleration is modelled (constant velocity), and it never touches real hardware.

use crate::transport::Transport;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PLANNER_DEPTH: usize = 16;
const CMD_QUEUE: usize = 4;
const TRAIL_CAP: usize = 300_000;

/// Hidden physical truth of the simulated machine (machine coordinates, mm).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SimTruth {
    pub bed: [f64; 3],
    /// Pencil tip − nozzle.
    pub pencil_offset: [f64; 2],
    /// Machine Z where the pencil tip touches the paper.
    pub paper_contact_z: f64,
    /// Paper rectangle in tip machine coordinates: [x0, y0, x1, y1].
    pub paper_rect: [f64; 4],
    /// Where the carriage physically starts (the sim begins with logical == physical).
    pub start: [f64; 3],
    /// Does the simulated firmware handle M112/M410 out-of-band (Marlin EMERGENCY_PARSER)?
    pub emergency_parser: bool,
    pub firmware: String,
    pub machine_type: String,
}

impl Default for SimTruth {
    fn default() -> Self {
        SimTruth {
            bed: [310.0, 310.0, 402.0],
            pencil_offset: [25.0, -10.0],
            paper_contact_z: 12.0,
            paper_rect: [40.0, 2.0, 250.0, 299.0],
            start: [0.0, 0.0, 60.0],
            emergency_parser: false,
            firmware: "Marlin Ver 1.60.9 (Creality3D)".into(),
            machine_type: "CR-10S Pro (SIMULATED)".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Fault {
    /// Unplug: the link disappears.
    Disconnect,
    /// MCU reset: logical position forgotten, planner flushed, banner reprinted.
    Reset,
    /// Firmware kill(): halted until reset.
    Halt,
    /// Stop sending anything for this many wall-clock seconds.
    Stall { seconds: f64 },
    /// The next command is answered with an Error: line.
    ErrorNext,
    /// Lose steps on an axis (physical position drifts from logical).
    SkipSteps { axis: usize, mm: f64 },
}

enum Ctl {
    Fault(Fault),
    Speed(f64),
    ClearTrail,
    Truth(SimTruth),
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Collision {
    pub at_ms: u64,
    pub message: String,
}

#[derive(Default)]
struct Shared {
    phys: [f64; 3],
    logical: [f64; 3],
    homed: [bool; 3],
    pen_down: bool,
    planner_depth: usize,
    halted: bool,
    stalled: bool,
    speed: f64,
    truth: SimTruth,
    trail: Vec<[f32; 4]>,
    trail_dropped: usize,
    collisions: Vec<Collision>,
    commands_seen: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SimSnapshot {
    pub phys: [f64; 3],
    pub logical: [f64; 3],
    pub tip: [f64; 2],
    pub homed: [bool; 3],
    pub pen_down: bool,
    pub planner_depth: usize,
    pub halted: bool,
    pub stalled: bool,
    pub speed: f64,
    pub truth: SimTruth,
    pub trail_len: usize,
    pub collisions: Vec<Collision>,
    pub commands_seen: u64,
}

#[derive(Clone)]
pub struct SimHandle {
    shared: Arc<Mutex<Shared>>,
    ctl: Sender<Ctl>,
}

impl SimHandle {
    pub fn fault(&self, f: Fault) {
        let _ = self.ctl.send(Ctl::Fault(f));
    }
    pub fn set_speed(&self, s: f64) {
        let _ = self.ctl.send(Ctl::Speed(s.clamp(0.1, 5000.0)));
    }
    pub fn clear_trail(&self) {
        let _ = self.ctl.send(Ctl::ClearTrail);
    }
    pub fn set_truth(&self, t: SimTruth) {
        let _ = self.ctl.send(Ctl::Truth(t));
    }
    pub fn snapshot(&self) -> SimSnapshot {
        let s = self.shared.lock().unwrap();
        SimSnapshot {
            phys: s.phys,
            logical: s.logical,
            tip: [s.phys[0] + s.truth.pencil_offset[0], s.phys[1] + s.truth.pencil_offset[1]],
            homed: s.homed,
            pen_down: s.pen_down,
            planner_depth: s.planner_depth,
            halted: s.halted,
            stalled: s.stalled,
            speed: s.speed,
            truth: s.truth.clone(),
            trail_len: s.trail_dropped + s.trail.len(),
            collisions: s.collisions.clone(),
            commands_seen: s.commands_seen,
        }
    }
    /// Trail segments (tip machine coords x0,y0,x1,y1) from absolute index `from`.
    pub fn trail_since(&self, from: usize) -> (usize, Vec<[f32; 4]>) {
        let s = self.shared.lock().unwrap();
        let total = s.trail_dropped + s.trail.len();
        let start = from.saturating_sub(s.trail_dropped).min(s.trail.len());
        (total, s.trail[start..].to_vec())
    }
}

pub struct SimTransport {
    tx: Option<Sender<String>>,
    rx: Receiver<String>,
    closed: Arc<AtomicBool>,
    pub handle: SimHandle,
}

impl SimTransport {
    pub fn new(truth: SimTruth, speed: f64) -> SimTransport {
        let (tx_cmd, rx_cmd) = mpsc::channel::<String>();
        let (tx_resp, rx_resp) = mpsc::channel::<String>();
        let (tx_ctl, rx_ctl) = mpsc::channel::<Ctl>();
        let shared = Arc::new(Mutex::new(Shared {
            phys: truth.start,
            logical: truth.start,
            homed: [true; 3],
            speed,
            truth,
            ..Default::default()
        }));
        let closed = Arc::new(AtomicBool::new(false));
        let (sh, cl) = (shared.clone(), closed.clone());
        thread::Builder::new()
            .name("sim-marlin".into())
            .spawn(move || sim_main(sh, rx_cmd, tx_resp, rx_ctl, cl))
            .expect("spawn sim");
        SimTransport { tx: Some(tx_cmd), rx: rx_resp, closed, handle: SimHandle { shared, ctl: tx_ctl } }
    }
}

impl Transport for SimTransport {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "simulated device disconnected"));
        }
        self.tx
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "closed"))?
            .send(line.to_string())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "simulated device disconnected"))
    }
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        match self.rx.recv_timeout(timeout) {
            Ok(l) => Ok(Some(l)),
            Err(RecvTimeoutError::Timeout) => {
                if self.closed.load(Ordering::SeqCst) {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "simulated device disconnected"))
                } else {
                    Ok(None)
                }
            }
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "simulated device disconnected")),
        }
    }
    fn close(&mut self) {
        self.tx = None;
        self.closed.store(true, Ordering::SeqCst);
    }
    fn describe(&self) -> String {
        "Simulated Marlin".into()
    }
}

struct Move {
    from: [f64; 3],
    to: [f64; 3],
    total: f64,
    done: f64,
    speed: f64, // mm/s along the 3D path
}

enum Block {
    None,
    Idle,
    Dwell(Instant),
    Home { axes: [bool; 3], started: bool },
}

struct Sim {
    sh: Arc<Mutex<Shared>>,
    tx: Sender<String>,
    incoming: VecDeque<String>,
    planner: VecDeque<Move>,
    block: Block,
    /// Logical position after the last queued move.
    planned: [f64; 3],
    feed: f64, // mm/min
    shift: [f64; 3], // phys = logical + shift
    homed: [bool; 3],
    halted: bool,
    stall_until: Option<Instant>,
    error_next: bool,
    last_busy: Instant,
    pen_event_active: bool,
    last_trail: Option<[f64; 2]>,
    collision_cool: bool,
    t0: Instant,
}

fn s_start(sh: &Arc<Mutex<Shared>>) -> [f64; 3] {
    sh.lock().unwrap().truth.start
}

fn sim_main(sh: Arc<Mutex<Shared>>, rx: Receiver<String>, tx: Sender<String>, ctl: Receiver<Ctl>, closed: Arc<AtomicBool>) {
    let start = s_start(&sh);
    let mut s = Sim {
        sh,
        tx,
        incoming: VecDeque::new(),
        planner: VecDeque::new(),
        block: Block::None,
        planned: start,
        feed: 1500.0,
        shift: [0.0; 3],
        homed: [true; 3],
        halted: false,
        stall_until: None,
        error_next: false,
        last_busy: Instant::now(),
        pen_event_active: false,
        last_trail: None,
        collision_cool: false,
        t0: Instant::now(),
    };
    s.banner(true);
    let mut last = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(2));
        let now = Instant::now();
        let speed = s.sh.lock().unwrap().speed;
        let dt = now.duration_since(last).as_secs_f64() * speed;
        last = now;

        while let Ok(c) = ctl.try_recv() {
            match c {
                Ctl::Speed(v) => s.sh.lock().unwrap().speed = v,
                Ctl::ClearTrail => {
                    let mut g = s.sh.lock().unwrap();
                    g.trail.clear();
                    g.trail_dropped = 0;
                    g.collisions.clear();
                    s.last_trail = None;
                }
                Ctl::Truth(t) => s.sh.lock().unwrap().truth = t,
                Ctl::Fault(Fault::Disconnect) => {
                    closed.store(true, Ordering::SeqCst);
                    return;
                }
                Ctl::Fault(Fault::Reset) => s.reset(),
                Ctl::Fault(Fault::Halt) => s.halt("Printer halted. kill() called!"),
                Ctl::Fault(Fault::Stall { seconds }) => s.stall_until = Some(now + Duration::from_secs_f64(seconds)),
                Ctl::Fault(Fault::ErrorNext) => s.error_next = true,
                Ctl::Fault(Fault::SkipSteps { axis, mm }) => {
                    if axis < 3 {
                        s.shift[axis] += mm;
                        let mut g = s.sh.lock().unwrap();
                        g.phys[axis] += mm;
                    }
                }
            }
        }
        // host closed the link
        loop {
            match rx.try_recv() {
                Ok(l) => {
                    let code = command_code(&l);
                    let oob = s.sh.lock().unwrap().truth.emergency_parser && matches!(code.as_str(), "M112" | "M410");
                    if oob {
                        s.execute(&l, &code, now);
                    } else {
                        s.incoming.push_back(l);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if s.incoming.is_empty() {
                        closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    break;
                }
            }
        }
        let stalled = s.stall_until.map_or(false, |u| now < u);
        if !stalled {
            s.stall_until = None;
        }
        s.sh.lock().unwrap().stalled = stalled;
        if stalled || s.halted {
            // frozen firmware: no motion progress is reported, no acks
            if s.halted {
                s.sync_shared();
            }
            continue;
        }
        s.advance(dt, now);
        s.process(now);
        s.sync_shared();
    }
}

impl Sim {
    fn say(&self, line: impl Into<String>) {
        let _ = self.tx.send(line.into());
    }

    fn banner(&self, with_start: bool) {
        if with_start {
            self.say("start");
        }
        let t = self.sh.lock().unwrap().truth.clone();
        self.say(format!("echo:{}", t.firmware));
        self.say("echo: Last Updated: 2019-04-16 | Author: (Creality, CR-10S Pro)");
    }

    fn reset(&mut self) {
        self.planner.clear();
        self.incoming.clear();
        self.block = Block::None;
        self.homed = [false; 3];
        let phys = self.sh.lock().unwrap().phys;
        self.planned = [0.0; 3];
        self.shift = phys; // physical carriage stays put, logical coordinates restart at 0
        self.halted = false;
        self.feed = 1500.0;
        self.banner(true);
    }

    fn halt(&mut self, msg: &str) {
        self.halted = true;
        self.planner.clear();
        self.incoming.clear();
        self.say(format!("Error:{msg}"));
        self.say("echo:Printer halted. Please restart (reset) the printer.");
    }

    fn clamp_logical(&self, mut p: [f64; 3]) -> [f64; 3] {
        let bed = self.sh.lock().unwrap().truth.bed;
        for i in 0..3 {
            p[i] = p[i].clamp(0.0, bed[i]);
        }
        p
    }

    fn advance(&mut self, mut dt: f64, now: Instant) {
        // moves
        while dt > 0.0 {
            let Some(m) = self.planner.front_mut() else { break };
            let remain = (m.total - m.done) / m.speed;
            let step = dt.min(remain).min(0.4 / m.speed);
            let d0 = m.done;
            m.done += step * m.speed;
            dt -= step;
            let frac = |d: f64| if m.total > 0.0 { d / m.total } else { 1.0 };
            let (p0, p1) = (lerp(m.from, m.to, frac(d0)), lerp(m.from, m.to, frac(m.done)));
            let done = m.done >= m.total - 1e-9;
            self.apply_motion(p0, p1);
            if done {
                self.planner.pop_front();
            }
        }
        // blocking command progress
        let blk = std::mem::replace(&mut self.block, Block::None);
        self.block = match blk {
            Block::Idle if self.planner.is_empty() => {
                self.say("ok");
                Block::None
            }
            Block::Dwell(t) if self.planner.is_empty() && now >= t => {
                self.say("ok");
                Block::None
            }
            Block::Home { axes, started } if self.planner.is_empty() => {
                if started {
                    self.finish_home(axes);
                    Block::None
                } else {
                    // homing = physical move to 0 on the chosen axes (Z slowly)
                    let phys = self.sh.lock().unwrap().phys;
                    let mut to = phys;
                    for i in 0..3 {
                        if axes[i] {
                            to[i] = 0.0;
                        }
                    }
                    let len = dist3(phys, to);
                    if len < 1e-6 {
                        self.finish_home(axes);
                        Block::None
                    } else {
                        let speed = if axes[2] && !axes[0] && !axes[1] { 4.0 } else { 20.0 };
                        self.planner.push_back(Move { from: sub(phys, self.shift), to: sub(to, self.shift), total: len, done: 0.0, speed });
                        Block::Home { axes, started: true }
                    }
                }
            }
            other => other,
        };
        let busy = !self.planner.is_empty() || !matches!(self.block, Block::None);
        if busy && now.duration_since(self.last_busy) > Duration::from_secs(2) {
            self.last_busy = now;
            self.say("echo:busy: processing");
        }
        let _ = self.t0;
    }

    fn finish_home(&mut self, axes: [bool; 3]) {
        for i in 0..3 {
            if axes[i] {
                self.shift[i] = 0.0;
                self.homed[i] = true;
                self.planned[i] = 0.0;
            }
        }
        self.say("ok");
    }

    /// Physical consequences of moving the carriage from `a` to `b` (both in LOGICAL coords).
    fn apply_motion(&mut self, a: [f64; 3], b: [f64; 3]) {
        let phys_b = add(b, self.shift);
        let mut g = self.sh.lock().unwrap();
        let _ = a;
        g.phys = phys_b;
        g.logical = b;
        let t = g.truth.clone();
        let tip = [phys_b[0] + t.pencil_offset[0], phys_b[1] + t.pencil_offset[1]];
        let down = phys_b[2] <= t.paper_contact_z + 0.05;
        g.pen_down = down;
        let on_paper = tip[0] >= t.paper_rect[0] && tip[0] <= t.paper_rect[2] && tip[1] >= t.paper_rect[1] && tip[1] <= t.paper_rect[3];
        let now_ms = self.t0.elapsed().as_millis() as u64;
        let mut hit: Option<String> = None;
        if phys_b[2] < t.paper_contact_z - 0.3 && on_paper {
            hit = Some(format!(
                "Pencil driven {:.1} mm into the paper (Z{:.2}, contact at Z{:.2}) — calibration/Z mismatch",
                t.paper_contact_z - phys_b[2],
                phys_b[2],
                t.paper_contact_z
            ));
        } else if down && !on_paper {
            hit = Some(format!("Pencil touched the bed OUTSIDE the paper at tip X{:.1} Y{:.1}", tip[0], tip[1]));
        } else if phys_b[2] < t.paper_contact_z - 0.3 {
            hit = Some(format!("Head driven below the surface at Z{:.2} outside the paper", phys_b[2]));
        }
        match hit {
            Some(msg) if !self.collision_cool => {
                self.collision_cool = true;
                g.collisions.push(Collision { at_ms: now_ms, message: msg });
            }
            None => self.collision_cool = false,
            _ => {}
        }
        if down {
            let seg_ok = self.last_trail.map_or(true, |p| ((tip[0] - p[0]).powi(2) + (tip[1] - p[1]).powi(2)).sqrt() > 0.05);
            if seg_ok {
                let from = if self.pen_event_active { self.last_trail.unwrap_or(tip) } else { tip };
                g.trail.push([from[0] as f32, from[1] as f32, tip[0] as f32, tip[1] as f32]);
                if g.trail.len() > TRAIL_CAP {
                    let drop = TRAIL_CAP / 10;
                    g.trail.drain(..drop);
                    g.trail_dropped += drop;
                }
                self.last_trail = Some(tip);
            }
            self.pen_event_active = true;
        } else {
            self.pen_event_active = false;
            self.last_trail = None;
        }
    }

    fn sync_shared(&mut self) {
        let mut g = self.sh.lock().unwrap();
        g.planner_depth = self.planner.len();
        g.halted = self.halted;
        g.homed = self.homed;
    }

    fn process(&mut self, now: Instant) {
        // Marlin only reads into a 4-slot command queue; keep behaviour: commands wait their turn.
        let mut guard = 0;
        while !self.halted && guard < 64 {
            guard += 1;
            if !matches!(self.block, Block::None) {
                break;
            }
            let Some(raw) = self.incoming.front().cloned() else { break };
            // queue backpressure: a motion command needs room in the planner
            let code = command_code(&raw);
            let is_move = matches!(code.as_str(), "G0" | "G1");
            if is_move && self.planner.len() >= PLANNER_DEPTH {
                break;
            }
            self.incoming.pop_front();
            self.sh.lock().unwrap().commands_seen += 1;
            self.execute(&raw, &code, now);
        }
        let _ = CMD_QUEUE;
    }

    fn execute(&mut self, raw: &str, code: &str, now: Instant) {
        let line = raw.split(';').next().unwrap_or("").trim().to_ascii_uppercase();
        if line.is_empty() {
            self.say("ok");
            return;
        }
        if self.error_next && !matches!(code, "M112" | "M114" | "M115") {
            self.error_next = false;
            self.say("Error:Simulated firmware error (fault injection)");
            self.say("ok");
            return;
        }
        let arg = |c: char| -> Option<f64> {
            line.split_whitespace().skip(1).find(|t| t.starts_with(c)).and_then(|t| t[1..].parse().ok())
        };
        let has = |c: char| line.split_whitespace().skip(1).any(|t| t.starts_with(c));
        match code {
            "G0" | "G1" => {
                if let Some(f) = arg('F') {
                    self.feed = f.max(1.0);
                }
                let mut tgt = self.planned;
                for (i, c) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if let Some(v) = arg(*c) {
                        tgt[i] = v;
                    }
                }
                let tgt = self.clamp_logical(tgt);
                let len = dist3(self.planned, tgt);
                // Marlin's max feedrates: XY 200 mm/s, Z 8 mm/s
                let mut v = (self.feed / 60.0).min(200.0);
                let dz = (tgt[2] - self.planned[2]).abs();
                if len > 0.0 && dz > 0.0 {
                    v = v.min(8.0 * len / dz);
                }
                if len > 1e-9 {
                    self.planner.push_back(Move { from: self.planned, to: tgt, total: len, done: 0.0, speed: v });
                }
                self.planned = tgt;
                self.say("ok");
            }
            "G4" => {
                let ms = arg('P').unwrap_or(0.0) + arg('S').unwrap_or(0.0) * 1000.0;
                self.block = Block::Dwell(now + Duration::from_secs_f64(ms / 1000.0 / self.sh.lock().unwrap().speed));
            }
            "G21" | "G90" | "M17" | "M82" => self.say("ok"),
            "M220" => {
                self.say("ok");
            }
            "G28" => {
                let mut axes = [has('X'), has('Y'), has('Z')];
                if !axes.iter().any(|a| *a) {
                    axes = [true; 3];
                }
                self.block = Block::Home { axes, started: false };
            }
            "M400" => self.block = Block::Idle,
            "M410" => {
                // quickstop: drop the planner, stop where we are
                self.planner.clear();
                let g = self.sh.lock().unwrap();
                let logical = sub(g.phys, self.shift);
                drop(g);
                self.planned = logical;
                self.say("ok");
            }
            "M112" => {
                self.halt("Printer halted. kill() called!");
            }
            "M114" => {
                let p = self.planned;
                self.say(format!(
                    "X:{:.2} Y:{:.2} Z:{:.2} E:0.00 Count X: {} Y:{} Z:{}",
                    p[0], p[1], p[2], (p[0] * 80.0) as i64, (p[1] * 80.0) as i64, (p[2] * 400.0) as i64
                ));
                self.say("ok");
            }
            "M115" => {
                let t = self.sh.lock().unwrap().truth.clone();
                self.say(format!(
                    "FIRMWARE_NAME:{} SOURCE_CODE_URL:https://github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:{} EXTRUDER_COUNT:1 UUID:cede2a2f-41a2-4748-9b12-c55c62f367ff",
                    t.firmware, t.machine_type
                ));
                self.say(format!("Cap:EMERGENCY_PARSER:{}", t.emergency_parser as u8));
                self.say("Cap:AUTOREPORT_TEMP:0");
                self.say("ok");
            }
            "M211" => {
                let b = self.sh.lock().unwrap().truth.bed;
                self.say(format!("echo:Soft endstops: ON  Min: X0.00 Y0.00 Z0.00  Max: X{:.2} Y{:.2} Z{:.2}", b[0], b[1], b[2]));
                self.say("ok");
            }
            _ => {
                self.say(format!("echo:Unknown command: \"{}\"", line.split_whitespace().next().unwrap_or("")));
                self.say("ok");
            }
        }
    }

}

fn command_code(raw: &str) -> String {
    let l = raw.split(';').next().unwrap_or("").trim().to_ascii_uppercase();
    let t = l.split_whitespace().next().unwrap_or("").to_string();
    // normalise leading zeros: G01 → G1
    if t.len() > 1 {
        let (c, n) = t.split_at(1);
        if let Ok(n) = n.parse::<u32>() {
            return format!("{c}{n}");
        }
    }
    t
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
