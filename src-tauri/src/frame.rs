//! Fixed A4 frame: a named, persistent calibration profile that places the paper in the plotter's coordinates.
//!
//! The frame holds the sheet in the same place every time, so once the sheet's position (origin, orientation, small skew)
//! and the pen's real contact height have been established and verified, drawing needs no camera. Everything is in pen
//! coordinates, i.e. the controller's commanded X/Y, which is where the pen TIP lands (the pen-to-nozzle offset is
//! absorbed because the frame is located by marks the pen itself made).
//!
//! Page space used here: mm, origin = bottom-left corner of the sheet, +y up (same as `PaperGeom::page_to_pen`).
//! "Paper space" (what the browser sends): origin top-left, +y down; `PaperGeom::paper_to_pen` converts.

use crate::paper::PaperGeom;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const A4: [f64; 2] = [210.0, 297.0];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameProfile {
    pub name: String,
    pub created: u64,
    pub updated: u64,
    pub page_w: f64,
    pub page_h: f64,
    /// pen mm of the page's bottom-left corner
    pub origin_pen: [f64; 2],
    /// which way the page axes point relative to the machine: 0, 90, 180 or 270 degrees (page +x direction = Rot(rotation) applied to machine +X)
    pub rotation: i32,
    /// small extra rotation (degrees) found during calibration
    pub skew_deg: f64,
    /// Z (mm) where the pen tip first touches the paper, MEASURED during calibration
    pub surface_z: f64,
    /// Contact height varies across a sheet (curl, bed tilt): z_contact(x, y) = surface_z + plane_gx*(x - ref_x) + plane_gy*(y - ref_y),
    /// with (x, y) in pen mm and the reference point at the page centre. Both slopes are mm of Z per mm of travel; 0 = flat.
    #[serde(default)]
    pub plane_gx: f64,
    #[serde(default)]
    pub plane_gy: f64,
    /// Pressure: how far below the contact height the pen goes when writing. None = use the global setting.
    #[serde(default)]
    pub pen_depth: Option<f64>,
    /// Pen-up (travel) height saved with this calibration. None = use the global setting.
    #[serde(default)]
    pub pen_up_z: Option<f64>,
    /// the points the user marked with the joystick when the position was calibrated (for the record)
    #[serde(default)]
    pub marks: Vec<Mark>,
    /// ink stays at least this far inside the paper edges
    pub margin_mm: f64,
    /// "manual" | "camera-assisted"
    pub source: String,
    /// the test pattern was drawn and confirmed
    pub verified: bool,
    pub verified_at: Option<u64>,
    /// derived, stored for readers that want the matrix: page (BL origin, +y up) -> pen mm
    #[serde(default)]
    pub page_to_pen: [[f64; 3]; 2],
}

pub fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn rot_m(deg: f64) -> [[f64; 2]; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s], [s, c]]
}

impl FrameProfile {
    pub fn new(name: &str, origin_pen: [f64; 2], rotation: i32, skew_deg: f64, surface_z: f64) -> FrameProfile {
        let mut p = FrameProfile {
            name: name.trim().to_string(),
            created: now_s(),
            updated: now_s(),
            page_w: A4[0],
            page_h: A4[1],
            origin_pen,
            rotation,
            skew_deg,
            surface_z,
            plane_gx: 0.0,
            plane_gy: 0.0,
            pen_depth: None,
            pen_up_z: None,
            marks: vec![],
            margin_mm: 8.0,
            source: "manual".into(),
            verified: false,
            verified_at: None,
            page_to_pen: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        };
        p.refresh();
        p
    }

    pub fn matrix(&self) -> [[f64; 3]; 2] {
        let r = rot_m(self.rotation as f64 + self.skew_deg);
        [[r[0][0], r[0][1], self.origin_pen[0]], [r[1][0], r[1][1], self.origin_pen[1]]]
    }

    /// recompute the derived matrix
    pub fn refresh(&mut self) {
        self.page_to_pen = self.matrix();
    }

    pub fn page_to_pen_pt(&self, p: [f64; 2]) -> [f64; 2] {
        let a = self.matrix();
        [a[0][0] * p[0] + a[0][1] * p[1] + a[0][2], a[1][0] * p[0] + a[1][1] * p[1] + a[1][2]]
    }

