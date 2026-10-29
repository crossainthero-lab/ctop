//! Application log ring + diagnostics redaction. Everything here comes from real events; nothing is synthesised.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub t_ms: u128,
    pub level: &'static str,
    pub src: String,
    pub msg: String,
}

static LOG: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());
const CAP: usize = 1500;

pub fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn log(level: &'static str, src: &str, msg: impl Into<String>) {
    let mut g = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= CAP {
        g.pop_front();
    }
    g.push_back(Entry { t_ms: now_ms(), level, src: src.into(), msg: msg.into() });
}
pub fn info(src: &str, msg: impl Into<String>) {
    log("info", src, msg)
}
pub fn warn(src: &str, msg: impl Into<String>) {
    log("warn", src, msg)
}
pub fn error(src: &str, msg: impl Into<String>) {
    log("error", src, msg)
}

pub fn recent(n: usize) -> Vec<Entry> {
    let g = LOG.lock().unwrap_or_else(|e| e.into_inner());
    g.iter().skip(g.len().saturating_sub(n)).cloned().collect()
}

/// Panics become log entries (with location + backtrace) so Technical View can show the real stack.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        error("panic", format!("{info}\n{bt}"));
        prev(info);
    }));
}

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["token", "secret", "password", "passwd", "credential", "apikey", "api_key", "authorization", "cookie", "private"].iter().any(|s| k.contains(s))
}

/// Redact secret-looking values in JSON (by key name) and in all strings.
pub fn redact_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), if is_secret_key(k) && !x.is_null() { Value::String("[redacted]".into()) } else { redact_json(x) })).collect()),
        Value::Array(a) => Value::Array(a.iter().map(redact_json).collect()),
        Value::String(s) => Value::String(redact(s)),
        o => o.clone(),
    }
}

/// Redact secrets in free text: `key=value` / `key: value` for secret-looking keys, `Bearer xxx`, long opaque tokens
/// (hex/base64/sk-..., >= 28 chars) and the home directory.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let home = std::env::var("HOME").unwrap_or_default();
    let s = if home.len() > 1 { s.replace(&home, "~") } else { s.to_string() };
    let mut prev_secret_key = false;
    let mut prev_bearer = false;
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // keep whitespace, process words
        let mut word = String::new();
        let flush = |w: &mut String, out: &mut String, prev_secret_key: &mut bool, prev_bearer: &mut bool| {
            if w.is_empty() {
                return;
            }
            let lw = w.to_ascii_lowercase();
            let mut text = w.clone();
            if *prev_secret_key && (lw == "bearer" || lw == "basic") {
                // "Authorization: Bearer <token>": keep the scheme, hide the credential that follows
                *prev_secret_key = false;
                *prev_bearer = true;
            } else if *prev_secret_key || *prev_bearer {
                text = "[redacted]".into();
                *prev_secret_key = false;
                *prev_bearer = false;
            } else if let Some(pos) = w.find(['=', ':']).filter(|&p| p > 0 && p + 1 < w.len()) {
                if is_secret_key(&w[..pos]) {
                    text = format!("{}{}[redacted]", &w[..pos], &w[pos..pos + 1]);
                }
            } else if is_secret_key(w.trim_end_matches([':', '='])) && (w.ends_with(':') || w.ends_with('=')) {
                *prev_secret_key = true;
            } else if lw == "bearer" || lw == "basic" {
                *prev_bearer = true;
            }
            if looks_opaque(&text) {
                text = "[redacted]".into();
            }
            out.push_str(&text);
            w.clear();
        };
        for ch in line.chars() {
            if ch.is_whitespace() {
                flush(&mut word, &mut out, &mut prev_secret_key, &mut prev_bearer);
                out.push(ch);
            } else {
                word.push(ch);
            }
        }
        flush(&mut word, &mut out, &mut prev_secret_key, &mut prev_bearer);
    }
    out
}

fn looks_opaque(w: &str) -> bool {
    let core = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if core.starts_with("sk-") && core.len() > 12 {
        return true;
    }
    core.len() >= 28 && core.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+' || c == '=') && core.chars().any(|c| c.is_ascii_digit()) && core.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_keys_and_tokens() {
        let t = redact("curl -H 'Authorization: Bearer abcDEF123' token=hunter2 password: swordfish ok");
        assert!(!t.contains("abcDEF123") && !t.contains("hunter2") && !t.contains("swordfish"), "{t}");
        assert!(t.contains("ok"));
        assert_eq!(redact("job 123 stroke 7 failed"), "job 123 stroke 7 failed");
        assert!(redact("key 3f9a1c0b77d24e5a8b6c9d0e1f2a3b4c5d6e7f80").contains("[redacted]"));
        assert!(redact("sk-ant-api03-abcdefghijk").contains("[redacted]"));
    }

    #[test]
    fn redacts_json_by_key() {
        let v = redact_json(&json!({"webToken": "x1", "sock": "/run/user/1000/pend.sock", "n": {"apiKey": "k", "pos": [1, 2]}}));
        assert_eq!(v["webToken"], "[redacted]");
        assert_eq!(v["n"]["apiKey"], "[redacted]");
        assert_eq!(v["sock"], "/run/user/1000/pend.sock");
        assert_eq!(v["n"]["pos"], json!([1, 2]));
    }

    #[test]
    fn log_ring_keeps_recent() {
        info("test", "hello");
        assert!(recent(5).iter().any(|e| e.msg == "hello"));
    }
}
