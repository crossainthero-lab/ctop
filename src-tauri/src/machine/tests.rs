//! End-to-end tests of the machine actor against the simulated Marlin.
//! No real serial device is ever opened here.

use super::*;
use crate::calibration::sample_calibration;
use crate::gcode::{generate, GcodeSettings};
use crate::planner::Stroke;
use crate::sim::{Fault, SimHandle, SimTransport, SimTruth};

fn truth() -> SimTruth {
    // matches sample_calibration(): offset (-30,10), pen touches at Z5, paper at tip origin (60,300)
    SimTruth { pencil_offset: [-30.0, 10.0], paper_contact_z: 5.0, paper_rect: [60.0, 3.0, 270.0, 300.0], start: [100.0, 100.0, 60.0], emergency_parser: true, ..Default::default() }
}

fn fast_cfg() -> ActorConfig {
    ActorConfig {
        ack_timeout: Duration::from_secs(8),
        home_timeout: Duration::from_secs(20),
        banner_wait: Duration::from_millis(800),
        handshake_timeout: Duration::from_secs(5),
        heartbeat: Duration::from_secs(3600),
        sync_quiet: Duration::from_millis(60),
        ..ActorConfig::default()
    }
}

struct Rig {
    h: MachineHandle,
    sim: SimHandle,
    cal: Arc<Mutex<Calibration>>,
}

fn rig_with(truth: SimTruth, cal: Calibration, speed: f64, cfg: ActorConfig) -> Rig {
    let t = SimTransport::new(truth, speed);
    let sim = t.handle.clone();
    let cal = Arc::new(Mutex::new(cal));
    let sink: EventSink = Arc::new(|_| {});
    let h = spawn(Box::new(t), Mode::Simulation, 1, cal.clone(), cfg, sink);
    let r = Rig { h, sim, cal };
    r.wait(|s| s.link == Link::Ready, 5000, "link ready");
    r
}

fn rig() -> Rig {
    rig_with(truth(), sample_calibration(), 800.0, fast_cfg())
}

impl Rig {
    fn wait(&self, f: impl Fn(&Snapshot) -> bool, ms: u64, what: &str) -> Snapshot {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            let s = self.h.snapshot();
            if f(&s) {
                return s;
            }
            if Instant::now() > end {
                panic!("timeout waiting for {what}; snapshot: {:?}", s);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn arm_and_verify(&self) {
        self.h.request(Cmd::Arm { mode: ArmMode::Envelope, acknowledged: false }).unwrap();
        self.h.request(Cmd::Sync).unwrap();
        self.wait(|s| s.position.is_some() && s.busy == Busy::Idle, 3000, "position read");
        self.h.request(Cmd::VerifyPosition).unwrap();
    }
    fn job(&self, strokes: Vec<Stroke>) -> JobSpec {
        let cal = self.cal.lock().unwrap().clone();
        let res = cal.resolve().unwrap();
        let prog = generate(&strokes, &res, &GcodeSettings::default(), &Limits::default()).unwrap();
        JobSpec { id: "t1".into(), name: "test".into(), lines: prog.lines, estimate_s: prog.stats.estimated_seconds, calibration_key: serde_json::to_string(&cal).unwrap() }
    }
}

fn circle(cx: f64, cy: f64, r: f64, n: usize) -> Stroke {
    (0..=n).map(|i| { let a = i as f64 / n as f64 * std::f64::consts::TAU; [cx + r * a.cos(), cy + r * a.sin()] }).collect()
}

fn square(x: f64, y: f64, s: f64) -> Stroke {
    vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]]
}

#[test]
fn connect_identifies_firmware_and_moves_nothing() {
    let r = rig();
    let s = r.h.snapshot();
    assert!(s.firmware.as_ref().unwrap().is_marlin);
    assert_eq!(s.fw_limits.unwrap().x_max, 310.0);
    assert_eq!(s.axes_known, [false; 3], "position unknown after connect");
    assert!(s.armed.is_none());
    let sim = r.sim.snapshot();
    assert_eq!(sim.phys, [100.0, 100.0, 60.0], "carriage did not move on connect");
    assert!(sim.trail_len == 0);
}

