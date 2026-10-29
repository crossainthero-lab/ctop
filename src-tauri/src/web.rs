//! Local web service: serves the same Svelte frontend and the same command set as the desktop window.
//! Loopback only; every API call needs the access token; Host/Origin are checked against DNS-rebinding.

use crate::diag;
use crate::writer::{self, Writer};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 7795;
const MAX_BODY: usize = 8 << 20;

/// Read or create the access token (32 random bytes, hex, mode 0600).
pub fn load_token(dir: &Path) -> Result<String, String> {
    let p = dir.join("web_token");
    if let Ok(t) = fs::read_to_string(&p) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let mut b = [0u8; 32];
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("no randomness: {e}"))?;
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p).map_err(|e| e.to_string())?;
        f.write_all(t.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(t)
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn dist_dir() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if let Ok(d) = std::env::var("CTOP_DIST") {
        c.push(d.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut p = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..5 {
            if let Some(d) = p {
                c.push(d.join("dist"));
                p = d.parent().map(|p| p.to_path_buf());
            } else {
                break;
            }
        }
    }
    c.push(PathBuf::from("dist"));
    c.into_iter().find(|d| d.join("index.html").exists())
}

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Req {
    fn h(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

fn read_req(s: &mut TcpStream) -> Result<Req, String> {
    s.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 64 * 1024 {
            return Err("headers too large".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or("empty")?;
    let mut it = first.split_whitespace();
    let (method, path) = (it.next().ok_or("bad request")?.to_string(), it.next().ok_or("bad request")?.to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))).collect();
    let len: usize = headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("truncated body".into());
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Ok(Req { method, path, headers, body })
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn host_ok(r: &Req, port: u16) -> bool {
    let Some(h) = r.h("host") else { return false };
    let lan = lan_enabled();
    // LAN mode: accept an IP-literal or Tailscale MagicDNS (.ts.net) host on our port (a DNS-rebinding page can't present either); otherwise loopback names only.
    let ok_host = |h: &str| {
        if h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}") {
            return true;
        }
        lan && h.strip_suffix(&format!(":{port}")).map(|n| n.parse::<Ipv4Addr>().is_ok() || n.ends_with(".ts.net")).unwrap_or(false)
    };
    if !ok_host(h) {
        return false;
    }
    match r.h("origin") {
        None => true,
        Some(o) => o.strip_prefix("http://").map(ok_host).unwrap_or(false),
    }
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// The page itself carries the access token for a browser on this machine (loopback peer; Host/Origin were already checked
/// and a foreign web page cannot read this response), so opening http://localhost:7795 just works. Peers on the LAN never
/// get the token embedded: it must be passed in the address as before.
fn with_token(html: &[u8], token: &str) -> Vec<u8> {
    let h = String::from_utf8_lossy(html);
    let meta = format!("<meta name=\"ctop-token\" content=\"{token}\" />");
    match h.find("</head>") {
        Some(i) => format!("{}{}{}", &h[..i], meta, &h[i..]).into_bytes(),
        None => html.to_vec(),
    }
}

fn serve_static(s: &mut TcpStream, dist: &Path, path: &str, token: Option<&str>) {
    let p = path.split(['?', '#']).next().unwrap_or("/");
    let rel = if p == "/" { "index.html" } else { p.trim_start_matches('/') };
    if rel.split('/').any(|c| c == ".." || c.contains('\\') || c.contains('\0')) {
        return respond(s, 403, "text/plain", b"forbidden");
    }
    let full = dist.join(rel);
    match (fs::canonicalize(&full), fs::canonicalize(dist)) {
        (Ok(f), Ok(d)) if f.starts_with(&d) && f.is_file() => match fs::read(&f) {
            Ok(b) => match token.filter(|_| f.file_name().map(|n| n == "index.html").unwrap_or(false)) {
                Some(t) => respond(s, 200, mime(&f), &with_token(&b, t)),
                None => respond(s, 200, mime(&f), &b),
            },
            Err(_) => respond(s, 404, "text/plain", b"not found"),
        },
        _ => respond(s, 404, "text/plain", b"not found"),
    }
}

fn handle(mut s: TcpStream, w: &Arc<Writer>, token: &str, port: u16, dist: &Option<PathBuf>) {
    let req = match read_req(&mut s) {
        Ok(r) => r,
        Err(e) => {
            if e != "closed" {
                respond(&mut s, 400, "text/plain", e.as_bytes());
            }
            return;
        }
    };
    // only a browser on this very machine (loopback peer) gets the token inside the page; LAN peers must be given it explicitly
    let local_peer = s.peer_addr().map(|a| a.ip().is_loopback()).unwrap_or(false);
    if !host_ok(&req, port) {
        diag::warn("web", format!("rejected request with Host {:?} Origin {:?}", req.h("host"), req.h("origin")));
        return respond(&mut s, 403, "text/plain", b"bad Host/Origin");
    }
    if req.path.starts_with("/api/") {
        if req.method != "POST" || req.path != "/api/call" {
            return respond(&mut s, 405, "text/plain", b"POST /api/call only");
        }
        if !req.h("x-ctop-token").map(|t| ct_eq(t, token)).unwrap_or(false) {
            diag::warn("web", "API call without a valid token");
            return respond(&mut s, 401, "application/json", br#"{"ok":false,"error":"missing or wrong access token"}"#);
        }
        let parsed: Result<Value, _> = serde_json::from_slice(&req.body);
        let (code, out) = match parsed {
            Ok(v) => {
                let cmd = v["cmd"].as_str().unwrap_or("").to_string();
                match writer::call(w, &cmd, v.get("args").cloned().unwrap_or(Value::Null)) {
                    Ok(r) => (200, json!({ "ok": true, "result": r })),
                    Err(e) => (200, json!({ "ok": false, "error": e })),
                }
            }
            Err(e) => (400, json!({ "ok": false, "error": format!("bad request: {e}") })),
        };
        return respond(&mut s, code, "application/json", out.to_string().as_bytes());
    }
    if req.method != "GET" {
        return respond(&mut s, 405, "text/plain", b"method not allowed");
    }
    match dist {
        Some(d) => serve_static(&mut s, d, &req.path, if local_peer { Some(token) } else { None }),
        None => respond(&mut s, 404, "text/plain", b"frontend not built: run `npx vite build` (or set CTOP_DIST)"),
    }
}

/// Opt-in LAN exposure: `CTOP_WEB_BIND=0.0.0.0` (or a specific interface address). Default is loopback only.
fn bind_addr() -> Ipv4Addr {
    std::env::var("CTOP_WEB_BIND").ok().and_then(|v| v.parse().ok()).unwrap_or(Ipv4Addr::LOCALHOST)
}

fn lan_enabled() -> bool {
    !bind_addr().is_loopback()
}

/// Tailscale addresses live in the CGNAT range 100.64.0.0/10.
fn is_tailscale(v: Ipv4Addr) -> bool {
    let o = v.octets();
    o[0] == 100 && (o[1] & 0xC0) == 64
}

fn peer_ok(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => v.is_loopback() || (lan_enabled() && (v.is_private() || v.is_link_local() || is_tailscale(v))),
        std::net::IpAddr::V6(v) => v.is_loopback(),
    }
}

/// Start the service on 127.0.0.1:port (or CTOP_WEB_BIND). Returns the URL (with token) to open.
pub fn start(w: Arc<Writer>, dir: &Path, port: u16) -> Result<String, String> {
    let token = load_token(dir)?;
    let bind = bind_addr();
    let l = TcpListener::bind(SocketAddr::from((bind, port))).map_err(|e| format!("cannot listen on {bind}:{port}: {e}"))?;
    if lan_enabled() {
        diag::info("web", format!("LAN access enabled on {bind}:{port} (private-network peers only, token required, plain HTTP)"));
    }
    let dist = dist_dir();
    let url = format!("http://127.0.0.1:{port}/?token={token}");
    *w.web_url.lock().unwrap() = Some(format!("http://127.0.0.1:{port}/"));
    diag::info("web", format!("browser service on http://127.0.0.1:{port}/ (frontend: {:?})", dist));
    let tok = token.clone();
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            if !conn.peer_addr().map(|a| peer_ok(a.ip())).unwrap_or(false) {
                continue;
            }
            let (w, tok, dist) = (w.clone(), tok.clone(), dist.clone());
            std::thread::spawn(move || handle(conn, &w, &tok, port, &dist));
        }
    });
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_embedded_in_the_page_head() {
        let out = String::from_utf8(with_token(b"<html><head><title>x</title></head><body></body></html>", "abc123")).unwrap();
        assert!(out.contains("<meta name=\"ctop-token\" content=\"abc123\" /></head>"), "{out}");
        assert_eq!(with_token(b"no head here", "t"), b"no head here");
    }
}
