//! Calibration storage and the paper → machine coordinate transform.
//!
//! Three coordinate spaces are kept strictly apart:
//!  * **paper**  – millimetres on the sheet, origin top-left, +y down the sheet.
//!  * **tip**    – machine coordinates of the *pencil tip*.
//!  * **nozzle** – machine coordinates of the *carriage/nozzle*; this is what the
//!    firmware positions and what its travel limits and our safe envelope apply to.
//!
//! `tip = nozzle + pencil_offset`, so `nozzle = tip - pencil_offset`.
//! Nothing here has a physical default: every measured value is `Option` and
//! stays `None` until the operator enters or imports it.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Margins {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

/// Safe working envelope for the *nozzle/carriage* in machine coordinates.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvelopeDraft {
    pub x_min: Option<f64>,
    pub x_max: Option<f64>,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    pub z_min: Option<f64>,
    pub z_max: Option<f64>,
}

/// What the operator has physically verified. Drawing stays disabled until all are true.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Verified {
    pub offsets: bool,
    pub z: bool,
    pub paper: bool,
    pub envelope: bool,
    pub orientation: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Calibration {
    /// Pencil tip X minus nozzle X (mm).
    pub pencil_offset_x: Option<f64>,
    pub pencil_offset_y: Option<f64>,
    /// Machine Z at which the pencil touches the paper.
    pub z_down: Option<f64>,
    /// Machine Z for pen-up travel across the paper.
    pub z_up: Option<f64>,
    /// Machine Z for approach, parking and long travel (safe clearance).
    pub z_safe: Option<f64>,
    /// Machine X/Y of the pencil *tip* at paper (0,0).
    pub paper_origin_x: Option<f64>,
    pub paper_origin_y: Option<f64>,
    /// Physical sheet size as laid on the bed.
    pub paper_width: Option<f64>,
    pub paper_height: Option<f64>,
    /// Paper +x runs toward machine −X.
    pub flip_x: bool,
    /// Paper +y (down the sheet) runs toward machine −Y.
    pub flip_y: bool,
    /// Paper x maps onto machine Y and vice versa.
    pub swap_xy: bool,
    /// Increasing machine Z raises the head. Verify on your machine.
    pub z_increases_up: bool,
    pub margins: Margins,
    pub envelope: EnvelopeDraft,
    pub verified: Verified,
    pub notes: String,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration {
            pencil_offset_x: None,
            pencil_offset_y: None,
            z_down: None,
            z_up: None,
            z_safe: None,
            paper_origin_x: None,
            paper_origin_y: None,
            paper_width: None,
            paper_height: None,
            flip_x: false,
            flip_y: true,
            swap_xy: false,
            z_increases_up: true,
            margins: Margins { left: 10.0, right: 10.0, top: 10.0, bottom: 10.0 },
            envelope: EnvelopeDraft::default(),
            verified: Verified::default(),
            notes: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub severity: Severity,
    pub field: String,
    pub message: String,
}

impl Issue {
    fn err(field: &str, message: impl Into<String>) -> Issue {
        Issue { severity: Severity::Error, field: field.into(), message: message.into() }
    }
    fn warn(field: &str, message: impl Into<String>) -> Issue {
        Issue { severity: Severity::Warning, field: field.into(), message: message.into() }
    }
}

/// Firmware-reported software limits (M211). Not the same as the safe envelope.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareLimits {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub z_min: f64,
    pub z_max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub z_min: f64,
    pub z_max: f64,
}

impl Envelope {
    pub fn contains(&self, p: [f64; 3]) -> bool {
        const T: f64 = 1e-6;
        p.iter().all(|v| v.is_finite())
            && p[0] >= self.x_min - T
            && p[0] <= self.x_max + T
            && p[1] >= self.y_min - T
            && p[1] <= self.y_max + T
            && p[2] >= self.z_min - T
            && p[2] <= self.z_max + T
    }
}

impl Calibration {
    /// Envelope only, if all six bounds are set and ordered.
    pub fn envelope(&self) -> Option<Envelope> {
        let e = &self.envelope;
        let env = Envelope {
            x_min: e.x_min?,
            x_max: e.x_max?,
            y_min: e.y_min?,
            y_max: e.y_max?,
            z_min: e.z_min?,
            z_max: e.z_max?,
        };
        (env.x_min < env.x_max && env.y_min < env.y_max && env.z_min < env.z_max).then_some(env)
    }