    /// Is this pen X/Y over the sheet, with `slack` mm to spare around it (negative = inside by that much)?
    pub fn over_sheet(&self, x: f64, y: f64, slack: f64) -> bool {
        let m = self.matrix();
        let (dx, dy) = (x - m[0][2], y - m[1][2]);
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        let (u, v) = ((m[1][1] * dx - m[0][1] * dy) / det, (-m[1][0] * dx + m[0][0] * dy) / det);
        u >= -slack && u <= self.page_w + slack && v >= -slack && v <= self.page_h + slack
    }

    pub fn corners_pen(&self) -> [[f64; 2]; 4] {
        [[0.0, 0.0], [self.page_w, 0.0], [self.page_w, self.page_h], [0.0, self.page_h]].map(|p| self.page_to_pen_pt(p))
    }

    pub fn check(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.len() > 60 || self.name.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
            return Err("The profile needs a name of 1-60 characters (no slashes).".into());
        }
        if !self.origin_pen.iter().all(|v| v.is_finite() && v.abs() < 1000.0) {
            return Err("The paper position is not a valid coordinate.".into());
        }
        if !(100.0..=500.0).contains(&self.page_w) || !(100.0..=500.0).contains(&self.page_h) {
            return Err("The paper size must be between 100 and 500 mm.".into());
        }
        if ![0, 90, 180, 270].contains(&self.rotation) {
            return Err("Paper rotation must be 0, 90, 180 or 270 degrees.".into());
        }
        if !self.skew_deg.is_finite() || self.skew_deg.abs() > 5.0 {
            return Err("A paper skew of more than 5 degrees means the sheet is not seated in the frame; check it and measure again.".into());
        }
        if !(-50.0..=200.0).contains(&self.surface_z) || !self.surface_z.is_finite() {
            return Err("The pen contact height is not valid.".into());
        }
        if !self.plane_gx.is_finite() || !self.plane_gy.is_finite() || self.plane_gx.abs() > 0.02 || self.plane_gy.abs() > 0.02 {
            return Err("The contact plane slope is implausible (more than 2 mm of height change per 100 mm).".into());
        }
        if let Some(d) = self.pen_depth {
            if !(0.2..=1.0).contains(&d) {
                return Err("Writing pressure must be between 0.2 and 1.0 mm.".into());
            }
        }
        if let Some(z) = self.pen_up_z {
            if !(18.0..=60.0).contains(&z) {
                return Err("The pen-up height must be between 18 and 60 mm.".into());
            }
        }
        if !(3.0..=40.0).contains(&self.margin_mm) {
            return Err("The safe margin must be 3 - 40 mm.".into());
        }
        Ok(())
    }

    /// Same data as the camera-derived geometry, so the proven stroke validator and job builder can be reused.
    pub fn geom(&self) -> PaperGeom {
        PaperGeom { corners_img: [[0.0; 2]; 4], corners_pen: self.corners_pen(), width_mm: self.page_w, height_mm: self.page_h, page_to_pen: self.matrix() }
    }

    /// Where the sheet lies in pen coordinates, as the rectangle the plotter can actually reach: the part of the safe
    /// area that is inside `reach` ([xmin, xmax, ymin, ymax]). Returns (reachable height in page mm of the safe area at the page's centre line, fraction of the safe area reachable).
    pub fn reach_report(&self, reach: [f64; 4]) -> (f64, f64) {
        let m = self.margin_mm;
        let (mut ok, mut all) = (0usize, 0usize);
        let n = 60;
        let mut col_ok = 0usize;
        for i in 0..=n {
            for j in 0..=n {
                let p = [m + (self.page_w - 2.0 * m) * i as f64 / n as f64, m + (self.page_h - 2.0 * m) * j as f64 / n as f64];
                let q = self.page_to_pen_pt(p);
                let inside = q[0] >= reach[0] + 0.5 && q[0] <= reach[1] - 0.5 && q[1] >= reach[2] + 0.5 && q[1] <= reach[3] - 0.5;
                all += 1;
                if inside {
                    ok += 1;
                    if i == n / 2 {
                        col_ok += 1;
                    }
                }
            }
        }
        ((self.page_h - 2.0 * m) * col_ok as f64 / (n + 1) as f64, ok as f64 / all as f64)
    }
}

