//! Real serial transport with single-owner semantics.
//!
//! Exclusivity is layered because no single mechanism is airtight on Linux:
//!  1. Before opening (which on FTDI/Arduino boards asserts DTR and RESETS the controller) we
//!     look for existing holders: open fds in /proc/*/fd, flock/fcntl locks in /proc/locks,
//!     and UUCP lock files. If anything holds the device we refuse — we never take it over.
//!  2. After opening we set TIOCEXCL (later opens by non-root users fail with EBUSY) and take
//!     an exclusive non-blocking flock.
//!  3. We re-check for a second holder to catch races.
//! Limits (stated honestly): fds owned by other users or root are invisible to /proc scans, and
//! flock is advisory; a process that ignores both and opens as root can still interfere.

use crate::transport::Transport;
use serde::Serialize;
use serialport::{SerialPort, TTYPort};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub path: String,
    pub real_path: String,
    pub by_id: bool,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub pid: u32,
    pub command: String,
    pub how: String,
}

#[derive(Debug, Clone)]
pub enum SerialError {
    NotFound(String),
    InUse(Vec<Holder>),
    Busy(String),
    Permission(String),
    Io(String),
}

impl std::fmt::Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerialError::NotFound(p) => write!(f, "Serial device {p} does not exist (is the printer plugged in?)"),
            SerialError::InUse(h) => {
                let who: Vec<String> = h.iter().map(|x| format!("{} (pid {}, {})", x.command, x.pid, x.how)).collect();
                write!(f, "Device is already in use by {}. CtoP will not take it over — close that program first.", who.join(", "))
            }
            SerialError::Busy(m) => write!(f, "Device is busy / locked by another process: {m}"),
            SerialError::Permission(m) => write!(f, "No permission to open the serial device: {m}. Add your user to the 'dialout' group."),
            SerialError::Io(m) => write!(f, "Serial error: {m}"),
        }
    }
}

pub fn list_ports() -> Vec<PortInfo> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir("/dev/serial/by-id") {
        for e in rd.flatten() {
            let p = e.path();
            let real = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
            out.push(PortInfo {
                path: p.to_string_lossy().into(),
                real_path: real.to_string_lossy().into(),
                by_id: true,
                label: e.file_name().to_string_lossy().into(),
            });
        }
    }
    if let Ok(rd) = fs::read_dir("/dev") {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("ttyUSB") || n.starts_with("ttyACM") {
                let p = e.path();
                if !out.iter().any(|o| o.real_path == p.to_string_lossy()) {
                    out.push(PortInfo { path: p.to_string_lossy().into(), real_path: p.to_string_lossy().into(), by_id: false, label: n });
                }
            }
        }
    }
    out.sort_by(|a, b| b.by_id.cmp(&a.by_id).then(a.path.cmp(&b.path)));
    out
}

fn comm_of(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into())
}

/// Everything we can see that currently holds `real` (canonical device path), excluding ourselves.
pub fn find_holders(real: &Path) -> Vec<Holder> {
    let me = std::process::id();
    let mut holders: Vec<Holder> = Vec::new();
    let mut push = |h: Holder| {
        if h.pid != me && !holders.iter().any(|x| x.pid == h.pid) {
            holders.push(h);
        }
    };
    // 1. open file descriptors
    if let Ok(procs) = fs::read_dir("/proc") {
        for p in procs.flatten() {
            let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) {
                for fd in fds.flatten() {
                    if fs::read_link(fd.path()).map_or(false, |t| t == real) {
                        push(Holder { pid, command: comm_of(pid), how: "has the port open".into() });
                        break;
                    }
                }
            }
        }
    }
    // 2. kernel file locks (visible across users)
    if let (Ok(meta), Ok(locks)) = (fs::metadata(real), fs::read_to_string("/proc/locks")) {
        let ino = meta.ino();
        let dev = meta.dev();
        let (maj, min) = (libc::major(dev as libc::dev_t), libc::minor(dev as libc::dev_t));
        for l in locks.lines() {
            let t: Vec<&str> = l.split_whitespace().collect();
            // "1: FLOCK ADVISORY WRITE 4242 00:06:1234 0 EOF"
            if t.len() >= 6 {
                let want = format!("{:02x}:{:02x}:{}", maj, min, ino);
                let alt = format!("{maj:02}:{min:02}:{ino}");
                if t[5] == want || t[5] == alt || t[5].ends_with(&format!(":{ino}")) && t[5].starts_with(&format!("{:02x}:{:02x}", maj, min)) {
                    if let Ok(pid) = t[4].parse::<u32>() {
                        push(Holder { pid, command: comm_of(pid), how: format!("holds a {} lock", t[1].to_lowercase()) });
                    }
                }
            }
        }
    }
    // 3. UUCP lock files
    if let Some(name) = real.file_name().and_then(|n| n.to_str()) {
        for dir in ["/var/lock", "/run/lock", "/var/lock/lockdev"] {
            let f = format!("{dir}/LCK..{name}");
            if let Ok(txt) = fs::read_to_string(&f) {
                if let Ok(pid) = txt.trim().parse::<u32>() {
                    if Path::new(&format!("/proc/{pid}")).exists() {
                        push(Holder { pid, command: comm_of(pid), how: format!("owns lock file {f}") });
                    }
                }
            }
        }
    }
    holders
}

