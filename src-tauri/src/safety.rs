//! Safety layer. Everything that can reach the serial port is checked here:
//! an allow-list of commands, feedrate caps, and a program verifier that
//! re-parses G-code (independently of the generator) against the calibrated
//! envelope and paper area.

use crate::calibration::{Envelope, FirmwareLimits, Resolved};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    /// Hard caps (mm/min) no setting may exceed.
    pub max_draw_feed: f64,
    pub max_travel_feed: f64,
    pub max_z_feed: f64,
    pub max_jog_feed: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_draw_feed: 6000.0, max_travel_feed: 9000.0, max_z_feed: 1200.0, max_jog_feed: 6000.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub code: String,
    pub args: BTreeMap<char, f64>,
}

/// Parse one G-code line (comments stripped). `Ok(None)` for blank lines.
pub fn parse_line(line: &str) -> Result<Option<Word>, String> {
    let line = line.split(';').next().unwrap_or("").trim();
    if line.is_empty() {
        return Ok(None);
    }
    let mut it = line.split_whitespace();
    let code = it.next().unwrap().to_ascii_uppercase();
    let mut args = BTreeMap::new();
    for tok in it {
        let mut ch = tok.chars();
        let letter = ch.next().unwrap().to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() {
            return Err(format!("Malformed word '{tok}'"));
        }
        let rest: String = ch.collect();
        let val = if rest.is_empty() { 0.0 } else { rest.parse::<f64>().map_err(|_| format!("Bad number in '{tok}'"))? };
        if !val.is_finite() {
            return Err(format!("Non-finite number in '{tok}'"));
        }
        args.insert(letter, val);
    }
    // normalise "G01" → "G1"
    let norm = {
        let (l, n) = code.split_at(1);
        match n.parse::<u32>() {
            Ok(n) => format!("{l}{n}"),
            Err(_) => return Err(format!("Unrecognised command '{code}'")),
        }
    };
    Ok(Some(Word { code: norm, args }))
}

/// Commands the application is permitted to send at all. Notably absent:
/// every heater/fan/extruder command, G91 (relative), G92, probing, M18/M84.
const ALLOWED: &[&str] = &["G0", "G1", "G4", "G21", "G90", "G28", "M17", "M112", "M114", "M115", "M211", "M220", "M400", "M410"];

pub fn check_allowed(line: &str) -> Result<Word, String> {
    let w = parse_line(line)?.ok_or_else(|| "Empty command".to_string())?;
    if !ALLOWED.contains(&w.code.as_str()) {
        return Err(format!("{} is not on the CtoP command allow-list", w.code));
    }
    if w.args.contains_key(&'E') || w.args.contains_key(&'T') || w.args.contains_key(&'S') && w.code != "M220" && w.code != "M17" {
        return Err(format!("{}: extruder/tool/heater words are never sent", w.code));
    }
    if w.code == "M220" {
        let s = w.args.get(&'S').copied().unwrap_or(f64::NAN);
        if !(20.0..=200.0).contains(&s) {
            return Err("M220 speed factor must be 20–200 %".into());
        }
    }
    Ok(w)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Violation {
    /// 0-based line index in the program (or 0 for single commands).
    pub line: usize,
    pub text: String,
    pub reason: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {} ('{}'): {}", self.line + 1, self.text, self.reason)
    }
}

const TOL: f64 = 1e-6;