/// Which ink-free zone checks apply: every point must lie inside the safe area (paper minus margin) AND inside the
/// plotter's reachable box. Returns a readable error that names the cause.
pub fn validate(strokes: &[Vec<[f64; 2]>], p: &FrameProfile, reach: [f64; 4]) -> Result<(), String> {
    crate::paper::validate_strokes(strokes, &p.geom(), p.margin_mm, reach).map_err(|e| {
        if e.contains("outside the paper") {
            format!("{e}. Reduce the size or margins so everything stays inside the safe area.")
        } else if e.contains("reach") {
            format!("{e}. The sheet extends beyond the plotter's travel here; use a smaller drawing area.")
        } else {
            e
        }
    })
}

/// Four small crosses near the corners of the safe area plus a centred box. Paper space (top-left origin, +y down).
/// The two bottom crosses are the reference marks used by `adjust`. Marks outside the plotter's reach are left out.
/// Returns (strokes, inset mm of the reference crosses).
pub fn test_pattern(p: &FrameProfile, reach: [f64; 4]) -> (Vec<Vec<[f64; 2]>>, f64) {
    let inset = p.margin_mm + 6.0;
    let arm = 4.0;
    let reachable = |q: [f64; 2]| {
        let w = p.page_to_pen_pt(q);
        w[0] >= reach[0] + 0.5 && w[0] <= reach[1] - 0.5 && w[1] >= reach[2] + 0.5 && w[1] <= reach[3] - 0.5
    };
    let mut centres = vec![[inset, inset], [p.page_w - inset, inset], [inset, p.page_h - inset], [p.page_w - inset, p.page_h - inset]];
    centres.retain(|c| [[-arm, 0.0], [arm, 0.0], [0.0, -arm], [0.0, arm]].iter().all(|d| reachable([c[0] + d[0], c[1] + d[1]])));
    let mut out = vec![];
    let to_paper = |q: [f64; 2]| [q[0], p.page_h - q[1]];
    for c in &centres {
        out.push(vec![to_paper([c[0] - arm, c[1]]), to_paper([c[0] + arm, c[1]])]);
        out.push(vec![to_paper([c[0], c[1] - arm]), to_paper([c[0], c[1] + arm])]);
    }
    // centred 40 x 24 box, only if it is reachable
    let (cx, cy) = (p.page_w / 2.0, p.page_h / 2.0);
    let b = [[cx - 20.0, cy - 12.0], [cx + 20.0, cy - 12.0], [cx + 20.0, cy + 12.0], [cx - 20.0, cy + 12.0], [cx - 20.0, cy - 12.0]];
    if b.iter().all(|q| reachable(*q)) {
        out.push(b.iter().map(|q| to_paper(*q)).collect());
    }
    (out, inset)
}

/// Correct the frame from two measured reference crosses. `d1`/`d2` = (actual - expected) position of the bottom-left and
/// bottom-right crosses on the paper, in mm, x to the right, y up the page. Returns the corrected profile and the
/// x-disagreement between the two marks (a size/scale error that a rigid correction cannot remove).
pub fn adjust(p: &FrameProfile, inset: f64, d1: [f64; 2], d2: [f64; 2]) -> Result<(FrameProfile, f64), String> {
    if d1.iter().chain(d2.iter()).any(|v| !v.is_finite() || v.abs() > 30.0) {
        return Err("The measured offsets must be numbers within 30 mm.".into());
    }
    let dx = p.page_w - 2.0 * inset;
    if dx < 20.0 {
        return Err("The reference marks are too close together.".into());
    }
    let phi = (d2[1] - d1[1]) / dx; // radians, counter-clockwise positive
    let tx = (d1[0] + d2[0]) / 2.0 + phi * inset;
    let ty = d1[1] - phi * inset;
    let mut q = p.clone();
    q.skew_deg = p.skew_deg - phi.to_degrees();
    let r = rot_m(q.rotation as f64 + q.skew_deg);
    q.origin_pen = [p.origin_pen[0] - (r[0][0] * tx + r[0][1] * ty), p.origin_pen[1] - (r[1][0] * tx + r[1][1] * ty)];
    q.verified = false;
    q.verified_at = None;
    q.updated = now_s();
    q.refresh();
    q.check()?;
    Ok((q, d2[0] - d1[0]))
}