#[test]
fn motion_requires_arming_and_verified_position() {
    let r = rig();
    let e = r.h.request(Cmd::Goto { x: Some(110.0), y: None, z: None, feed: 3000.0 }).unwrap_err();
    assert!(e.contains("not armed"), "{e}");
    r.h.request(Cmd::Arm { mode: ArmMode::Envelope, acknowledged: false }).unwrap();
    let e = r.h.request(Cmd::Goto { x: Some(110.0), y: None, z: None, feed: 3000.0 }).unwrap_err();
    assert!(e.contains("not verified"), "{e}");
    assert_eq!(r.sim.snapshot().phys, [100.0, 100.0, 60.0]);
}

#[test]
fn jog_moves_sim_and_position_is_confirmed_by_firmware() {
    let r = rig();
    r.arm_and_verify();
    r.h.request(Cmd::Goto { x: Some(120.0), y: Some(100.0), z: None, feed: 3000.0 }).unwrap();
    let s = r.wait(|s| s.position_source.contains("firmware") && s.busy == Busy::Idle, 4000, "sync after jog");
    assert_eq!(s.position.unwrap(), [120.0, 100.0, 60.0]);
    assert_eq!(r.sim.snapshot().phys, [120.0, 100.0, 60.0]);
}

#[test]
fn jog_outside_envelope_is_refused_before_sending() {
    let r = rig();
    r.arm_and_verify();
    let before = r.sim.snapshot().commands_seen;
    let e = r.h.request(Cmd::Goto { x: Some(400.0), y: None, z: None, feed: 3000.0 }).unwrap_err();
    assert!(e.contains("envelope"), "{e}");
    // below pen-down height
    let e = r.h.request(Cmd::Goto { x: None, y: None, z: Some(2.0), feed: 600.0 }).unwrap_err();
    assert!(e.contains("envelope") || e.contains("below"), "{e}");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(r.sim.snapshot().commands_seen, before, "nothing reached the firmware");
}

#[test]
fn jog_feed_is_capped() {
    let r = rig();
    r.arm_and_verify();
    assert!(r.h.request(Cmd::Goto { x: Some(110.0), y: None, z: None, feed: 50_000.0 }).is_err());
}

#[test]
fn firmware_only_arming_needs_acknowledgement_and_limits_z_down() {
    let r = rig_with(truth(), Calibration::default(), 800.0, fast_cfg());
    assert!(r.h.request(Cmd::Arm { mode: ArmMode::Envelope, acknowledged: true }).is_err(), "no envelope defined");
    assert!(r.h.request(Cmd::Arm { mode: ArmMode::FirmwareOnly, acknowledged: false }).is_err());
    r.h.request(Cmd::Arm { mode: ArmMode::FirmwareOnly, acknowledged: true }).unwrap();
    r.h.request(Cmd::Sync).unwrap();
    r.wait(|s| s.position.is_some() && s.busy == Busy::Idle, 3000, "pos");
    r.h.request(Cmd::VerifyPosition).unwrap();
    assert!(r.h.request(Cmd::Goto { x: None, y: None, z: Some(40.0), feed: 600.0 }).is_err(), "20 mm down step refused");
    r.h.request(Cmd::Goto { x: None, y: None, z: Some(59.5), feed: 600.0 }).unwrap();
}