/// Non-opening availability check (opening could reset the board).
pub fn check_available(path: &str) -> Result<PathBuf, SerialError> {
    let real = fs::canonicalize(path).map_err(|_| SerialError::NotFound(path.into()))?;
    let h = find_holders(&real);
    if h.is_empty() { Ok(real) } else { Err(SerialError::InUse(h)) }
}

pub struct SerialTransport {
    port: Option<TTYPort>,
    buf: Vec<u8>,
    path: String,
}

pub fn open_exclusive(path: &str, baud: u32) -> Result<SerialTransport, SerialError> {
    let real = check_available(path)?;
    let port = serialport::new(real.to_string_lossy().as_ref(), baud)
        .timeout(Duration::from_millis(20))
        .open_native()
        .map_err(|e| match e.kind() {
            serialport::ErrorKind::NoDevice => SerialError::NotFound(path.into()),
            serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied) => SerialError::Permission(e.to_string()),
            serialport::ErrorKind::Io(_) if e.description.contains("busy") || e.description.contains("Busy") => SerialError::Busy(e.to_string()),
            _ => SerialError::Io(e.to_string()),
        })?;
    let mut port = port;
    let fd = port.as_raw_fd();
    // advisory exclusive lock
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        drop(port);
        return Err(SerialError::Busy(format!("flock failed: {e}")));
    }
    // kernel-enforced exclusivity against later opens
    port.set_exclusive(true).map_err(|e| SerialError::Io(format!("TIOCEXCL failed: {e}")))?;
    // race check: did someone else open it between our check and our open?
    let other = find_holders(&real);
    if !other.is_empty() {
        drop(port);
        return Err(SerialError::InUse(other));
    }
    let _ = port.clear(serialport::ClearBuffer::All);
    Ok(SerialTransport { port: Some(port), buf: Vec::new(), path: path.to_string() })
}

impl Transport for SerialTransport {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let p = self.port.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "port closed"))?;
        p.write_all(line.as_bytes())?;
        p.write_all(b"\n")?;
        p.flush()
    }

    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let s = String::from_utf8_lossy(&line).trim().to_string();
                if s.is_empty() {
                    continue;
                }
                return Ok(Some(s));
            }
            let p = self.port.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "port closed"))?;
            let mut tmp = [0u8; 256];
            match p.read(&mut tmp) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "serial device disappeared")),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    fn close(&mut self) {
        if let Some(p) = self.port.take() {
            let _ = p.as_raw_fd();
            drop(p); // closing releases flock and TIOCEXCL
        }
    }

    fn describe(&self) -> String {
        self.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_device_is_not_found() {
        assert!(matches!(check_available("/dev/does-not-exist-ctop"), Err(SerialError::NotFound(_))));
    }

    #[test]
    fn detects_holder_of_a_file_we_hold_via_another_process() {
        // `sleep` keeps a file open via shell redirection; find_holders must see it.
        let dir = std::env::temp_dir().join(format!("ctop-hold-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("fakedev");
        fs::write(&target, b"").unwrap();
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec 3<>{}; sleep 5", target.display()))
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let real = fs::canonicalize(&target).unwrap();
        let h = find_holders(&real);
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
        assert!(!h.is_empty(), "should detect the other process holding the file");
        assert_ne!(h[0].pid, std::process::id());
    }

    #[test]
    fn no_holders_for_unused_file() {
        let t = std::env::temp_dir().join(format!("ctop-free-{}", std::process::id()));
        fs::write(&t, b"").unwrap();
        let real = fs::canonicalize(&t).unwrap();
        assert!(find_holders(&real).is_empty());
        let _ = fs::remove_file(&t);
    }
}