/// Proposal from a camera-derived paper geometry. This is only a PROPOSAL: it is verified by the test pattern.
pub fn propose_from_geom(g: &PaperGeom) -> Result<(FrameProfile, Vec<String>), String> {
    let a = g.page_to_pen;
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    if det <= 0.0 {
        return Err("The camera sees the paper mirrored; check the camera calibration.".into());
    }
    let ang = a[1][0].atan2(a[0][0]).to_degrees();
    let rot = ((ang / 90.0).round() as i32).rem_euclid(4) * 90;
    let skew = ang - (ang / 90.0).round() * 90.0;
    let mut notes = vec![];
    if (g.width_mm - A4[0]).abs() > 8.0 || (g.height_mm - A4[1]).abs() > 8.0 {
        notes.push(format!("The camera measured the sheet as {:.0} x {:.0} mm, not A4; the camera calibration may be off.", g.width_mm, g.height_mm));
    }
    let mut p = FrameProfile::new("camera proposal", [a[0][2], a[1][2]], rot, skew, 0.0);
    p.source = "camera-assisted".into();
    p.check().map_err(|e| format!("The camera's paper estimate is not usable: {e}"))?;
    Ok((p, notes))
}

// ───────────── manual calibration: rigid fit of marked points ─────────────

/// A point marked with the pen over the physical sheet, as the user sees it standing at the front of the printer:
/// page x runs left to right, page y from the front (low Y) to the back. `kind` is "corner" (name tl/tr/br/bl) or
/// "edge" (the pen touched that edge somewhere along it; name left/right/front/back).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// pen height when the mark was made (the pen is lowered by hand until it touches before marking)
    #[serde(default)]
    pub z: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub origin: [f64; 2],
    pub skew_deg: f64,
    pub rms_mm: f64,
    /// (mark name, residual mm)
    pub residuals: Vec<(String, f64)>,
    pub warnings: Vec<String>,
}

fn residuals(marks: &[Mark], w: f64, h: f64, p: [f64; 3]) -> Vec<f64> {
    let th = p[2] / 100.0;
    let (c, s) = (th.cos(), th.sin());
    let (ex, ey) = ([c, s], [-s, c]);
    let mut r = vec![];
    for m in marks {
        let (dx, dy) = (m.x - p[0], m.y - p[1]);
        let (u, v) = (dx * ex[0] + dy * ex[1], dx * ey[0] + dy * ey[1]); // the mark in page coordinates
        match (m.kind.as_str(), m.name.as_str()) {
            ("corner", "bl") => r.extend([u, v]),
            ("corner", "br") => r.extend([u - w, v]),
            ("corner", "tr") => r.extend([u - w, v - h]),
            ("corner", "tl") => r.extend([u, v - h]),
            ("edge", "left") => r.push(u),
            ("edge", "right") => r.push(u - w),
            ("edge", "front") => r.push(v),
            ("edge", "back") => r.push(v - h),
            _ => {}
        }
    }
    r
}

