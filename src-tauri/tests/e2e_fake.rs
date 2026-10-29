//! End-to-end run of the app's controller backend against a FAKE controller (daemon4 on a pty board) on its own socket.
//! Needs CTOP_E2E_SOCK=<fake socket path>; skipped otherwise. Refuses the real socket.
//! Start the fake with: tools/fake_controller.py --sock /run/user/1000/ctop-fake.sock --speedup 15

use ctop_lib::ctl::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn sock() -> Option<String> {
    let s = std::env::var("CTOP_E2E_SOCK").ok()?;
    assert!(!s.ends_with("pend.sock"), "refusing to run the e2e test against the real controller");
    Some(s)
}

/// raw access for the *harness only* (fake controller): e.g. re-arming after the e-stop test
fn raw(sock: &str, line: &str) -> String {
    let mut s = UnixStream::connect(sock).unwrap();
    s.write_all(format!("{line}\n").as_bytes()).unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut b = String::new();
    s.read_to_string(&mut b).unwrap();
    b.trim().to_string()
}

fn text_like_strokes() -> Vec<Vec<[f64; 2]>> {
    // 30 short strokes, paper space (mm, y down): three rows of ten 8 mm "letters"
    let mut v = vec![];
    for row in 0..3 {
        for i in 0..10 {
            let (x, y) = (30.0 + i as f64 * 12.0, 70.0 + row as f64 * 15.0);
            v.push(vec![[x, y], [x + 4.0, y - 8.0], [x + 8.0, y]]);
        }
    }
    v
}