#[test]
fn drawing_job_completes_and_completion_is_confirmed_not_assumed() {
    let r = rig();
    r.arm_and_verify();
    let spec = r.job(vec![square(40.0, 40.0, 20.0), square(100.0, 100.0, 15.0)]);
    let total_lines = spec.lines.len();
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.state == JobState::Completed), 20_000, "job completion");
    let j = s.job.unwrap();
    assert_eq!(j.accepted, total_lines);
    assert_eq!(j.confirmed_strokes, 2);
    assert_eq!(j.total_strokes, 2);
    assert!(j.error.is_none() && j.warnings.is_empty(), "{:?}", j);
    let sim = r.sim.snapshot();
    assert!(sim.collisions.is_empty(), "{:?}", sim.collisions);
    {
        let (_, tr) = r.sim.trail_since(0);
        let len: f64 = tr.iter().map(|s| (((s[2] - s[0]) as f64).powi(2) + ((s[3] - s[1]) as f64).powi(2)).sqrt()).sum();
        assert!((len - 140.0).abs() < 12.0, "drawn length ≈ 80+60 mm, got {len}");
    }
    assert!(!sim.pen_down, "pen ends up");
    assert_eq!(sim.phys[2], 20.0, "parked at safe clearance");
    // the trail must land on the paper where the paper-space strokes were (tip coords)
    let (n, trail) = r.sim.trail_since(0);
    assert_eq!(n, trail.len());
    let c = sample_calibration().resolve().unwrap();
    let (tx, ty) = c.paper_to_tip(40.0, 40.0);
    assert!(trail.iter().any(|t| ((t[0] as f64) - tx).abs() < 0.2 && ((t[1] as f64) - ty).abs() < 0.2));
}

#[test]
fn accepted_runs_ahead_of_confirmed_while_drawing() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.accepted > 6), 5000, "accepted lines");
    let j = s.job.unwrap();
    assert!(j.confirmed_strokes < j.total_strokes, "acks arrive before motion finishes");
    assert!(j.state == JobState::Running);
    r.h.request(Cmd::CancelJob).unwrap();
}

#[test]
fn job_refused_without_confirmation_or_verified_calibration() {
    let r = rig();
    r.arm_and_verify();
    let spec = r.job(vec![square(40.0, 40.0, 10.0)]);
    assert!(r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: false }).is_err());
    // un-verify one calibration item → drawing must be disabled
    let spec = r.job(vec![square(40.0, 40.0, 10.0)]);
    r.cal.lock().unwrap().verified.z = false;
    let spec2 = JobSpec { calibration_key: serde_json::to_string(&*r.cal.lock().unwrap()).unwrap(), ..spec };
    let e = r.h.request(Cmd::StartJob { spec: Box::new(spec2), paper_confirmed: true }).unwrap_err();
    assert!(e.contains("Calibration incomplete"), "{e}");
}

#[test]
fn stale_calibration_job_is_refused() {
    let r = rig();
    r.arm_and_verify();
    let spec = r.job(vec![square(40.0, 40.0, 10.0)]);
    r.cal.lock().unwrap().pencil_offset_x = Some(-31.0);
    let e = r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap_err();
    assert!(e.contains("Calibration changed"), "{e}");
}

#[test]
fn wrong_physical_z_is_caught_as_simulated_collision() {
    // calibration says pen touches at Z5 but the paper is really at Z8: the pencil would not mark,
    // and if the paper were at Z3 the pencil would be driven into it. Test the second case.
    let mut t = truth();
    t.paper_contact_z = 8.0;
    let r = rig_with(t, sample_calibration(), 800.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![square(40.0, 40.0, 10.0)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| !j.state.is_active()), 20_000, "end");
    assert!(!r.sim.snapshot().collisions.is_empty(), "sim must report the pencil being driven into the paper");
}

#[test]
fn manual_motion_locked_during_job() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    let e = r.h.request(Cmd::Goto { x: Some(105.0), y: None, z: None, feed: 3000.0 }).unwrap_err();
    assert!(e.contains("job"), "{e}");
    assert!(r.h.request(Cmd::Home { axes: [true, true, false] }).is_err());
    assert!(r.h.request(Cmd::Disconnect).is_err(), "cannot walk away from a running job");
    r.h.request(Cmd::CancelJob).unwrap();
}