/// Rigid (rotation + translation only, no scaling) fit of the sheet to the marked points, against the known sheet size.
/// Returns the sheet's bottom-left origin in pen mm and its small tilt. Errors say which mark to redo or what is still missing.
pub fn solve_marks(marks: &[Mark], w: f64, h: f64) -> Result<Solution, String> {
    for m in marks {
        let ok = matches!((m.kind.as_str(), m.name.as_str()), ("corner", "tl" | "tr" | "br" | "bl") | ("edge", "left" | "right" | "front" | "back"));
        if !ok || !m.x.is_finite() || !m.y.is_finite() {
            return Err(format!("Unknown mark '{} {}'.", m.kind, m.name));
        }
    }
    if marks.is_empty() {
        return Err("Mark at least one corner first.".into());
    }
    // starting guess: place the origin from whatever the marks say about it
    let (mut ox, mut oy, mut nx, mut ny) = (0.0, 0.0, 0.0, 0.0);
    for m in marks {
        match (m.kind.as_str(), m.name.as_str()) {
            ("corner", "bl") => { ox += m.x; oy += m.y; nx += 1.0; ny += 1.0 }
            ("corner", "br") => { ox += m.x - w; oy += m.y; nx += 1.0; ny += 1.0 }
            ("corner", "tr") => { ox += m.x - w; oy += m.y - h; nx += 1.0; ny += 1.0 }
            ("corner", "tl") => { ox += m.x; oy += m.y - h; nx += 1.0; ny += 1.0 }
            ("edge", "left") => { ox += m.x; nx += 1.0 }
            ("edge", "right") => { ox += m.x - w; nx += 1.0 }
            ("edge", "front") => { oy += m.y; ny += 1.0 }
            ("edge", "back") => { oy += m.y - h; ny += 1.0 }
            _ => {}
        }
    }
    let mut p = [if nx > 0.0 { ox / nx } else { 0.0 }, if ny > 0.0 { oy / ny } else { 0.0 }, 0.0];
    let n = residuals(marks, w, h, p).len();
    if n < 3 {
        return Err("Not enough marks to fix the sheet's position and tilt. Mark more corners or edge points.".into());
    }
    let jac = |p: [f64; 3]| -> Vec<[f64; 3]> {
        let base = residuals(marks, w, h, p);
        let mut cols = [vec![], vec![], vec![]];
        for k in 0..3 {
            let mut q = p;
            q[k] += 1e-5;
            cols[k] = residuals(marks, w, h, q).iter().zip(&base).map(|(a, b)| (a - b) / 1e-5).collect();
        }
        (0..base.len()).map(|i| [cols[0][i], cols[1][i], cols[2][i]]).collect()
    };
    let mut lambda = 1e-6;
    for _ in 0..60 {
        let r = residuals(marks, w, h, p);
        let j = jac(p);
        let mut a = [[0.0; 3]; 3];
        let mut g = [0.0; 3];
        for (row, rv) in j.iter().zip(&r) {
            for x in 0..3 {
                g[x] += row[x] * rv;
                for y in 0..3 {
                    a[x][y] += row[x] * row[y];
                }
            }
        }
        for x in 0..3 {
            a[x][x] += lambda;
        }
        let d = solve3(a, [-g[0], -g[1], -g[2]]).ok_or("The marks do not pin down the sheet's position and tilt yet.")?;
        let cost = |q: [f64; 3]| residuals(marks, w, h, q).iter().map(|v| v * v).sum::<f64>();
        let np = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
        if cost(np) <= cost(p) {
            p = np;
            lambda = (lambda * 0.3).max(1e-9);
        } else {
            lambda *= 10.0;
        }
        if d.iter().map(|v| v.abs()).fold(0.0, f64::max) < 1e-9 {
            break;
        }
    }
    // is everything observable? (position in both axes and tilt)
    let j = jac(p);
    let mut a = [[0.0; 3]; 3];
    for row in &j {
        for x in 0..3 {
            for y in 0..3 {
                a[x][y] += row[x] * row[y];
            }
        }
    }
    let tr = (a[0][0] + a[1][1] + a[2][2]) / 3.0;
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det / tr.powi(3).max(1e-12) < 1e-3 {
        return Err("The marks do not fix the sheet yet: you need points on two different sides, and two points along a side to know its tilt.".into());
    }
    let per_mark = |m: &Mark| residuals(std::slice::from_ref(m), w, h, p).iter().map(|v| v * v).sum::<f64>().sqrt();
    let res: Vec<(String, f64)> = marks.iter().map(|m| (format!("{} {}", m.name, m.kind), per_mark(m))).collect();
    let rms = (res.iter().map(|r| r.1 * r.1).sum::<f64>() / res.len() as f64).sqrt();
    let skew = p[2] / 100.0 * 180.0 / std::f64::consts::PI;
    if skew.abs() > 4.0 {
        return Err(format!("The sheet would be tilted {skew:.1} degrees, which is implausible. Check the marks, especially the one with the largest error."));
    }
    if let Some((name, e)) = res.iter().filter(|r| r.1 > 4.0).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
        return Err(format!("The marks do not fit an A4 sheet: '{name}' is {e:.1} mm off. Redo that mark."));
    }
    let mut warnings = vec![];
    if let Some((name, e)) = res.iter().filter(|r| r.1 > 2.0).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
        warnings.push(format!("'{name}' is {e:.1} mm off the best fit; redo it if you were not sure of that spot."));
    }
    Ok(Solution { origin: [p[0], p[1]], skew_deg: skew, rms_mm: rms, residuals: res, warnings })
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(a);
    if d.abs() < 1e-18 {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut m = a;
        for r in 0..3 {
            m[r][k] = b[r];
        }
        out[k] = det(m) / d;
    }
    Some(out)
}

// ───────────── storage ─────────────