    /// Check the envelope alone (enough to allow supervised manual jogging).
    pub fn envelope_issues(&self, fw: Option<&FirmwareLimits>) -> Vec<Issue> {
        let mut v = Vec::new();
        let e = &self.envelope;
        let fields = [
            ("envelope.xMin", e.x_min),
            ("envelope.xMax", e.x_max),
            ("envelope.yMin", e.y_min),
            ("envelope.yMax", e.y_max),
            ("envelope.zMin", e.z_min),
            ("envelope.zMax", e.z_max),
        ];
        let mut missing = false;
        for (f, val) in fields {
            match val {
                None => {
                    missing = true;
                    v.push(Issue::err(f, "Not set"));
                }
                Some(x) if !x.is_finite() => v.push(Issue::err(f, "Not a finite number")),
                _ => {}
            }
        }
        if !missing {
            match self.envelope() {
                None => v.push(Issue::err("envelope", "Each minimum must be below its maximum")),
                Some(env) => {
                    if let Some(fw) = fw {
                        if env.x_min < fw.x_min || env.x_max > fw.x_max {
                            v.push(Issue::err("envelope.x", "X envelope exceeds firmware software limits"));
                        }
                        if env.y_min < fw.y_min || env.y_max > fw.y_max {
                            v.push(Issue::err("envelope.y", "Y envelope exceeds firmware software limits"));
                        }
                        if env.z_min < fw.z_min || env.z_max > fw.z_max {
                            v.push(Issue::err("envelope.z", "Z envelope exceeds firmware software limits"));
                        }
                    }
                }
            }
        }
        v
    }

    /// Full validation. Errors block drawing; unverified items block it separately
    /// (see `ready_for_drawing`).
    pub fn validate(&self, fw: Option<&FirmwareLimits>) -> Vec<Issue> {
        let mut v = self.envelope_issues(fw);
        let reqs: [(&str, Option<f64>); 9] = [
            ("pencilOffsetX", self.pencil_offset_x),
            ("pencilOffsetY", self.pencil_offset_y),
            ("zDown", self.z_down),
            ("zUp", self.z_up),
            ("zSafe", self.z_safe),
            ("paperOriginX", self.paper_origin_x),
            ("paperOriginY", self.paper_origin_y),
            ("paperWidth", self.paper_width),
            ("paperHeight", self.paper_height),
        ];
        for (f, val) in reqs {
            match val {
                None => v.push(Issue::err(f, "Not set — measure and enter this value")),
                Some(x) if !x.is_finite() => v.push(Issue::err(f, "Not a finite number")),
                _ => {}
            }
        }
        if let (Some(zd), Some(zu), Some(zs)) = (self.z_down, self.z_up, self.z_safe) {
            let s = if self.z_increases_up { 1.0 } else { -1.0 };
            if (zu - zd) * s <= 0.0 {
                v.push(Issue::err("zUp", "Pen-up Z must be on the raised side of pen-down Z"));
            } else if (zu - zd) * s < 1.0 {
                v.push(Issue::warn("zUp", "Pen-up clearance under 1 mm — paper curl or pencil wear may drag"));
            }
            if (zs - zu) * s < 0.0 {
                v.push(Issue::err("zSafe", "Safe clearance Z must not be below pen-up Z"));
            }
            if let Some(env) = self.envelope() {
                let (lo, hi) = (env.z_min, env.z_max);
                for (f, z) in [("zDown", zd), ("zUp", zu), ("zSafe", zs)] {
                    if z < lo || z > hi {
                        v.push(Issue::err(f, "Outside the safe Z envelope"));
                    }
                }
            }
        }
        let m = &self.margins;
        if [m.left, m.right, m.top, m.bottom].iter().any(|x| !x.is_finite() || *x < 0.0) {
            v.push(Issue::err("margins", "Margins must be finite and non-negative"));
        }
        if let (Some(w), Some(h)) = (self.paper_width, self.paper_height) {
            if w <= 0.0 || h <= 0.0 {
                v.push(Issue::err("paperWidth", "Paper size must be positive"));
            } else if w - m.left - m.right <= 0.0 || h - m.top - m.bottom <= 0.0 {
                v.push(Issue::err("margins", "Margins leave no drawable area"));
            }
        }
        for (f, o) in [("pencilOffsetX", self.pencil_offset_x), ("pencilOffsetY", self.pencil_offset_y)] {
            if let Some(o) = o {
                if o.abs() > 100.0 {
                    v.push(Issue::warn(f, "Offset over 100 mm — check the measurement"));
                }
            }
        }
        // The whole drawable rectangle must be reachable by the nozzle inside the envelope.
        if v.iter().all(|i| i.severity != Severity::Error) {
            if let Ok(r) = self.resolve() {
                for (px, py) in r.drawable_corners() {
                    let (nx, ny) = r.paper_to_nozzle(px, py);
                    let env = r.envelope;
                    if nx < env.x_min || nx > env.x_max || ny < env.y_min || ny > env.y_max {
                        v.push(Issue::err(
                            "envelope",
                            format!(
                                "Paper corner ({px:.0},{py:.0}) needs nozzle at X{nx:.1} Y{ny:.1}, outside the safe envelope"
                            ),
                        ));
                        break;
                    }
                }
            }
        }
        v
    }