#[test]
fn pause_stops_submission_not_buffered_motion() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 8), 5000, "some lines sent");
    r.h.request(Cmd::Pause).unwrap();
    thread::sleep(Duration::from_millis(150));
    let a = r.h.snapshot().job.unwrap();
    assert_eq!(a.state, JobState::Paused);
    assert!(a.message.contains("NOT an instant stop"));
    thread::sleep(Duration::from_millis(300));
    let b = r.h.snapshot().job.unwrap();
    assert_eq!(a.sent, b.sent, "no new commands while paused");
    r.h.request(Cmd::Resume).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > a.sent), 3000, "resumed");
    r.h.request(Cmd::CancelJob).unwrap();
}

#[test]
fn cancel_discards_unsent_and_lifts_pencil() {
    let r = rig_with(truth(), sample_calibration(), 30.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    let total = spec.lines.len();
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.h.request(Cmd::CancelJob).unwrap();
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.state == JobState::Cancelled), 20_000, "cancelled");
    assert!(s.job.unwrap().sent < total, "unsent commands were discarded");
    let sim = r.sim.snapshot();
    assert!(!sim.pen_down);
    assert_eq!(sim.phys[2], 20.0);
}

#[test]
fn quick_stop_halts_job_and_invalidates_position() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.h.request(Cmd::QuickStop).unwrap();
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.state == JobState::Stopped), 3000, "stopped");
    assert_eq!(s.axes_known, [false; 3]);
    assert!(s.armed.is_none());
    let sent = s.job.as_ref().unwrap().sent;
    thread::sleep(Duration::from_millis(300));
    // M410 flushed the planner, but commands already in the firmware's 4-slot queue still ran
    assert!(r.sim.snapshot().planner_depth <= 4, "planner mostly flushed: {}", r.sim.snapshot().planner_depth);
    assert_eq!(r.h.snapshot().job.unwrap().sent, sent, "no further commands are sent after a stop");
    assert!(r.h.request(Cmd::Goto { x: Some(105.0), y: None, z: None, feed: 3000.0 }).is_err());
}

#[test]
fn emergency_halt_is_immediate_and_latches() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.h.request(Cmd::Emergency).unwrap();
    let s = r.wait(|s| s.link == Link::Halted, 2000, "halted");
    assert_eq!(s.job.as_ref().unwrap().state, JobState::Failed);
    assert!(s.attention.unwrap().contains("CUT PRINTER POWER"));
    r.wait(|_| r.sim.snapshot().halted, 2000, "sim saw M112");
    assert!(r.h.request(Cmd::Goto { x: Some(105.0), y: None, z: None, feed: 3000.0 }).is_err());
    assert!(r.h.request(Cmd::Arm { mode: ArmMode::Envelope, acknowledged: false }).is_err());
}

#[test]
fn disconnect_mid_job_interrupts_and_never_resumes() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.sim.fault(Fault::Disconnect);
    let s = r.wait(|s| s.link == Link::Lost, 3000, "lost");
    assert_eq!(s.job.as_ref().unwrap().state, JobState::Interrupted);
    assert_eq!(s.axes_known, [false; 3]);
    assert!(s.position.is_none(), "position invalidated");
    assert!(s.armed.is_none());
    let sent_then = s.job.as_ref().unwrap().sent;
    thread::sleep(Duration::from_millis(300));
    assert_eq!(r.h.snapshot().job.unwrap().sent, sent_then, "nothing more is sent");
    assert!(r.h.request(Cmd::Resume).is_err());
}

#[test]
fn controller_reset_invalidates_everything_and_fails_job() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.sim.fault(Fault::Reset);
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.state == JobState::Failed), 3000, "failed");
    assert_eq!(s.axes_known, [false; 3]);
    assert!(s.armed.is_none());
    assert!(s.job.unwrap().error.unwrap().contains("reset"));
}