pub fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|t| !t.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "frame".into() } else { s.chars().take(48).collect() }
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(data: &Path) -> Store {
        Store { dir: data.join("frames") }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.json", slug(name)))
    }

    pub fn save(&self, p: &FrameProfile) -> Result<(), String> {
        p.check()?;
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let mut q = p.clone();
        q.refresh();
        let f = self.path(&q.name);
        let tmp = f.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&q).map_err(|e| e.to_string())?).map_err(|e| format!("cannot save {}: {e}", f.display()))?;
        fs::rename(&tmp, &f).map_err(|e| e.to_string())
    }

    pub fn load(&self, name: &str) -> Result<FrameProfile, String> {
        let f = self.path(name);
        let t = fs::read_to_string(&f).map_err(|_| format!("There is no calibration profile called '{name}'."))?;
        let mut p: FrameProfile = serde_json::from_str(&t).map_err(|e| format!("Calibration profile '{name}' is unreadable ({e})."))?;
        p.refresh();
        p.check().map_err(|e| format!("Calibration profile '{name}' is invalid: {e}"))?;
        Ok(p)
    }

    pub fn list(&self) -> Vec<FrameProfile> {
        let mut v: Vec<FrameProfile> = fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter_map(|e| fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<FrameProfile>(&t).ok())).collect())
            .unwrap_or_default();
        v.iter_mut().for_each(|p| p.refresh());
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        fs::remove_file(self.path(name)).map_err(|e| e.to_string())
    }
}