fn wait<F: Fn(&CtlSnapshot) -> bool>(ctl: &Arc<Ctl>, what: &str, secs: u64, f: F) -> CtlSnapshot {
    let t = Instant::now();
    loop {
        ctl.tick();
        let s = ctl.snapshot();
        if f(&s) {
            return s;
        }
        assert!(t.elapsed() < Duration::from_secs(secs), "timeout waiting for {what}: {:?}", s.run);
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn prep(ctl: &Arc<Ctl>, sheet: &str) -> Result<PreparedView, String> {
    ctl.prepare(PrepareReq { name: "e2e text".into(), strokes: text_like_strokes(), paper_w: 210.0, paper_h: 297.0, sheet_id: sheet.into(), origin_x: Some(40.0), origin_y: Some(50.0), depth: 0.5, feed: 1000.0 })
}

#[test]
fn full_flow_against_fake_controller() {
    let Some(sock) = sock() else {
        eprintln!("CTOP_E2E_SOCK not set: skipped");
        return;
    };
    let data = std::env::temp_dir().join(format!("ctop_e2e_{}", std::process::id()));
    // sheet calibration fixture (a SIMULATED calibration, copied from the worksheets sim run) in a private controller dir
    let root = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let ctl = Ctl::new(data, Some(sock.clone()), Some(root.clone()), Arc::new(|_, _| {}));
    ctl.set_config(Some(sock.clone()), Some(root));

    // 1. connect + status
    ctl.tick();
    let s = ctl.snapshot();
    assert!(s.reachable, "{:?}", s.error);
    let st = s.status.clone().unwrap();
    println!("status: pos={:?} box={:?} zfloor={:?} zup={:?} estop={} link_ok={}", st.pos, st.bbox, st.zfloor, st.zup, st.estop, st.link.as_ref().unwrap().ok);
    assert!(s.unsafe_reasons.is_empty(), "{:?}", s.unsafe_reasons);

    // 2. a job using the sheet calibration found in the controller's data dir (read-only) -> jobcheck verdict
    let sheets = list_sheets(&ctl.root());
    println!("sheets found: {:?}", sheets.iter().map(|s| (&s.id, &s.stage)).collect::<Vec<_>>());
    let p = prep(&ctl, "e2e").expect("prepare with e2e calibration");
    println!("prepared: strokes={} accepted={} verdict={:?}", p.strokes, p.accepted, p.verdict);
    assert!(p.accepted && p.verdict.is_some());
    assert_eq!(p.strokes, 30);

    // 3. refusal paths: not confirmed
    let e = ctl.start().unwrap_err();
    println!("start before confirming sheet -> {e}");
    assert!(e.contains("confirm"));
    // a job outside the box is refused by the controller's jobcheck (and so by us)
    let far = ctl.prepare(PrepareReq { name: "far".into(), strokes: vec![vec![[5.0, 5.0], [200.0, 5.0]]], paper_w: 210.0, paper_h: 297.0, sheet_id: "e2e".into(), origin_x: None, origin_y: None, depth: 0.5, feed: 1000.0 }).unwrap();
    println!("out-of-box job: accepted={} reason={:?}", far.accepted, far.reason);
    assert!(!far.accepted);
    assert!(ctl.start().unwrap_err().contains("rejected"));

    // 4. run -> progress -> pause -> resume -> done
    let p = prep(&ctl, "e2e").unwrap();
    assert!(p.accepted);
    ctl.confirm_sheet("e2e", true).unwrap();
    ctl.start().expect("start");
    let s = wait(&ctl, "some progress", 40, |s| s.run.as_ref().map(|r| r.done >= 4).unwrap_or(false));
    println!("progress seen: {:?}", s.run.as_ref().map(|r| (r.state, r.done, r.total)));
    ctl.pause().expect("pause");
    let s = wait(&ctl, "paused", 40, |s| s.run.as_ref().map(|r| r.state != RunState::Pausing && r.state != RunState::Running).unwrap_or(false));
    let r = s.run.unwrap();
    println!("after pause: state={:?} done={} total={}", r.state, r.done, r.total);
    assert_eq!(r.state, RunState::Paused);
    assert!(r.done >= 4 && r.done < 30, "paused mid-job");
    let done_at_pause = r.done;
    std::thread::sleep(Duration::from_millis(800));
    ctl.tick();
    assert_eq!(ctl.snapshot().run.unwrap().done, done_at_pause, "no progress while paused");
    ctl.resume().expect("resume");
    let s = wait(&ctl, "completed", 80, |s| s.run.as_ref().map(|r| !matches!(r.state, RunState::Running | RunState::Pausing)).unwrap_or(false));
    let r = s.run.unwrap();
    println!("after resume: state={:?} done={}/{} base={} segTotal={}", r.state, r.done, r.total, r.segment_base, r.segment_total);
    assert_eq!(r.state, RunState::Completed);
    assert_eq!((r.done, r.segment_base, r.segment_total), (30, done_at_pause, 30 - done_at_pause));

    // evidence from the controller's own event log that no stroke was repeated: slice sizes add up
    let log = raw(&sock, "!log 1000");
    let starts: Vec<usize> = log.match_indices("JOB START: ").map(|(i, _)| log[i + 11..].split(' ').next().unwrap().parse().unwrap()).collect();
    println!("controller JOB START sizes: {starts:?}");
    let n = starts.len();
    assert!(n >= 2 && starts[n - 2] == 30 && starts[n - 1] == 30 - done_at_pause);
    ctl.discard_run();

    // 5. stop
    let p = prep(&ctl, "e2e").unwrap();
    assert!(p.accepted);
    ctl.start().expect("start 2");
    wait(&ctl, "progress 2", 40, |s| s.run.as_ref().map(|r| r.done >= 2).unwrap_or(false));
    ctl.stop().expect("stop");
    let s = wait(&ctl, "stopped", 40, |s| s.run.as_ref().map(|r| !matches!(r.state, RunState::Running | RunState::Stopping)).unwrap_or(false));
    let r = s.run.unwrap();
    println!("after stop: state={:?} done={}", r.state, r.done);
    assert_eq!(r.state, RunState::Stopped);
    assert!(!r.resumable);
    ctl.discard_run();

    // 6. manual moves via the controller (pen is up after a job)
    // after a job the pen sits a few mm above the paper but BELOW the controller's pen-up height: sideways jogs are refused
    let e = ctl.jog_xy(2.0, 0.0).unwrap_err();
    println!("jog right after a job -> {e}");
    assert!(e.contains("raise the pen"));
    ctl.pen_up().expect("pen up");
    let before = ctl.snapshot().status.unwrap().pos.unwrap();
    let r = ctl.jog_xy(2.0, 0.0).expect("jog");
    println!("jog reply: {}", r.lines().last().unwrap());
    let after = ctl.snapshot().status.unwrap().pos.unwrap();
    assert!((after[0] - before[0] - 2.0).abs() < 0.05);
    assert!(ctl.jog_xy(0.0, 400.0).is_err());
    ctl.pen_up().expect("pen up");

    // 7. e-stop during a job
    let p = prep(&ctl, "e2e").unwrap();
    assert!(p.accepted);
    ctl.confirm_sheet("e2e", true).unwrap();
    ctl.start().expect("start 3");
    wait(&ctl, "progress 3", 40, |s| s.run.as_ref().map(|r| r.done >= 1).unwrap_or(false));
    let r = ctl.estop().expect("estop");
    println!("estop reply: {r}");
    assert!(r.contains("ESTOP LATCHED"));
    let s = wait(&ctl, "estop visible", 20, |s| s.status.as_ref().map(|s| s.estop).unwrap_or(false));
    assert!(s.unsafe_reasons.iter().any(|x| x.contains("EMERGENCY")));
    assert_eq!(s.run.as_ref().unwrap().state, RunState::Failed);
    assert!(s.sheet_confirmed.is_none(), "sheet confirmation must be dropped by an e-stop");
    println!("after estop: unsafe_reasons={:?}", s.unsafe_reasons);
    assert!(ctl.jog_xy(1.0, 0.0).is_err());
    ctl.confirm_sheet("e2e", true).unwrap();
    assert!(ctl.start().unwrap_err().contains("EMERGENCY") || ctl.start().is_err());
    // harness (not the app) re-arms the FAKE controller so the next test run starts clean
    let mut r = String::new();
    for _ in 0..40 {
        r = raw(&sock, "!rearm confirmed");
        if r.starts_with("rearmed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("harness rearm: {r}");
    assert!(r.starts_with("rearmed"), "{r}");
}

#[test]
fn unreachable_controller_is_reported_and_blocks_everything() {
    let ctl = Ctl::new(std::env::temp_dir(), Some("/run/user/1000/ctop-does-not-exist.sock".into()), None, Arc::new(|_, _| {}));
    ctl.set_config(Some("/run/user/1000/ctop-does-not-exist.sock".into()), None);
    ctl.tick();
    let s = ctl.snapshot();
    assert!(!s.reachable);
    println!("unreachable: {}", s.error.as_deref().unwrap());
    assert!(ctl.start().is_err() && ctl.jog_xy(1.0, 0.0).is_err() && ctl.estop().is_err());
}