    /// Drawing may only be enabled when there are no errors and everything is verified.
    pub fn ready_for_drawing(&self, fw: Option<&FirmwareLimits>) -> Result<Resolved, Vec<String>> {
        let issues = self.validate(fw);
        let mut why: Vec<String> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("{}: {}", i.field, i.message))
            .collect();
        let ver = &self.verified;
        for (ok, name) in [
            (ver.offsets, "pencil offsets"),
            (ver.z, "Z heights"),
            (ver.paper, "paper origin and size"),
            (ver.envelope, "safe envelope"),
            (ver.orientation, "axis orientation"),
        ] {
            if !ok {
                why.push(format!("Operator has not verified {name}"));
            }
        }
        if !why.is_empty() {
            return Err(why);
        }
        self.resolve().map_err(|e| e.into_iter().map(|i| i.message).collect())
    }

    /// Resolve into a fully-specified transform. Errors if any value is missing.
    pub fn resolve(&self) -> Result<Resolved, Vec<Issue>> {
        let missing = |f: &str| vec![Issue::err(f, "Not set")];
        let env = self.envelope().ok_or_else(|| missing("envelope"))?;
        Ok(Resolved {
            offset: (
                self.pencil_offset_x.ok_or_else(|| missing("pencilOffsetX"))?,
                self.pencil_offset_y.ok_or_else(|| missing("pencilOffsetY"))?,
            ),
            z_down: self.z_down.ok_or_else(|| missing("zDown"))?,
            z_up: self.z_up.ok_or_else(|| missing("zUp"))?,
            z_safe: self.z_safe.ok_or_else(|| missing("zSafe"))?,
            origin: (
                self.paper_origin_x.ok_or_else(|| missing("paperOriginX"))?,
                self.paper_origin_y.ok_or_else(|| missing("paperOriginY"))?,
            ),
            paper: (
                self.paper_width.ok_or_else(|| missing("paperWidth"))?,
                self.paper_height.ok_or_else(|| missing("paperHeight"))?,
            ),
            flip_x: self.flip_x,
            flip_y: self.flip_y,
            swap_xy: self.swap_xy,
            z_increases_up: self.z_increases_up,
            margins: self.margins.clone(),
            envelope: env,
        })
    }
}

/// A fully specified, immutable calibration. Only obtainable from `Calibration::resolve`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub offset: (f64, f64),
    pub z_down: f64,
    pub z_up: f64,
    pub z_safe: f64,
    pub origin: (f64, f64),
    pub paper: (f64, f64),
    pub flip_x: bool,
    pub flip_y: bool,
    pub swap_xy: bool,
    pub z_increases_up: bool,
    pub margins: Margins,
    pub envelope: Envelope,
}

impl Resolved {
    /// Paper mm → pencil-tip machine XY.
    pub fn paper_to_tip(&self, px: f64, py: f64) -> (f64, f64) {
        let (u, v) = if self.swap_xy { (py, px) } else { (px, py) };
        (
            self.origin.0 + if self.flip_x { -u } else { u },
            self.origin.1 + if self.flip_y { -v } else { v },
        )
    }

    /// Inverse of `paper_to_tip`.
    pub fn tip_to_paper(&self, mx: f64, my: f64) -> (f64, f64) {
        let u = (mx - self.origin.0) * if self.flip_x { -1.0 } else { 1.0 };
        let v = (my - self.origin.1) * if self.flip_y { -1.0 } else { 1.0 };
        if self.swap_xy { (v, u) } else { (u, v) }
    }

    /// Paper mm → nozzle machine XY (what the firmware is commanded to).
    pub fn paper_to_nozzle(&self, px: f64, py: f64) -> (f64, f64) {
        let (tx, ty) = self.paper_to_tip(px, py);
        (tx - self.offset.0, ty - self.offset.1)
    }

    /// The paper-space rectangle the pencil may touch: (x0, y0, x1, y1).
    pub fn drawable_rect(&self) -> (f64, f64, f64, f64) {
        (
            self.margins.left,
            self.margins.top,
            self.paper.0 - self.margins.right,
            self.paper.1 - self.margins.bottom,
        )
    }