#[test]
fn silent_controller_times_out_and_stops_sending() {
    let r = rig_with(truth(), sample_calibration(), 800.0, ActorConfig { ack_timeout: Duration::from_millis(600), ..fast_cfg() });
    r.arm_and_verify();
    r.sim.fault(Fault::Stall { seconds: 30.0 });
    thread::sleep(Duration::from_millis(50));
    let spec = r.job(vec![square(30.0, 30.0, 40.0)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    let s = r.wait(|s| s.link == Link::Lost, 5000, "timeout");
    assert!(s.last_error.unwrap().contains("No response"));
    assert_eq!(s.job.unwrap().state, JobState::Interrupted);
}

#[test]
fn firmware_error_fails_the_job() {
    let r = rig_with(truth(), sample_calibration(), 4.0, fast_cfg());
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 6), 5000, "drawing");
    r.sim.fault(Fault::ErrorNext);
    let s = r.wait(|s| s.job.as_ref().map_or(false, |j| j.state == JobState::Failed), 3000, "failed");
    assert!(s.job.unwrap().error.unwrap().contains("Firmware error"));
}

#[test]
fn halt_from_firmware_latches_halted_state() {
    let r = rig();
    r.arm_and_verify();
    r.sim.fault(Fault::Halt);
    let s = r.wait(|s| s.link == Link::Halted, 2000, "halted");
    assert!(s.armed.is_none());
    assert!(s.attention.unwrap().contains("reset"));
}

#[test]
fn simulated_reset_makes_logical_coordinates_wrong_and_homing_fixes_xy() {
    let r = rig();
    r.sim.fault(Fault::Reset);
    r.wait(|s| s.attention.as_deref().map_or(false, |a| a.contains("controller reset")), 2000, "reset seen");
    r.h.request(Cmd::Arm { mode: ArmMode::Envelope, acknowledged: false }).unwrap();
    r.h.request(Cmd::Sync).unwrap();
    let s = r.wait(|s| s.position.is_some() && s.busy == Busy::Idle, 3000, "pos");
    // firmware now believes 0,0,0 although the carriage is at 100,100,60
    assert_eq!(s.position.unwrap(), [0.0, 0.0, 0.0]);
    assert_eq!(r.sim.snapshot().phys, [100.0, 100.0, 60.0]);
    r.h.request(Cmd::Home { axes: [true, true, false] }).unwrap();
    r.wait(|s| s.axes_known[0] && s.axes_known[1] && s.busy == Busy::Idle, 10_000, "homed xy");
    assert_eq!(r.sim.snapshot().phys[2], 60.0, "Z was not touched by an XY home");
}

#[test]
fn disconnect_when_idle_works() {
    let r = rig();
    r.h.request(Cmd::Disconnect).unwrap();
    r.h.join();
    assert_eq!(r.h.snapshot().link, Link::Disconnected);
}

#[test]
fn quick_stop_without_emergency_parser_is_not_immediate_and_says_so() {
    let mut tr = truth();
    tr.emergency_parser = false;
    let r = rig_with(tr, sample_calibration(), 4.0, fast_cfg());
    assert_eq!(r.h.snapshot().firmware.unwrap().emergency_parser, Some(false));
    r.arm_and_verify();
    let spec = r.job(vec![circle(110.0, 150.0, 40.0, 300)]);
    r.h.request(Cmd::StartJob { spec: Box::new(spec), paper_confirmed: true }).unwrap();
    r.wait(|s| s.job.as_ref().map_or(false, |j| j.sent > 10), 5000, "drawing");
    r.h.request(Cmd::QuickStop).unwrap();
    let s = r.h.snapshot();
    assert!(s.attention.unwrap().contains("NO emergency parser"));
    // M410 is queued behind M400/moves: the planner is not flushed instantly
    thread::sleep(Duration::from_millis(30));
    assert!(r.sim.snapshot().planner_depth > 0);
}
