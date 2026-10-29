//! Camera -> paper -> plotter geometry. The camera hub (camview) finds the paper outline in the image; a persistent
//! camera calibration (normalised image coords -> pen mm at a fixed observation pose) turns that into plotter
//! coordinates. Everything is deterministic geometry; no inference.

use crate::geom::{self, H};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

// ───────────── camera calibration ─────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalPoint {
    pub img: [f64; 2],
    pub pen: [f64; 2],
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CameraCal {
    pub version: u32,
    /// normalised image coordinates (u,v in 0..1 of the frame) -> pen mm; stored as 9 numbers, row-major
    #[serde(with = "flat_h")]
    pub h: H,
    /// machine X,Y at which the camera frames were taken; the paper only maps correctly at this pose (the bed moves)
    pub pose: [f64; 2],
    pub rms_mm: f64,
    pub loo_mm: Option<f64>,
    pub created: u64,
    #[serde(default)]
    pub points: Vec<CalPoint>,
}

mod flat_h {
    use super::H;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(h: &H, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(h.iter().flatten())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<H, D::Error> {
        let v = Vec::<f64>::deserialize(d)?;
        if v.len() != 9 {
            return Err(serde::de::Error::custom("h must have 9 numbers"));
        }
        Ok([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
    }
}

pub const MAX_CAL_RMS_MM: f64 = 1.5;
pub const POSE_TOL_MM: f64 = 2.0;

impl CameraCal {
    pub fn solve(points: Vec<CalPoint>, pose: [f64; 2], now: u64) -> Result<CameraCal, String> {
        if points.len() < 4 {
            return Err("need at least 4 calibration points (6 recommended)".into());
        }
        if points.iter().any(|p| p.img.iter().chain(p.pen.iter()).any(|v| !v.is_finite())) {
            return Err("non-finite calibration point".into());
        }
        let src: Vec<_> = points.iter().map(|p| p.img).collect();
        let dst: Vec<_> = points.iter().map(|p| p.pen).collect();
        // the points must span an area, not lie on a line
        let (minx, maxx) = src.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
        let (miny, maxy) = src.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
        if maxx - minx < 0.1 || maxy - miny < 0.1 {
            return Err("calibration points are clustered: spread them over the paper".into());
        }
        let h = geom::fit(&src, &dst).ok_or("could not solve the calibration (points are degenerate)")?;
        let rms = geom::rms(&h, &src, &dst);
        let loo = geom::loo_max(&src, &dst);
        let worst = loo.unwrap_or(rms);
        if rms > MAX_CAL_RMS_MM || worst > 2.0 * MAX_CAL_RMS_MM {
            return Err(format!("calibration is not accurate enough (rms {rms:.2} mm, worst leave-one-out {worst:.2} mm): a click was probably on the wrong mark"));
        }
        Ok(CameraCal { version: 1, h, pose, rms_mm: rms, loo_mm: loo, created: now, points })
    }

    /// Calibrate from clicked mark positions given in ANY order: the marks are laid out asymmetrically, so only the
    /// correct labelling fits a homography. The best labelling must beat every other by a wide margin.
    pub fn solve_unordered(imgs: &[[f64; 2]], pens: &[[f64; 2]], pose: [f64; 2], now: u64) -> Result<CameraCal, String> {
        let n = pens.len();
        if imgs.len() != n || !(4..=7).contains(&n) {
            return Err(format!("expected {n} clicked marks, got {}", imgs.len()));
        }
        let mut idx: Vec<usize> = (0..n).collect();
        let mut best: (f64, Vec<usize>) = (f64::MAX, vec![]);
        let mut second = f64::MAX;
        permute(&mut idx, 0, &mut |perm| {
            let src: Vec<[f64; 2]> = perm.iter().map(|&i| imgs[i]).collect();
            if let Some(h) = geom::fit(&src, pens) {
                let r = geom::rms(&h, &src, pens);
                if r < best.0 {
                    second = best.0;
                    best = (r, perm.to_vec());
                } else if r < second {
                    second = r;
                }
            }
        });
        if best.1.is_empty() {
            return Err("could not match the clicked marks".into());
        }
        if second < 3.0 * best.0.max(0.2) {
            return Err("the clicked marks could not be matched unambiguously: click only the drawn crosses".into());
        }
        let pts = best.1.iter().enumerate().map(|(k, &i)| CalPoint { img: imgs[i], pen: pens[k] }).collect();
        CameraCal::solve(pts, pose, now)
    }

    pub fn to_pen(&self, uv: [f64; 2]) -> Option<[f64; 2]> {
        geom::apply(&self.h, uv)
    }

    pub fn pose_delta(&self, pos: [f64; 2]) -> f64 {
        geom::dist(pos, self.pose)
    }
}

fn permute(a: &mut Vec<usize>, k: usize, f: &mut dyn FnMut(&[usize])) {
    if k == a.len() {
        f(a);
        return;
    }
    for i in k..a.len() {
        a.swap(k, i);
        permute(a, k + 1, f);
        a.swap(k, i);
    }
}

// ───────────── hub client (plain HTTP/1.0, loopback only) ─────────────

pub fn parse_base(url: &str) -> Result<(String, u16), String> {
    let rest = url.trim().strip_prefix("http://").ok_or("camera address must start with http://")?;
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "bad camera port")?),
        None => (hostport.to_string(), 80),
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return Err("the camera service must run on this computer (127.0.0.1)".into());
    }
    Ok((host, port))
}