/// The user's earlier, hand-measured paper position (controller `paper_frame_pen_coords.json`), used only to pre-fill the
/// calibration form. It is NOT trusted for drawing.
pub fn prior_origin(controller_root: &Path) -> Option<[f64; 2]> {
    let v: Value = serde_json::from_str(&fs::read_to_string(controller_root.join("profiles/paper_frame_pen_coords.json")).ok()?).ok()?;
    let a = v.pointer("/paper_origin/pen_xy")?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof() -> FrameProfile {
        FrameProfile::new("A4 frame", [29.4, 39.1], 0, 0.0, 20.3)
    }

    #[test]
    fn matrix_maps_page_corners() {
        let p = prof();
        let c = p.corners_pen();
        assert!((c[0][0] - 29.4).abs() < 1e-9 && (c[0][1] - 39.1).abs() < 1e-9);
        assert!((c[1][0] - 239.4).abs() < 1e-9 && (c[2][1] - 336.1).abs() < 1e-9);
        let q = FrameProfile::new("r", [239.4, 336.1], 180, 0.0, 20.0);
        let c = q.corners_pen();
        assert!((c[2][0] - 29.4).abs() < 1e-6 && (c[2][1] - 39.1).abs() < 1e-6, "{c:?}");
    }

    #[test]
    fn paper_space_conversion_agrees_with_page_space() {
        let p = prof();
        let g = p.geom();
        // paper space (top-left origin): the top-left corner is page (0, H)
        let a = g.paper_to_pen([0.0, 0.0]);
        let b = p.page_to_pen_pt([0.0, p.page_h]);
        assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    }

    #[test]
    fn validation_keeps_ink_in_safe_area_and_reach() {
        let p = prof();
        let reach = [30.0, 280.0, 30.0, 308.0];
        let ok = vec![vec![[20.0, 100.0], [100.0, 100.0]]];
        assert!(validate(&ok, &p, reach).is_ok());
        let edge = vec![vec![[3.0, 100.0], [100.0, 100.0]]]; // 3 mm from the paper edge, margin is 8
        assert!(validate(&edge, &p, reach).unwrap_err().contains("outside the paper"));
        // top of the sheet (paper y = 10) is pen y ~ 326 > 308: unreachable
        let high = vec![vec![[50.0, 10.0], [100.0, 10.0]]];
        assert!(validate(&high, &p, reach).unwrap_err().contains("reach"));
        let nan = vec![vec![[f64::NAN, 1.0], [2.0, 2.0]]];
        assert!(validate(&nan, &p, reach).is_err());
    }

    #[test]
    fn reach_report_matches_geometry() {
        let p = prof();
        let (h, frac) = p.reach_report([30.0, 280.0, 30.0, 308.0]);
        // safe area is 281 mm tall (297-16) and starts at pen y 47.1; reach ends at 308 -> about 260 mm usable
        assert!((h - 260.0).abs() < 8.0, "{h}");
        assert!(frac > 0.85 && frac < 1.0, "{frac}");
    }

    #[test]
    fn test_pattern_stays_inside_everything() {
        let p = prof();
        let reach = [30.0, 280.0, 30.0, 308.0];
        let (s, inset) = test_pattern(&p, reach);
        assert!((inset - 14.0).abs() < 1e-9);
        assert!(!s.is_empty());
        validate(&s, &p, reach).expect("pattern is valid");
        // bottom references exist (the first two crosses)
        assert!(s.len() >= 4);
    }

    #[test]
    fn adjust_recovers_a_known_error() {
        let believed = prof();
        // reality: what lands on the paper is rotated by phi and shifted by t relative to what the profile commands
        let (phi, tx, ty) = (0.012f64, 1.7f64, -2.3f64);
        let landed = |p: [f64; 2]| [phi.cos() * p[0] - phi.sin() * p[1] + tx, phi.sin() * p[0] + phi.cos() * p[1] + ty];
        let inset = 14.0;
        let (p1, p2) = ([inset, inset], [210.0 - inset, inset]);
        let (l1, l2) = (landed(p1), landed(p2));
        let (fixed, scale_err) = adjust(&believed, inset, [l1[0] - p1[0], l1[1] - p1[1]], [l2[0] - p2[0], l2[1] - p2[1]]).unwrap();
        assert!(scale_err.abs() < 0.05);
        // now command with the fixed profile: landing(command(p)) must equal p. The landing model acts on the believed page frame
        // (command = believed frame coordinates), so go via pen space.
        for p in [[14.0, 14.0], [196.0, 14.0], [105.0, 150.0], [14.0, 283.0], [196.0, 283.0]] {
            let pen = fixed.page_to_pen_pt(p); // where we send the pen
            // invert the believed frame to get the believed page coordinate, then apply reality
            let a = believed.matrix();
            let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
            let (dx, dy) = (pen[0] - a[0][2], pen[1] - a[1][2]);
            let bp = [(a[1][1] * dx - a[0][1] * dy) / det, (-a[1][0] * dx + a[0][0] * dy) / det];
            let l = landed(bp);
            assert!((l[0] - p[0]).abs() < 0.05 && (l[1] - p[1]).abs() < 0.05, "p={p:?} landed={l:?}");
        }
    }

    #[test]
    fn camera_proposal_extracts_origin_rotation_and_skew() {
        let g = FrameProfile::new("x", [50.0, 60.0], 180, 0.8, 0.0).geom();
        let (p, notes) = propose_from_geom(&g).unwrap();
        assert_eq!(p.rotation, 180);
        assert!((p.skew_deg - 0.8).abs() < 1e-9 && (p.origin_pen[0] - 50.0).abs() < 1e-9);
        assert!(notes.is_empty());
        assert!(!p.verified);
        let mut odd = g.clone();
        odd.width_mm = 150.0;
        assert!(!propose_from_geom(&odd).unwrap().1.is_empty());
    }

    fn mk(kind: &str, name: &str, x: f64, y: f64) -> Mark {
        Mark { kind: kind.into(), name: name.into(), x, y, z: None }
    }

    /// pen position of page point (u, v) for a sheet at origin o tilted th degrees
    fn at(o: [f64; 2], th: f64, u: f64, v: f64) -> (f64, f64) {
        let (s, c) = th.to_radians().sin_cos();
        (o[0] + c * u - s * v, o[1] + s * u + c * v)
    }

    #[test]
    fn solver_recovers_origin_and_tilt_from_reachable_marks_only() {
        // the real machine can only reach the front-right corner and points along the right and front edges
        let (o, th) = ([-9.4, 44.2], 0.8);
        let (br, r1, r2, f1, f2) = (at(o, th, 210.0, 0.0), at(o, th, 210.0, 40.0), at(o, th, 210.0, 250.0), at(o, th, 120.0, 0.0), at(o, th, 60.0, 0.0));
        let marks = vec![mk("corner", "br", br.0 + 0.2, br.1 - 0.3), mk("edge", "right", r1.0, r1.1), mk("edge", "right", r2.0 - 0.2, r2.1), mk("edge", "front", f1.0, f1.1 + 0.2), mk("edge", "front", f2.0, f2.1)];
        let s = solve_marks(&marks, 210.0, 297.0).unwrap();
        assert!((s.origin[0] - o[0]).abs() < 0.6 && (s.origin[1] - o[1]).abs() < 0.6, "{:?}", s.origin);
        assert!((s.skew_deg - th).abs() < 0.15, "{}", s.skew_deg);
        assert!(s.rms_mm < 0.5);
    }

    #[test]
    fn solver_with_four_corners_and_an_outlier() {
        let (o, th) = (&[20.0, 30.0], 0.0);
        let mut marks = vec![mk("corner", "bl", 20.0, 30.0), mk("corner", "br", 230.0, 30.0), mk("corner", "tr", 230.0, 327.0), mk("corner", "tl", 20.0, 327.0)];
        let ok = solve_marks(&marks, 210.0, 297.0).unwrap();
        assert!((ok.origin[0] - o[0]).abs() < 1e-3 && ok.skew_deg.abs() < 1e-3);
        marks[2].y += 12.0; // the top-right mark is far off
        let e = solve_marks(&marks, 210.0, 297.0).unwrap_err();
        assert!(e.contains("tr corner"), "{e}");
    }

    #[test]
    fn solver_says_what_is_missing() {
        assert!(solve_marks(&[], 210.0, 297.0).is_err());
        // one corner: position known but no tilt
        let e = solve_marks(&[mk("corner", "br", 200.0, 45.0)], 210.0, 297.0).unwrap_err();
        assert!(e.contains("Not enough") || e.contains("fix the sheet"), "{e}");
        // two points on the same edge give the tilt but not the other axis
        let e = solve_marks(&[mk("edge", "right", 200.0, 45.0), mk("edge", "right", 200.0, 200.0)], 210.0, 297.0).unwrap_err();
        assert!(e.contains("two different sides") || e.contains("Not enough"), "{e}");
        assert!(solve_marks(&[mk("corner", "xx", 1.0, 1.0)], 210.0, 297.0).is_err());
    }

    #[test]
    fn solver_rejects_a_tilt_that_cannot_be_right() {
        let marks = vec![mk("corner", "bl", 0.0, 0.0), mk("corner", "br", 209.0, 30.0), mk("corner", "tr", 180.0, 330.0), mk("corner", "tl", -30.0, 297.0)];
        assert!(solve_marks(&marks, 210.0, 297.0).is_err());
    }

    #[test]
    fn over_sheet_follows_the_transform() {
        let p = prof(); // origin 29.4, 39.1
        assert!(p.over_sheet(100.0, 100.0, 0.0));
        assert!(!p.over_sheet(20.0, 100.0, 4.0));
        assert!(p.over_sheet(27.0, 100.0, 4.0));
        let q = FrameProfile::new("r", [239.4, 336.1], 180, 0.0, 20.0);
        assert!(q.over_sheet(100.0, 100.0, 0.0) && !q.over_sheet(250.0, 100.0, 4.0));
    }

    #[test]
    fn pen_settings_are_validated() {
        let mut p = prof();
        p.pen_depth = Some(1.5);
        assert!(p.check().is_err());
        p.pen_depth = Some(0.6);
        p.pen_up_z = Some(10.0);
        assert!(p.check().is_err());
        p.pen_up_z = Some(30.0);
        assert!(p.check().is_ok());
    }

    #[test]
    fn rejects_unsafe_profiles() {
        let mut p = prof();
        p.skew_deg = 9.0;
        assert!(p.check().is_err());
        let mut p = prof();
        p.rotation = 45;
        assert!(p.check().is_err());
        let mut p = prof();
        p.name = "a/b".into();
        assert!(p.check().is_err());
    }

    #[test]
    fn profiles_persist_across_restarts() {
        let dir = std::env::temp_dir().join(format!("ctop-frame-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let st = Store::new(&dir);
        let mut p = prof();
        p.verified = true;
        p.verified_at = Some(123);
        st.save(&p).unwrap();
        let again = Store::new(&dir); // a new process would build a new Store
        let q = again.load("A4 frame").unwrap();
        assert_eq!(q.origin_pen, p.origin_pen);
        assert!(q.verified && q.verified_at == Some(123));
        assert_eq!(again.list().len(), 1);
        again.delete("A4 frame").unwrap();
        assert!(again.load("A4 frame").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
