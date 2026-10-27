//! Marlin G-code generation from planned pen strokes (paper space) and a
//! resolved calibration. Output only uses G21, G90, G0, G1, G4, M220, M400.
//! No heater, fan or extruder command can be produced here.

use crate::calibration::Resolved;
use crate::planner::{stroke_length, Stroke};
use crate::safety::{verify_program, Limits, Violation};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GcodeSettings {
    /// mm/min while the pencil is on the paper.
    pub draw_feed: f64,
    /// mm/min for pen-up moves.
    pub travel_feed: f64,
    /// mm/min for Z moves.
    pub z_feed: f64,
    /// Dwell after lowering the pencil (ms).
    pub pen_settle_ms: u32,
    /// Acceleration used only for the time *estimate* (mm/s²).
    pub accel_mm_s2: f64,
    /// Segments shorter than this are merged away.
    pub min_segment_mm: f64,
}

impl Default for GcodeSettings {
    fn default() -> Self {
        GcodeSettings { draw_feed: 1800.0, travel_feed: 4800.0, z_feed: 600.0, pen_settle_ms: 0, accel_mm_s2: 400.0, min_segment_mm: 0.05 }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Setup,
    Travel,
    PenDown,
    Draw,
    PenUp,
    /// `M400`: its `ok` proves every earlier move has physically finished.
    Checkpoint,
    Park,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProgramLine {
    pub text: String,
    pub kind: LineKind,
    /// Index of the stroke this line belongs to.
    pub stroke: Option<usize>,
    /// Nozzle position after this line executes (for checkpoint position verification).
    pub target: Option<[f64; 3]>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProgramStats {
    pub strokes: usize,
    pub lines: usize,
    pub draw_length_mm: f64,
    pub travel_length_mm: f64,
    pub pen_lifts: usize,
    /// Estimate only: from path length, configured feedrates and a trapezoid accel model.
    pub estimated_seconds: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    pub lines: Vec<ProgramLine>,
    pub stats: ProgramStats,
}

impl Program {
    pub fn to_text(&self, header: &str) -> String {
        let mut s = String::new();
        for l in header.lines() {
            s.push_str("; ");
            s.push_str(l);
            s.push('\n');
        }
        for l in &self.lines {
            s.push_str(&l.text);
            s.push('\n');
        }
        s
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum GcodeError {
    Settings(String),
    Unsafe { stroke: Option<usize>, reason: String },
    Nothing,
}

impl std::fmt::Display for GcodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GcodeError::Settings(s) => write!(f, "Invalid settings: {s}"),
            GcodeError::Unsafe { stroke, reason } => match stroke {
                Some(i) => write!(f, "Unsafe path (stroke {}): {reason}", i + 1),
                None => write!(f, "Unsafe path: {reason}"),
            },
            GcodeError::Nothing => write!(f, "Nothing to draw"),
        }
    }
}

fn n(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// Trapezoidal move time in seconds for distance `d` mm at `v` mm/s, accel `a`.
fn move_time(d: f64, v: f64, a: f64) -> f64 {
    if d <= 0.0 {
        return 0.0;
    }
    let ramp = v * v / a; // distance for accel + decel
    if d >= ramp { d / v + v / a } else { 2.0 * (d / a).sqrt() }
}

pub fn validate_settings(s: &GcodeSettings, l: &Limits) -> Result<(), GcodeError> {
    let bad = |m: &str| Err(GcodeError::Settings(m.into()));
    if !(s.draw_feed > 0.0 && s.draw_feed <= l.max_draw_feed) {
        return bad(&format!("Draw feed must be 1–{} mm/min", l.max_draw_feed));
    }
    if !(s.travel_feed > 0.0 && s.travel_feed <= l.max_travel_feed) {
        return bad(&format!("Travel feed must be 1–{} mm/min", l.max_travel_feed));
    }
    if !(s.z_feed > 0.0 && s.z_feed <= l.max_z_feed) {
        return bad(&format!("Z feed must be 1–{} mm/min", l.max_z_feed));
    }
    if !(s.accel_mm_s2 > 0.0) || s.pen_settle_ms > 5000 || !(s.min_segment_mm >= 0.0) {
        return bad("Acceleration, settle time or minimum segment out of range");
    }
    Ok(())
}

pub fn generate(strokes: &[Stroke], cal: &Resolved, s: &GcodeSettings, limits: &Limits) -> Result<Program, GcodeError> {
    validate_settings(s, limits)?;
    let strokes: Vec<&Stroke> = strokes.iter().filter(|st| st.len() >= 2).collect();
    if strokes.is_empty() {
        return Err(GcodeError::Nothing);
    }

    let mut lines: Vec<ProgramLine> = Vec::new();
    let mut est = 0.0;
    let (mut draw_len, mut travel_len) = (0.0, 0.0);
    let (v_draw, v_trav, v_z) = (s.draw_feed / 60.0, s.travel_feed / 60.0, s.z_feed / 60.0);
    let push = |lines: &mut Vec<ProgramLine>, text: String, kind, stroke, target| {
        lines.push(ProgramLine { text, kind, stroke, target });
    };

    push(&mut lines, "G21".into(), LineKind::Setup, None, None);
    push(&mut lines, "G90".into(), LineKind::Setup, None, None);
    push(&mut lines, "M220 S100".into(), LineKind::Setup, None, None);

    // Establish Z at calibrated safe clearance before any XY motion. The caller guarantees Z is
    // a verified, known coordinate (job start is refused otherwise).
    let mut nz = cal.z_safe;
    let _ = &mut nz;
    push(&mut lines, format!("G1 Z{} F{}", n(cal.z_safe), n(s.z_feed)), LineKind::Setup, None, None);
    let mut cur_xy: Option<(f64, f64)> = None;

    for (i, st) in strokes.iter().enumerate() {
        let nozzle: Vec<(f64, f64)> = st
            .iter()
            .map(|p| {
                if !cal.in_drawable(p[0], p[1]) {
                    Err(GcodeError::Unsafe {
                        stroke: Some(i),
                        reason: format!("point ({:.2}, {:.2}) mm is outside the drawable paper area", p[0], p[1]),
                    })
                } else {
                    Ok(cal.paper_to_nozzle(p[0], p[1]))
                }
            })
            .collect::<Result<_, _>>()?;

        let (x0, y0) = nozzle[0];
        // approach: first stroke travels at safe clearance, later strokes at pen-up height
        let first = cur_xy.is_none();
        let d = cur_xy.map_or(0.0, |(cx, cy)| ((x0 - cx).powi(2) + (y0 - cy).powi(2)).sqrt());
        if first || d > 1e-4 {
            push(&mut lines, format!("G0 X{} Y{} F{}", n(x0), n(y0), n(s.travel_feed)), LineKind::Travel, Some(i), Some([x0, y0, nz]));
            est += move_time(d, v_trav, s.accel_mm_s2);
            travel_len += d;
        }
        if first {
            push(&mut lines, format!("G1 Z{} F{}", n(cal.z_up), n(s.z_feed)), LineKind::Setup, Some(i), Some([x0, y0, cal.z_up]));
            est += move_time((cal.z_safe - cal.z_up).abs(), v_z, s.accel_mm_s2);
        }
        push(&mut lines, format!("G1 Z{} F{}", n(cal.z_down), n(s.z_feed)), LineKind::PenDown, Some(i), Some([x0, y0, cal.z_down]));
        est += move_time((cal.z_up - cal.z_down).abs(), v_z, s.accel_mm_s2);
        if s.pen_settle_ms > 0 {
            push(&mut lines, format!("G4 P{}", s.pen_settle_ms), LineKind::PenDown, Some(i), None);
            est += s.pen_settle_ms as f64 / 1000.0;
        }

        let mut last = (x0, y0);
        let mut first_seg = true;
        let mut path = 0.0;
        for (k, &(x, y)) in nozzle.iter().enumerate().skip(1) {
            let seg = ((x - last.0).powi(2) + (y - last.1).powi(2)).sqrt();
            let is_end = k == nozzle.len() - 1;
            if seg < s.min_segment_mm && !is_end {
                continue;
            }
            if seg < 1e-6 {
                continue;
            }
            let f = if first_seg { format!(" F{}", n(s.draw_feed)) } else { String::new() };
            push(&mut lines, format!("G1 X{} Y{}{}", n(x), n(y), f), LineKind::Draw, Some(i), Some([x, y, cal.z_down]));
            first_seg = false;
            path += seg;
            last = (x, y);
        }
        draw_len += path;
        // ramp up/down once per continuous stroke
        est += path / v_draw + v_draw / s.accel_mm_s2;

        push(&mut lines, format!("G1 Z{} F{}", n(cal.z_up), n(s.z_feed)), LineKind::PenUp, Some(i), Some([last.0, last.1, cal.z_up]));
        est += move_time((cal.z_up - cal.z_down).abs(), v_z, s.accel_mm_s2);
        push(&mut lines, "M400".into(), LineKind::Checkpoint, Some(i), Some([last.0, last.1, cal.z_up]));
        cur_xy = Some(last);
        nz = cal.z_up;
    }

    let (lx, ly) = cur_xy.unwrap();
    push(&mut lines, format!("G1 Z{} F{}", n(cal.z_safe), n(s.z_feed)), LineKind::Park, None, Some([lx, ly, cal.z_safe]));
    est += move_time((cal.z_safe - cal.z_up).abs(), v_z, s.accel_mm_s2);
    push(&mut lines, "M400".into(), LineKind::Checkpoint, None, Some([lx, ly, cal.z_safe]));

    // Defence in depth: re-parse the finished text against the calibrated envelope.
    verify_program(lines.iter().map(|l| l.text.as_str()), cal, limits)
        .map_err(|Violation { line, reason, .. }| GcodeError::Unsafe { stroke: lines.get(line).and_then(|l| l.stroke), reason })?;

    let stats = ProgramStats {
        strokes: strokes.len(),
        lines: lines.len(),
        draw_length_mm: draw_len,
        travel_length_mm: travel_len,
        pen_lifts: strokes.len(),
        estimated_seconds: est,
    };
    let _ = stroke_length; // (re-exported helper used by callers)
    Ok(Program { lines, stats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::sample_calibration;

    fn cal() -> Resolved {
        sample_calibration().resolve().unwrap()
    }
    fn square(x: f64, y: f64, s: f64) -> Stroke {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]]
    }

    #[test]
    fn generates_valid_program_with_structure() {
        let p = generate(&[square(50.0, 50.0, 20.0)], &cal(), &GcodeSettings::default(), &Limits::default()).unwrap();
        let t: Vec<&str> = p.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(&t[..2], &["G21", "G90"]);
        assert!(t.iter().any(|l| l.starts_with("G1 Z5 ")), "pen-down uses calibrated z_down: {t:?}");
        assert_eq!(*t.last().unwrap(), "M400");
        assert!(p.lines.iter().filter(|l| l.kind == LineKind::PenDown).count() == 1);
        assert!((p.stats.draw_length_mm - 80.0).abs() < 1e-6);
    }

    #[test]
    fn pencil_offset_is_applied_to_nozzle_commands() {
        let c = cal();
        let p = generate(&[square(50.0, 50.0, 10.0)], &c, &GcodeSettings::default(), &Limits::default()).unwrap();
        let (nx, ny) = c.paper_to_nozzle(50.0, 50.0);
        let travel = p.lines.iter().find(|l| l.kind == LineKind::Travel).unwrap();
        assert_eq!(travel.text, format!("G0 X{} Y{} F4800", n(nx), n(ny)));
        assert_ne!((nx, ny), c.paper_to_tip(50.0, 50.0), "nozzle differs from tip when offset != 0");
    }

    #[test]
    fn never_emits_heater_or_extruder_commands() {
        let p = generate(&[square(20.0, 20.0, 50.0)], &cal(), &GcodeSettings::default(), &Limits::default()).unwrap();
        for l in &p.lines {
            assert!(!l.text.contains('E') && !l.text.starts_with("M10") && !l.text.starts_with("M14") && !l.text.starts_with("M19"), "{}", l.text);
        }
    }

    #[test]
    fn stroke_outside_paper_is_refused() {
        let e = generate(&[square(195.0, 50.0, 20.0)], &cal(), &GcodeSettings::default(), &Limits::default()).unwrap_err();
        assert!(matches!(e, GcodeError::Unsafe { stroke: Some(0), .. }), "{e}");
        // inside the paper but inside the margin is also refused
        let e = generate(&[vec![[5.0, 50.0], [50.0, 50.0]]], &cal(), &GcodeSettings::default(), &Limits::default()).unwrap_err();
        assert!(matches!(e, GcodeError::Unsafe { .. }));
    }

    #[test]
    fn excessive_feed_settings_refused() {
        let s = GcodeSettings { draw_feed: 20000.0, ..Default::default() };
        assert!(matches!(generate(&[square(50.0, 50.0, 5.0)], &cal(), &s, &Limits::default()), Err(GcodeError::Settings(_))));
        let s = GcodeSettings { z_feed: 5000.0, ..Default::default() };
        assert!(generate(&[square(50.0, 50.0, 5.0)], &cal(), &s, &Limits::default()).is_err());
    }

    #[test]
    fn empty_input_is_nothing() {
        assert!(matches!(generate(&[], &cal(), &GcodeSettings::default(), &Limits::default()), Err(GcodeError::Nothing)));
        assert!(matches!(generate(&[vec![[1.0, 1.0]]], &cal(), &GcodeSettings::default(), &Limits::default()), Err(GcodeError::Nothing)));
    }

    #[test]
    fn pen_is_lifted_between_disconnected_strokes() {
        let p = generate(&[square(30.0, 30.0, 10.0), square(100.0, 100.0, 10.0)], &cal(), &GcodeSettings::default(), &Limits::default()).unwrap();
        let downs = p.lines.iter().filter(|l| l.kind == LineKind::PenDown).count();
        let ups = p.lines.iter().filter(|l| l.kind == LineKind::PenUp).count();
        assert_eq!(downs, 2);
        assert_eq!(ups, 2);
        // second travel happens at pen-up Z (no Z command between PenUp and Travel lowers it)
        let travels = p.lines.iter().filter(|l| l.kind == LineKind::Travel).count();
        assert_eq!(travels, 2);
        assert!(p.stats.travel_length_mm > 90.0);
    }

    #[test]
    fn estimate_scales_with_speed() {
        let slow = generate(&[square(50.0, 50.0, 50.0)], &cal(), &GcodeSettings { draw_feed: 600.0, ..Default::default() }, &Limits::default()).unwrap();
        let fast = generate(&[square(50.0, 50.0, 50.0)], &cal(), &GcodeSettings { draw_feed: 3000.0, ..Default::default() }, &Limits::default()).unwrap();
        assert!(slow.stats.estimated_seconds > fast.stats.estimated_seconds * 2.0);
        // 200 mm at 10 mm/s ≈ 20 s plus overhead
        assert!(slow.stats.estimated_seconds > 20.0);
    }

    #[test]
    fn number_format_is_compact() {
        assert_eq!(n(1.0), "1");
        assert_eq!(n(1.2300), "1.23");
        assert_eq!(n(-0.0001), "0");
        assert_eq!(n(12.3456), "12.346");
    }
}