/// Verify a whole job program against the calibrated machine model.
/// Independent of `gcode::generate`: it only looks at the text.
pub fn verify_program<'a, I: IntoIterator<Item = &'a str>>(lines: I, cal: &Resolved, limits: &Limits) -> Result<(), Violation> {
    let mut pos: [f64; 3] = [f64::NAN; 3];
    let mut absolute = false;
    let mut feed: Option<f64> = None;
    let mut mm = false;
    let env = cal.envelope;
    for (i, text) in lines.into_iter().enumerate() {
        let v = |reason: String| Violation { line: i, text: text.to_string(), reason };
        let w = match check_allowed(text) {
            Ok(w) => w,
            Err(e) => {
                if parse_line(text).map(|w| w.is_none()).unwrap_or(false) {
                    continue;
                }
                return Err(v(e));
            }
        };
        match w.code.as_str() {
            "G21" => mm = true,
            "G90" => absolute = true,
            "G28" | "M17" | "M112" | "M115" | "M211" | "M410" => {
                return Err(v(format!("{} must not appear in a job program", w.code)));
            }
            "G0" | "G1" => {
                if !mm || !absolute {
                    return Err(v("Motion before explicit G21 (mm) and G90 (absolute)".into()));
                }
                if let Some(f) = w.args.get(&'F') {
                    feed = Some(*f);
                }
                let cur = pos;
                let has_xy = w.args.contains_key(&'X') || w.args.contains_key(&'Y');
                let tgt = [
                    w.args.get(&'X').copied().unwrap_or(cur[0]),
                    w.args.get(&'Y').copied().unwrap_or(cur[1]),
                    w.args.get(&'Z').copied().unwrap_or(cur[2]),
                ];
                if tgt[2].is_nan() {
                    return Err(v("Move depends on an unknown Z".into()));
                }
                if tgt[2] < env.z_min - TOL || tgt[2] > env.z_max + TOL {
                    return Err(v(format!("Z{:.3} outside the safe envelope", tgt[2])));
                }
                if !cal.z_higher(tgt[2], cal.z_down) {
                    return Err(v(format!("Z{:.3} is below the calibrated pen-down height {:.3}", tgt[2], cal.z_down)));
                }
                if tgt[0].is_nan() || tgt[1].is_nan() {
                    // lift-first move: XY not yet established, only legal at safe clearance
                    if !cal.z_higher(tgt[2], cal.z_safe) {
                        return Err(v("XY unknown below safe clearance".into()));
                    }
                    check_feed(feed, limits, !has_xy, false).map_err(&v)?;
                    pos = tgt;
                    continue;
                }
                if !env.contains(tgt) {
                    return Err(v(format!("Target X{:.3} Y{:.3} Z{:.3} outside the safe envelope", tgt[0], tgt[1], tgt[2])));
                }
                // below safe clearance the pencil must be over the drawable paper area
                let below_safe = !cal.z_higher(tgt[2], cal.z_safe) || !cal.z_higher(cur[2], cal.z_safe);
                if below_safe {
                    let mut ends = vec![[tgt[0], tgt[1]]];
                    if !cur[0].is_nan() && !cur[1].is_nan() {
                        ends.push([cur[0], cur[1]]);
                    } else {
                        return Err(v("XY unknown below safe clearance".into()));
                    }
                    for p in ends {
                        let (px, py) = cal.tip_to_paper(p[0] + cal.offset.0, p[1] + cal.offset.1);
                        if !cal.in_drawable(px, py) {
                            return Err(v(format!(
                                "Pencil would be off the drawable area ({px:.1},{py:.1} mm) below safe clearance"
                            )));
                        }
                    }
                }
                let z_only = (tgt[0] - cur[0]).abs() < TOL && (tgt[1] - cur[1]).abs() < TOL;
                let pen_down = !cal.z_higher(tgt[2], cal.z_down + 1e-3);
                check_feed(feed, limits, z_only, pen_down).map_err(&v)?;
                pos = tgt;
            }
            "G4" | "M114" | "M400" => {}
            "M220" => {}
            _ => return Err(v("Unhandled command".into())),
        }
    }
    Ok(())
}

fn check_feed(feed: Option<f64>, l: &Limits, z_only: bool, pen_down: bool) -> Result<(), String> {
    let f = feed.ok_or("Motion without an F feedrate")?;
    if f <= 0.0 {
        return Err("Feedrate must be positive".into());
    }
    let cap = if z_only {
        l.max_z_feed
    } else if pen_down {
        l.max_draw_feed
    } else {
        l.max_travel_feed
    };
    if f > cap + TOL {
        return Err(format!("Feedrate F{f} exceeds the {cap} mm/min limit for this move"));
    }
    Ok(())
}

/// Policy for supervised manual jogging.
#[derive(Clone, Copy, Debug)]
pub enum JogPolicy {
    /// Safe envelope defined by the operator.
    Envelope { env: Envelope, z_floor: Option<f64> },
    /// No envelope yet: firmware limits only, with a tiny allowed downward Z step.
    FirmwareOnly { fw: FirmwareLimits },
}

pub const UNCALIBRATED_MAX_Z_DOWN_STEP: f64 = 1.0;