pub fn http_get(base: &str, path: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    http_req("GET", base, path, timeout)
}

/// Body-less POST (the hub's command endpoints take no payload we need).
pub fn http_post(base: &str, path: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    http_req("POST", base, path, timeout)
}

/// POST with a JSON body (the camera hub's /api/mode).
pub fn http_post_json(base: &str, path: &str, body: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    http_req_body("POST", base, path, timeout, body)
}

fn http_req(method: &str, base: &str, path: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    http_req_body(method, base, path, timeout, "")
}

fn http_req_body(method: &str, base: &str, path: &str, timeout: Duration, body: &str) -> Result<Vec<u8>, String> {
    let (host, port) = parse_base(base)?;
    let addr = (if host == "localhost" { "127.0.0.1" } else { host.as_str() }, port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("bad address")?;
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("camera service not reachable on {host}:{port} ({e})"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    s.write_all(format!("{method} {path} HTTP/1.0\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.take(16 << 20).read_to_end(&mut buf).map_err(|e| format!("camera read: {e}"))?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("camera sent a malformed response")?;
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    if status != 200 {
        return Err(format!("camera service answered HTTP {status}"));
    }
    Ok(buf[split + 4..].to_vec())
}

/// What the hub says about the paper in the current frame.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Seen {
    pub found: bool,
    pub corners: Option<[[f64; 2]; 4]>,
    pub all_visible: bool,
}

pub fn parse_analysis(v: &Value) -> Seen {
    let p = &v["paper"];
    let found = p["found"].as_bool().unwrap_or(false);
    let corners = p["corners"].as_array().filter(|a| a.len() == 4).and_then(|a| {
        let mut o = [[0.0; 2]; 4];
        for (i, c) in a.iter().enumerate() {
            o[i] = [c.get(0)?.as_f64()?, c.get(1)?.as_f64()?];
        }
        Some(o)
    });
    let all_visible = p["corner_visible"].as_array().map(|a| a.len() == 4 && a.iter().all(|x| x.as_bool().unwrap_or(false))).unwrap_or(false);
    Seen { found: found && corners.is_some(), corners: if found { corners } else { None }, all_visible }
}

// ───────────── paper geometry ─────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PaperGeom {
    /// corners in PAGE order [BL, BR, TR, TL], normalised image coordinates
    pub corners_img: [[f64; 2]; 4],
    /// same corners in pen mm
    pub corners_pen: [[f64; 2]; 4],
    pub width_mm: f64,
    pub height_mm: f64,
    /// page (origin bottom-left, +y up, mm) -> pen mm
    pub page_to_pen: [[f64; 3]; 2],
}

impl PaperGeom {
    /// page mm (origin top-left, +y down; the app's "paper space") -> pen mm
    pub fn paper_to_pen(&self, p: [f64; 2]) -> [f64; 2] {
        let (px, py) = (p[0], self.height_mm - p[1]);
        let a = &self.page_to_pen;
        [a[0][0] * px + a[0][1] * py + a[0][2], a[1][0] * px + a[1][1] * py + a[1][2]]
    }
}

pub fn paper_geom(seen: &[[f64; 2]; 4], cal: &CameraCal, rotation_deg: f64) -> Result<PaperGeom, String> {
    let mut pen = [[0.0; 2]; 4];
    for i in 0..4 {
        pen[i] = cal.to_pen(seen[i]).ok_or("paper corner cannot be mapped to the plotter")?;
    }
    let o = geom::order_quad(&pen, rotation_deg).ok_or("the detected paper outline is not a proper rectangle")?;
    let cp = [pen[o[0]], pen[o[1]], pen[o[2]], pen[o[3]]];
    let ci = [seen[o[0]], seen[o[1]], seen[o[2]], seen[o[3]]];
    let (bl, br, tr, tl) = (cp[0], cp[1], cp[2], cp[3]);
    let (w1, w2, h1, h2) = (geom::dist(bl, br), geom::dist(tl, tr), geom::dist(bl, tl), geom::dist(br, tr));
    let (w, h) = ((w1 + w2) / 2.0, (h1 + h2) / 2.0);
    if !(40.0..=500.0).contains(&w) || !(40.0..=500.0).contains(&h) {
        return Err(format!("detected paper size {w:.0} x {h:.0} mm is implausible: check camera calibration"));
    }
    if (w1 - w2).abs() > 0.06 * w || (h1 - h2).abs() > 0.06 * h {
        return Err("detected paper is not a rectangle in plotter space (opposite sides differ >6%): check camera calibration".into());
    }
    let ang = (br[1] - bl[1]).atan2(br[0] - bl[0]);
    let mut ang_l = (tl[1] - bl[1]).atan2(tl[0] - bl[0]) - ang;
    while ang_l > std::f64::consts::PI {
        ang_l -= 2.0 * std::f64::consts::PI;
    }
    while ang_l < -std::f64::consts::PI {
        ang_l += 2.0 * std::f64::consts::PI;
    }
    if (ang_l - std::f64::consts::FRAC_PI_2).abs() > 0.1 {
        return Err("detected paper corners are not square (>5.7 deg off)".into());
    }
    let (c, s) = (ang.cos(), ang.sin());
    Ok(PaperGeom { corners_img: ci, corners_pen: cp, width_mm: w, height_mm: h, page_to_pen: [[c, -s, bl[0]], [s, c, bl[1]]] })
}

/// Largest corner displacement (mm) between two pen-space quads.
pub fn quad_shift(a: &[[f64; 2]; 4], b: &[[f64; 2]; 4]) -> f64 {
    (0..4).map(|i| geom::dist(a[i], b[i])).fold(0.0, f64::max)
}

/// Tracks the paper over time: it is `stable` after several consistent samples, and the `locked` geometry only
/// changes when the paper has really moved, so downstream analysis (line detection) is not repeated needlessly.
#[derive(Default)]
pub struct Tracker {
    samples: VecDeque<[[f64; 2]; 4]>,
    pub locked: Option<PaperGeom>,
    pub lock_id: u64,
    pub live: Option<PaperGeom>,
    pub stable: bool,
    pub last_err: Option<String>,
}

pub const STABLE_JITTER_MM: f64 = 0.8;
pub const RELOCK_SHIFT_MM: f64 = 1.5;
const WINDOW: usize = 4;

impl Tracker {
    /// Put a previously valid lock back (the paper was moved by us and returned to the same place).
    pub fn restore(&mut self, g: PaperGeom, id: u64) {
        self.locked = Some(g);
        self.lock_id = id;
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.live = None;
        self.stable = false;
        self.locked = None;
    }

    pub fn update(&mut self, seen: &Seen, cal: Option<&CameraCal>, rotation_deg: f64) {
        self.last_err = None;
        let (Some(corners), Some(cal)) = (seen.corners.filter(|_| seen.found && seen.all_visible), cal) else {
            self.samples.clear();
            self.live = None;
            self.stable = false;
            self.locked = None; // the paper is gone or hidden: nothing may be written against an old position
            return;
        };
        match paper_geom(&corners, cal, rotation_deg) {
            Err(e) => {
                self.last_err = Some(e);
                self.samples.clear();
                self.live = None;
                self.stable = false;
                self.locked = None;
            }
            Ok(g) => {
                self.samples.push_back(g.corners_pen);
                while self.samples.len() > WINDOW {
                    self.samples.pop_front();
                }
                let n = self.samples.len();
                let last = self.samples[n - 1];
                let jitter = self.samples.iter().map(|s| quad_shift(s, &last)).fold(0.0, f64::max);
                self.stable = n >= WINDOW && jitter <= STABLE_JITTER_MM;
                if self.stable {
                    let moved = self.locked.as_ref().map(|l| quad_shift(&l.corners_pen, &g.corners_pen) > RELOCK_SHIFT_MM || (l.width_mm - g.width_mm).abs() > 3.0).unwrap_or(true);
                    if moved {
                        self.locked = Some(g.clone());
                        self.lock_id += 1;
                    }
                } else if let Some(l) = &self.locked {
                    // paper is moving: drop the lock once it has really left it
                    if quad_shift(&l.corners_pen, &g.corners_pen) > RELOCK_SHIFT_MM * 2.0 {
                        self.locked = None;
                    }
                }
                self.live = Some(g);
            }
        }
    }
}

// ───────────── validation of what is about to be drawn ─────────────

/// Check strokes (paper space: mm, origin top-left, y down) against the paper and the plotter's reachable box.
/// `edge` is the minimum distance from the paper edge. Returns an error describing the first problems found.
pub fn validate_strokes(strokes: &[Vec<[f64; 2]>], g: &PaperGeom, edge: f64, reach: [f64; 4]) -> Result<(), String> {
    if strokes.is_empty() {
        return Err("nothing to write".into());
    }
    let (mut off_paper, mut off_reach, mut bad) = (0usize, 0usize, 0usize);
    for s in strokes {
        for p in s {
            if !p[0].is_finite() || !p[1].is_finite() {
                bad += 1;
                continue;
            }
            if p[0] < edge || p[0] > g.width_mm - edge || p[1] < edge || p[1] > g.height_mm - edge {
                off_paper += 1;
            }
            let q = g.paper_to_pen(*p);
            if q[0] < reach[0] + 0.5 || q[0] > reach[1] - 0.5 || q[1] < reach[2] + 0.5 || q[1] > reach[3] - 0.5 {
                off_reach += 1;
            }
        }
    }
    if bad > 0 {
        return Err("the writing contains invalid coordinates".into());
    }
    if off_paper > 0 {
        return Err(format!("{off_paper} points would fall outside the paper (closer than {edge:.0} mm to the edge)"));
    }
    if off_reach > 0 {
        return Err(format!("{off_reach} points are outside the area the plotter can reach"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground-truth camera: pen mm -> normalised image, 180 deg rotated with perspective.
    pub fn cam() -> (H, CameraCal) {
        let to_img: H = [[-0.0017, 0.0, 0.85], [0.0, 0.0030, 0.20], [1.0e-5, 2.0e-5, 1.0]];
        let inv = geom::invert(&to_img).unwrap();
        let pen: Vec<[f64; 2]> = vec![[80., 60.], [240., 60.], [240., 250.], [80., 250.], [160., 150.], [110., 220.]];
        let pts = pen.iter().map(|p| CalPoint { img: geom::apply(&to_img, *p).unwrap(), pen: *p }).collect();
        let c = CameraCal::solve(pts, [270.0, 305.0], 0).unwrap();
        let _ = inv;
        (to_img, c)
    }

    fn paper_quad(to_img: &H, x0: f64, y0: f64, w: f64, h: f64) -> [[f64; 2]; 4] {
        [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]].map(|p| geom::apply(to_img, p).unwrap())
    }

    #[test]
    fn calibration_solves_and_rejects_bad_clicks() {
        let (_, c) = cam();
        assert!(c.rms_mm < 1e-6);
        let mut bad = c.points.clone();
        bad[2].img[0] += 0.08;
        assert!(CameraCal::solve(bad, [0.0, 0.0], 0).is_err());
        assert!(CameraCal::solve(c.points[..3].to_vec(), [0.0, 0.0], 0).is_err());
    }

    #[test]
    fn calibration_file_round_trips_flat_h() {
        let (_, c) = cam();
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["h"].as_array().unwrap().len(), 9);
        let back: CameraCal = serde_json::from_value(j).unwrap();
        assert_eq!(back.h, c.h);
    }

    #[test]
    fn unordered_clicks_are_matched_only_when_layout_is_asymmetric() {
        let (to_img, _) = cam();
        let pens = crate::writer::mark_layout([155.0, 169.0], 60.0, 50.0);
        // clicks arrive shuffled and with ~0.4 px noise at 1280x720 (= 0.0003 normalised)
        let order = [3usize, 0, 5, 1, 4, 2];
        let imgs: Vec<[f64; 2]> = order.iter().enumerate().map(|(k, &i)| {
            let p = geom::apply(&to_img, pens[i]).unwrap();
            [p[0] + 0.0003 * ((k as f64 * 1.7).sin()), p[1] + 0.0003 * ((k as f64 * 2.3).cos())]
        }).collect();
        let c = CameraCal::solve_unordered(&imgs, &pens, [270.0, 305.0], 0).unwrap();
        let mid = c.to_pen(geom::apply(&to_img, [130.0, 200.0]).unwrap()).unwrap();
        assert!(geom::dist(mid, [130.0, 200.0]) < 0.8, "{mid:?}");
        // a symmetric 3x2 grid would be ambiguous: the matcher must refuse rather than guess
        let grid: Vec<[f64; 2]> = vec![[100., 120.], [150., 120.], [200., 120.], [100., 180.], [150., 180.], [200., 180.]];
        let gi: Vec<[f64; 2]> = grid.iter().map(|p| geom::apply(&to_img, *p).unwrap()).collect();
        assert!(CameraCal::solve_unordered(&gi, &grid, [0.0, 0.0], 0).is_err());
        // a stray click is refused
        let mut bad = imgs.clone();
        bad[0] = [0.5, 0.5];
        assert!(CameraCal::solve_unordered(&bad, &pens, [0.0, 0.0], 0).is_err());
    }

    #[test]
    fn paper_geometry_recovers_position_size_and_orientation() {
        let (to_img, c) = cam();
        let q = paper_quad(&to_img, 40.0, 30.0, 210.0, 297.0 - 60.0);
        // shuffle the corner order: the hub's order is not relied upon
        let seen = [q[2], q[0], q[3], q[1]];
        let g = paper_geom(&seen, &c, 0.0).unwrap();
        assert!((g.width_mm - 210.0).abs() < 0.01 && (g.height_mm - 237.0).abs() < 0.01);
        // page origin (bottom-left) is pen (40,30); page top-left corner (paper space 0,0) is pen (40, 267)
        assert!(geom::dist(g.paper_to_pen([0.0, g.height_mm]), [40.0, 30.0]) < 0.01);
        assert!(geom::dist(g.paper_to_pen([0.0, 0.0]), [40.0, 267.0]) < 0.01);
        assert!(geom::dist(g.paper_to_pen([210.0, 0.0]), [250.0, 267.0]) < 0.01);
        // rotated heading flips which corner is the origin
        let g180 = paper_geom(&seen, &c, 180.0).unwrap();
        assert!(geom::dist(g180.paper_to_pen([0.0, g180.height_mm]), [250.0, 267.0]) < 0.01);
    }

    #[test]
    fn tilted_paper_keeps_geometry() {
        let (to_img, c) = cam();
        let a = 7.0f64.to_radians();
        let rot = |p: [f64; 2]| [120.0 + (p[0] - 120.0) * a.cos() - (p[1] - 100.0) * a.sin(), 100.0 + (p[0] - 120.0) * a.sin() + (p[1] - 100.0) * a.cos()];
        let q = [[0., 0.], [148., 0.], [148., 210.], [0., 210.]].map(|p| geom::apply(&to_img, rot([p[0] + 60.0, p[1] + 40.0])).unwrap());
        let g = paper_geom(&q, &c, 0.0).unwrap();
        assert!((g.width_mm - 148.0).abs() < 0.01 && (g.height_mm - 210.0).abs() < 0.01);
        // text baseline along the page x-axis must be 7 degrees rotated in pen space
        let (p0, p1) = (g.paper_to_pen([10.0, 50.0]), g.paper_to_pen([110.0, 50.0]));
        let ang = (p1[1] - p0[1]).atan2(p1[0] - p0[0]).to_degrees();
        assert!((ang - 7.0).abs() < 0.01, "{ang}");
    }

    #[test]
    fn implausible_paper_is_refused() {
        let (to_img, c) = cam();
        let q = paper_quad(&to_img, 40.0, 30.0, 10.0, 10.0);
        assert!(paper_geom(&q, &c, 0.0).is_err());
    }

    #[test]
    fn tracker_locks_when_still_and_relocks_when_moved() {
        let (to_img, c) = cam();
        let mut t = Tracker::default();
        let mk = |x: f64| Seen { found: true, corners: Some(paper_quad(&to_img, x, 30.0, 210.0, 230.0)), all_visible: true };
        for _ in 0..3 {
            t.update(&mk(40.0), Some(&c), 0.0);
            assert!(t.locked.is_none());
        }
        t.update(&mk(40.0), Some(&c), 0.0);
        assert!(t.stable && t.locked.is_some());
        let id = t.lock_id;
        for _ in 0..5 {
            t.update(&mk(40.2), Some(&c), 0.0); // jitter below the relock threshold
        }
        assert_eq!(t.lock_id, id, "small jitter must not retrigger analysis");
        for _ in 0..6 {
            t.update(&mk(55.0), Some(&c), 0.0);
        }
        assert!(t.lock_id > id, "moved paper is re-locked");
        t.update(&Seen::default(), Some(&c), 0.0);
        assert!(t.locked.is_none(), "paper gone: lock dropped");
    }

    #[test]
    fn stroke_validation_blocks_off_paper_and_off_reach() {
        let (to_img, c) = cam();
        let q = paper_quad(&to_img, 40.0, 30.0, 210.0, 230.0);
        let g = paper_geom(&q, &c, 0.0).unwrap();
        let reach = [30.0, 280.0, 30.0, 308.0];
        let ok = vec![vec![[20.0, 20.0], [100.0, 20.0]]];
        assert!(validate_strokes(&ok, &g, 3.0, reach).is_ok());
        assert!(validate_strokes(&[vec![[1.0, 20.0], [50.0, 20.0]]], &g, 3.0, reach).unwrap_err().contains("outside the paper"));
        assert!(validate_strokes(&[vec![[20.0, 20.0], [215.0, 20.0]]], &g, 3.0, reach).is_err());
        // inside the paper but beyond the plotter box
        assert!(validate_strokes(&ok, &g, 3.0, [30.0, 280.0, 30.0, 200.0]).unwrap_err().contains("reach"));
        assert!(validate_strokes(&[vec![[f64::NAN, 1.0], [5.0, 5.0]]], &g, 3.0, reach).is_err());
        assert!(validate_strokes(&[], &g, 3.0, reach).is_err());
    }

    #[test]
    fn analysis_parsing_matches_hub_shape() {
        let v = serde_json::json!({"paper": {"found": true, "corners": [[0.1,0.1],[0.9,0.1],[0.9,0.9],[0.1,0.9]], "corner_visible": [true,true,true,true]}});
        let s = parse_analysis(&v);
        assert!(s.found && s.all_visible);
        assert!(!parse_analysis(&serde_json::json!({"paper": {"found": false}})).found);
        assert!(parse_base("http://example.com:7792").is_err());
        assert!(parse_base("http://127.0.0.1:7792").is_ok());
    }
}
