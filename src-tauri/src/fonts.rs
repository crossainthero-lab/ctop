//! Installed-font discovery for the writer. Only .ttf/.otf files (opentype.js cannot read .ttc collections).
//! `read` only ever returns bytes of a path that discovery found (no arbitrary file reads).
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub const MAX_FONT_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FontEntry {
    pub id: String,
    pub family: String,
    pub style: String,
}

struct Found {
    entry: FontEntry,
    path: PathBuf,
}

static CACHE: OnceLock<Vec<Found>> = OnceLock::new();

fn cache() -> &'static Vec<Found> {
    CACHE.get_or_init(discover)
}

/// Stable id: FNV-1a 64 of the path bytes, lowercase hex.
pub fn path_id(p: &Path) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

fn is_font_file(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("ttf") | Some("otf")
    )
}

fn from_fc_list() -> Vec<(PathBuf, String, String)> {
    let out = match Command::new("fc-list").arg("--format").arg("%{file}\t%{family[0]}\t%{style[0]}\n").output() {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };
    parse_fc(&String::from_utf8_lossy(&out.stdout))
}

fn parse_fc(s: &str) -> Vec<(PathBuf, String, String)> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let file = it.next()?.trim();
            let fam = it.next()?.trim();
            let style = it.next().unwrap_or("Regular").trim();
            if file.is_empty() || fam.is_empty() {
                return None;
            }
            let p = PathBuf::from(file);
            is_font_file(&p).then(|| (p, fam.to_string(), if style.is_empty() { "Regular".into() } else { style.to_string() }))
        })
        .collect()
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, depth + 1, out);
        } else if is_font_file(&p) {
            out.push(p);
        }
    }
}

/// Guess family/style from a file name like "DejaVuSerif-BoldItalic.ttf".
fn name_guess(p: &Path) -> (String, String) {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("font");
    match stem.split_once('-') {
        Some((f, s)) => (f.to_string(), s.to_string()),
        None => (stem.to_string(), "Regular".to_string()),
    }
}

fn from_walk() -> Vec<(PathBuf, String, String)> {
    let mut dirs = vec![PathBuf::from("/usr/share/fonts")];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(Path::new(&home).join(".local/share/fonts"));
        dirs.push(Path::new(&home).join(".fonts"));
    }
    let mut files = vec![];
    for d in dirs {
        walk(&d, 0, &mut files);
    }
    files
        .into_iter()
        .map(|p| {
            let (f, s) = name_guess(&p);
            (p, f, s)
        })
        .collect()
}

fn build(mut raw: Vec<(PathBuf, String, String)>) -> Vec<Found> {
    raw.sort_by(|a, b| (&a.1, &a.2, &a.0).cmp(&(&b.1, &b.2, &b.0)));
    let mut seen = HashSet::new();
    let mut out = vec![];
    for (path, family, style) in raw {
        if !seen.insert((family.clone(), style.clone())) {
            continue;
        }
        out.push(Found { entry: FontEntry { id: path_id(&path), family, style }, path });
    }
    out
}

fn discover() -> Vec<Found> {
    let mut raw = from_fc_list();
    if raw.is_empty() {
        raw = from_walk();
    }
    build(raw)
}

pub fn list() -> Vec<FontEntry> {
    cache().iter().map(|f| f.entry.clone()).collect()
}

pub fn read(id: &str) -> Result<Vec<u8>, String> {
    let f = cache().iter().find(|f| f.entry.id == id).ok_or_else(|| format!("unknown font id: {id}"))?;
    let md = std::fs::metadata(&f.path).map_err(|e| format!("font unreadable: {e}"))?;
    if md.len() > MAX_FONT_BYTES {
        return Err(format!("font too large ({} bytes, max {})", md.len(), MAX_FONT_BYTES));
    }
    std::fs::read(&f.path).map_err(|e| format!("font unreadable: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_id_errors() {
        assert!(read("deadbeef").is_err());
        assert!(read("/etc/passwd").is_err());
        assert!(read("../../etc/passwd").is_err());
    }

    #[test]
    fn id_is_stable_and_distinct() {
        let a = path_id(Path::new("/usr/share/fonts/a.ttf"));
        assert_eq!(a, path_id(Path::new("/usr/share/fonts/a.ttf")));
        assert_ne!(a, path_id(Path::new("/usr/share/fonts/b.ttf")));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn parse_and_dedupe() {
        let raw = parse_fc("/x/A.ttf\tFoo\tRegular\n/y/A2.ttf\tFoo\tRegular\n/x/B.ttc\tBar\tRegular\n/x/C.otf\tBaz\tBold\n\n");
        assert_eq!(raw.len(), 3); // .ttc dropped
        let b = build(raw);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].entry.family, "Baz");
        assert_eq!(b[1].entry.family, "Foo");
        assert_eq!(b[1].path, PathBuf::from("/x/A.ttf"));
    }

    #[test]
    fn list_is_cached_and_readable() {
        let a = list();
        assert_eq!(a, list());
        if let Some(f) = a.first() {
            let bytes = read(&f.id).unwrap();
            assert!(bytes.len() > 100);
        }
    }

    #[test]
    fn name_guess_works() {
        assert_eq!(name_guess(Path::new("/a/DejaVuSerif-Bold.ttf")), ("DejaVuSerif".into(), "Bold".into()));
        assert_eq!(name_guess(Path::new("/a/Foo.otf")), ("Foo".into(), "Regular".into()));
    }
}