impl JogPolicy {
    /// `z_down_dir`: +1 if increasing Z lowers the head, −1 normally (z_increases_up).
    pub fn check_target(&self, from: [f64; 3], to: [f64; 3], z_increases_up: bool) -> Result<(), String> {
        if to.iter().any(|x| !x.is_finite()) {
            return Err("Target is not finite".into());
        }
        match self {
            JogPolicy::Envelope { env, z_floor } => {
                if !env.contains(to) {
                    return Err(format!(
                        "Target X{:.2} Y{:.2} Z{:.2} is outside the safe envelope",
                        to[0], to[1], to[2]
                    ));
                }
                if let Some(zf) = z_floor {
                    let below = if z_increases_up { to[2] < zf - TOL } else { to[2] > zf + TOL };
                    if below {
                        return Err(format!("Z{:.2} is below the calibrated pen-down height {:.2}", to[2], zf));
                    }
                }
                Ok(())
            }
            JogPolicy::FirmwareOnly { fw } => {
                let e = Envelope { x_min: fw.x_min, x_max: fw.x_max, y_min: fw.y_min, y_max: fw.y_max, z_min: fw.z_min, z_max: fw.z_max };
                if !e.contains(to) {
                    return Err("Target is outside the firmware's software limits".into());
                }
                let down = if z_increases_up { from[2] - to[2] } else { to[2] - from[2] };
                if down > UNCALIBRATED_MAX_Z_DOWN_STEP + TOL {
                    return Err(format!(
                        "Uncalibrated mode: downward Z steps are limited to {UNCALIBRATED_MAX_Z_DOWN_STEP} mm"
                    ));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::sample_calibration;

    fn cal() -> Resolved {
        sample_calibration().resolve().unwrap()
    }

    #[test]
    fn allow_list_blocks_heaters_and_extruder() {
        for bad in ["M104 S200", "M109 S200", "M140 S60", "M190 S60", "M106 S255", "G92 E0", "G1 X1 E5", "G91", "G29", "M84", "M18", "M999", "G30"] {
            assert!(check_allowed(bad).is_err(), "{bad} should be blocked");
        }
        for ok in ["G1 X10 Y10 F1000", "G0 Z20", "M114", "M400", "G21", "G90", "M220 S100", "M410", "M112", "G28 X Y", "g1 x5"] {
            assert!(check_allowed(ok).is_ok(), "{ok} should be allowed");
        }
    }

    #[test]
    fn m220_range_enforced() {
        assert!(check_allowed("M220 S500").is_err());
        assert!(check_allowed("M220 S5").is_err());
    }

    #[test]
    fn program_requires_explicit_modes() {
        let p = ["G1 Z20 F600"];
        assert!(verify_program(p, &cal(), &Limits::default()).is_err());
    }

    #[test]
    fn program_rejects_out_of_envelope_target() {
        let p = ["G21", "G90", "G1 Z20 F600", "G1 X500 Y100 F3000"];
        let e = verify_program(p, &cal(), &Limits::default()).unwrap_err();
        assert!(e.reason.contains("envelope"), "{e}");
    }

    #[test]
    fn program_rejects_pen_below_down_height() {
        let p = ["G21", "G90", "G1 Z20 F600", "G0 X90 Y240 F3000", "G1 Z4 F600"];
        // Z4 is below envelope zMin (5) → envelope error, which is the earlier guard
        assert!(verify_program(p, &cal(), &Limits::default()).is_err());
    }

    #[test]
    fn program_rejects_low_move_off_paper() {
        // Move at pen-up height (Z8 < zSafe 20) to a spot that is inside the envelope but off the paper.
        let p = ["G21", "G90", "G1 Z20 F600", "G0 X300 Y300 F3000", "G1 Z8 F600"];
        let e = verify_program(p, &cal(), &Limits::default()).unwrap_err();
        assert!(e.reason.contains("drawable"), "{e}");
    }

    #[test]
    fn program_rejects_excess_feed() {
        let p = ["G21", "G90", "G1 Z20 F5000"];
        assert!(verify_program(p, &cal(), &Limits::default()).is_err());
    }

    #[test]
    fn program_accepts_valid_sequence() {
        let c = cal();
        // nozzle = tip - pencil offset
        let (nx, ny) = c.paper_to_nozzle(50.0, 50.0);
        let p = vec![
            "G21".to_string(),
            "G90".to_string(),
            "G1 Z20 F600".to_string(),
            format!("G0 X{nx} Y{ny} F4800"),
            "G1 Z8 F600".to_string(),
            "G1 Z5 F600".to_string(),
            format!("G1 X{} Y{} F1800", nx + 10.0, ny),
            "G1 Z8 F600".to_string(),
            "M400".to_string(),
        ];
        verify_program(p.iter().map(|s| s.as_str()), &c, &Limits::default()).unwrap();
    }

    #[test]
    fn jog_policy_envelope_and_floor() {
        let c = cal();
        let p = JogPolicy::Envelope { env: c.envelope, z_floor: Some(c.z_down) };
        assert!(p.check_target([100.0, 100.0, 10.0], [100.0, 100.0, 6.0], true).is_ok());
        assert!(p.check_target([100.0, 100.0, 6.0], [100.0, 100.0, 4.0], true).is_err());
        assert!(p.check_target([100.0, 100.0, 10.0], [400.0, 100.0, 10.0], true).is_err());
        assert!(p.check_target([100.0, 100.0, 10.0], [f64::NAN, 100.0, 10.0], true).is_err());
    }

    #[test]
    fn jog_policy_uncalibrated_limits_downward_z() {
        let fw = FirmwareLimits { x_min: 0.0, x_max: 310.0, y_min: 0.0, y_max: 310.0, z_min: 0.0, z_max: 402.0 };
        let p = JogPolicy::FirmwareOnly { fw };
        assert!(p.check_target([10.0, 10.0, 50.0], [10.0, 10.0, 49.5], true).is_ok());
        assert!(p.check_target([10.0, 10.0, 50.0], [10.0, 10.0, 40.0], true).is_err());
        assert!(p.check_target([10.0, 10.0, 50.0], [10.0, 10.0, 60.0], true).is_ok());
        assert!(p.check_target([10.0, 10.0, 50.0], [320.0, 10.0, 50.0], true).is_err());
    }
}
