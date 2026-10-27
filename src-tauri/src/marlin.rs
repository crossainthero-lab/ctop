//! Parsing of Marlin's serial output.

use crate::calibration::FirmwareLimits;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Rx {
    Ok,
    /// Planner/target position report from M114.
    Position([f64; 3]),
    /// Firmware reset banner.
    Start,
    Busy,
    /// Fatal: firmware halted (kill() / thermal / etc.). Needs a reset.
    Halted(String),
    Error(String),
    Resend(u32),
    UnknownCommand(String),
    FwInfo(BTreeMap<String, String>),
    SoftLimits(FirmwareLimits),
    Echo(String),
    /// `Cap:NAME:0|1` from M115.
    Cap(String, bool),
    Other(String),
}

pub fn parse_rx(raw: &str) -> Rx {
    let line = raw.trim();
    if line == "ok" || line.starts_with("ok ") {
        return Rx::Ok;
    }
    if line == "start" {
        return Rx::Start;
    }
    if let Some(rest) = line.strip_prefix("Error:") {
        let l = rest.to_ascii_lowercase();
        if l.contains("halted") || l.contains("kill()") || l.contains("thermal") || l.contains("mintemp") || l.contains("maxtemp") {
            return Rx::Halted(rest.trim().to_string());
        }
        return Rx::Error(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("Resend:") {
        if let Ok(n) = rest.trim().parse() {
            return Rx::Resend(n);
        }
    }
    if let Some(rest) = line.strip_prefix("Cap:") {
        if let Some((k, v)) = rest.split_once(':') {
            return Rx::Cap(k.trim().to_string(), v.trim() == "1");
        }
    }
    if line.starts_with("FIRMWARE_NAME:") {
        return Rx::FwInfo(parse_m115(line));
    }
    if let Some(rest) = line.strip_prefix("echo:") {
        let r = rest.trim();
        if r.starts_with("busy:") {
            return Rx::Busy;
        }
        if r.starts_with("Unknown command") {
            return Rx::UnknownCommand(r.to_string());
        }
        if r.starts_with("Soft endstops") {
            if let Some(l) = parse_soft_limits(r) {
                return Rx::SoftLimits(l);
            }
        }
        return Rx::Echo(r.to_string());
    }
    if line.starts_with("X:") {
        if let Some(p) = parse_position(line) {
            return Rx::Position(p);
        }
    }
    Rx::Other(line.to_string())
}

fn parse_position(line: &str) -> Option<[f64; 3]> {
    // "X:10.00 Y:20.00 Z:5.00 E:0.00 Count X: 800 Y:1600 Z:2000"
    let head = line.split("Count").next()?;
    let mut out = [f64::NAN; 3];
    for tok in head.split_whitespace() {
        if let Some((k, v)) = tok.split_once(':') {
            let idx = match k {
                "X" => 0,
                "Y" => 1,
                "Z" => 2,
                _ => continue,
            };
            out[idx] = v.parse().ok()?;
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

fn parse_soft_limits(s: &str) -> Option<FirmwareLimits> {
    // "Soft endstops: ON  Min: X0.00 Y0.00 Z0.00  Max: X310.00 Y310.00 Z402.00"
    let min_i = s.find("Min:")?;
    let max_i = s.find("Max:")?;
    let grab = |seg: &str| -> Option<[f64; 3]> {
        let mut o = [f64::NAN; 3];
        for t in seg.split_whitespace() {
            let (c, v) = t.split_at(1);
            let idx = match c {
                "X" => 0,
                "Y" => 1,
                "Z" => 2,
                _ => continue,
            };
            o[idx] = v.parse().ok()?;
        }
        o.iter().all(|v| v.is_finite()).then_some(o)
    };
    let (mn, mx) = if min_i < max_i { (&s[min_i + 4..max_i], &s[max_i + 4..]) } else { (&s[min_i + 4..], &s[max_i + 4..min_i]) };
    let (a, b) = (grab(mn)?, grab(mx)?);
    Some(FirmwareLimits { x_min: a[0], y_min: a[1], z_min: a[2], x_max: b[0], y_max: b[1], z_max: b[2] })
}

/// Split `KEY:value KEY2:value two words KEY3:...` where keys are UPPER_SNAKE.
pub fn parse_m115(line: &str) -> BTreeMap<String, String> {
    let mut keys: Vec<(usize, usize)> = Vec::new(); // (key start, colon)
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if (i == 0 || b[i - 1] == b' ') && b[i].is_ascii_uppercase() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j] == b'_' || b[j].is_ascii_digit()) {
                j += 1;
            }
            if j < b.len() && b[j] == b':' && j - i >= 3 {
                keys.push((i, j));
                i = j;
            }
        }
        i += 1;
    }
    let mut map = BTreeMap::new();
    for (n, &(s, c)) in keys.iter().enumerate() {
        let end = keys.get(n + 1).map_or(line.len(), |k| k.0);
        map.insert(line[s..c].to_string(), line[c + 1..end].trim().to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ok_and_start() {
        assert_eq!(parse_rx("ok"), Rx::Ok);
        assert_eq!(parse_rx("ok T:21.0 /0.0"), Rx::Ok);
        assert_eq!(parse_rx("start"), Rx::Start);
    }

    #[test]
    fn parses_position() {
        assert_eq!(parse_rx("X:10.00 Y:20.50 Z:5.00 E:0.00 Count X: 800 Y:1640 Z:2000"), Rx::Position([10.0, 20.5, 5.0]));
        assert!(matches!(parse_rx("X:abc Y:1 Z:2"), Rx::Other(_)));
    }

    #[test]
    fn parses_errors_and_halt() {
        assert!(matches!(parse_rx("Error:Printer halted. kill() called!"), Rx::Halted(_)));
        assert!(matches!(parse_rx("Error:Thermal Runaway, system stopped!"), Rx::Halted(_)));
        assert!(matches!(parse_rx("Error:checksum mismatch, Last Line: 4"), Rx::Error(_)));
        assert_eq!(parse_rx("Resend: 12"), Rx::Resend(12));
        assert!(matches!(parse_rx("echo:Unknown command: \"M9999\""), Rx::UnknownCommand(_)));
        assert_eq!(parse_rx("echo:busy: processing"), Rx::Busy);
    }

    #[test]
    fn parses_m115() {
        let r = parse_rx("FIRMWARE_NAME:Marlin Ver 1.60.9 (Creality3D) SOURCE_CODE_URL:https://github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:CR-10S Pro EXTRUDER_COUNT:1");
        match r {
            Rx::FwInfo(m) => {
                assert_eq!(m["FIRMWARE_NAME"], "Marlin Ver 1.60.9 (Creality3D)");
                assert_eq!(m["MACHINE_TYPE"], "CR-10S Pro");
                assert_eq!(m["EXTRUDER_COUNT"], "1");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn parses_capabilities() {
        assert_eq!(parse_rx("Cap:EMERGENCY_PARSER:1"), Rx::Cap("EMERGENCY_PARSER".into(), true));
        assert_eq!(parse_rx("Cap:AUTOREPORT_TEMP:0"), Rx::Cap("AUTOREPORT_TEMP".into(), false));
    }

    #[test]
    fn parses_soft_limits() {
        match parse_rx("echo:Soft endstops: ON  Min: X0.00 Y0.00 Z0.00  Max: X310.00 Y310.00 Z402.00") {
            Rx::SoftLimits(l) => {
                assert_eq!((l.x_max, l.y_max, l.z_max), (310.0, 310.0, 402.0));
                assert_eq!(l.x_min, 0.0);
            }
            o => panic!("{o:?}"),
        }
    }
}