    pub fn drawable_corners(&self) -> [(f64, f64); 4] {
        let (x0, y0, x1, y1) = self.drawable_rect();
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    pub fn in_drawable(&self, px: f64, py: f64) -> bool {
        const T: f64 = 1e-6;
        let (x0, y0, x1, y1) = self.drawable_rect();
        px.is_finite() && py.is_finite() && px >= x0 - T && px <= x1 + T && py >= y0 - T && py <= y1 + T
    }

    /// Z on the raised side of `a` compared to `b`?
    pub fn z_higher(&self, a: f64, b: f64) -> bool {
        if self.z_increases_up { a >= b } else { a <= b }
    }
}

#[cfg(test)]
pub fn sample_calibration() -> Calibration {
    Calibration {
        pencil_offset_x: Some(-30.0),
        pencil_offset_y: Some(10.0),
        z_down: Some(5.0),
        z_up: Some(8.0),
        z_safe: Some(20.0),
        paper_origin_x: Some(60.0),
        paper_origin_y: Some(300.0),
        paper_width: Some(210.0),
        paper_height: Some(297.0),
        envelope: EnvelopeDraft {
            x_min: Some(0.0),
            x_max: Some(310.0),
            y_min: Some(0.0),
            y_max: Some(310.0),
            z_min: Some(5.0),
            z_max: Some(60.0),
        },
        verified: Verified { offsets: true, z: true, paper: true, envelope: true, orientation: true },
        ..Calibration::default()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::sample_calibration as sample;

    #[test]
    fn empty_calibration_is_not_resolvable_or_ready() {
        let c = Calibration::default();
        assert!(c.resolve().is_err());
        assert!(c.ready_for_drawing(None).is_err());
        assert!(c.envelope().is_none());
    }

    #[test]
    fn transform_roundtrip_all_orientations() {
        for (fx, fy, sw) in [(false, false, false), (true, false, false), (false, true, false), (true, true, true), (false, false, true)] {
            let mut c = sample();
            c.flip_x = fx;
            c.flip_y = fy;
            c.swap_xy = sw;
            let r = c.resolve().unwrap();
            for (px, py) in [(0.0, 0.0), (12.5, 40.0), (100.0, 200.0)] {
                let (mx, my) = r.paper_to_tip(px, py);
                let (bx, by) = r.tip_to_paper(mx, my);
                assert!((bx - px).abs() < 1e-9 && (by - py).abs() < 1e-9, "{fx} {fy} {sw}");
            }
        }
    }

    #[test]
    fn pencil_offset_shifts_nozzle_not_tip() {
        let r = sample().resolve().unwrap();
        // paper origin: tip must land on the configured paper origin
        let (tx, ty) = r.paper_to_tip(0.0, 0.0);
        assert_eq!((tx, ty), (60.0, 300.0));
        // the nozzle is commanded to tip - offset
        let (nx, ny) = r.paper_to_nozzle(0.0, 0.0);
        assert_eq!((nx, ny), (60.0 + 30.0, 300.0 - 10.0));
    }

    #[test]
    fn default_orientation_paper_y_down_is_machine_y_minus() {
        let r = sample().resolve().unwrap();
        let (_, y0) = r.paper_to_tip(0.0, 0.0);
        let (_, y1) = r.paper_to_tip(0.0, 100.0);
        assert!(y1 < y0);
    }

    #[test]
    fn complete_verified_calibration_is_ready() {
        assert!(sample().ready_for_drawing(None).is_ok());
    }

    #[test]
    fn unverified_blocks_drawing() {
        let mut c = sample();
        c.verified.z = false;
        let e = c.ready_for_drawing(None).unwrap_err();
        assert!(e.iter().any(|m| m.contains("Z heights")));
    }

    #[test]
    fn z_ordering_errors() {
        let mut c = sample();
        c.z_up = Some(4.0); // below z_down
        assert!(c.validate(None).iter().any(|i| i.field == "zUp" && i.severity == Severity::Error));
        let mut c = sample();
        c.z_increases_up = false; // now "up" must be lower Z; sample violates it
        assert!(c.validate(None).iter().any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn paper_corner_outside_envelope_is_error() {
        let mut c = sample();
        c.paper_origin_y = Some(20.0); // pushes the sheet off the low-Y end
        let issues = c.validate(None);
        assert!(issues.iter().any(|i| i.field == "envelope" && i.message.contains("corner")));
    }

    #[test]
    fn envelope_beyond_firmware_limits_is_error() {
        let fw = FirmwareLimits { x_min: 0.0, x_max: 300.0, y_min: 0.0, y_max: 310.0, z_min: 0.0, z_max: 402.0 };
        assert!(sample().validate(Some(&fw)).iter().any(|i| i.field == "envelope.x"));
    }

    #[test]
    fn drawable_rect_respects_margins() {
        let r = sample().resolve().unwrap();
        assert_eq!(r.drawable_rect(), (10.0, 10.0, 200.0, 287.0));
        assert!(r.in_drawable(10.0, 10.0));
        assert!(!r.in_drawable(9.0, 10.0));
        assert!(!r.in_drawable(f64::NAN, 10.0));
    }

    #[test]
    fn json_roundtrip_keeps_unset_values_unset() {
        let c = Calibration::default();
        let s = serde_json::to_string(&c).unwrap();
        let back: Calibration = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
        assert!(back.z_down.is_none());
    }
}
