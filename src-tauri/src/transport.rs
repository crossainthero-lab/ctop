//! Line-oriented transport abstraction shared by the real serial port and the simulator.

use std::io;
use std::time::Duration;

pub trait Transport: Send {
    /// Write one command line (a newline is appended).
    fn write_line(&mut self, line: &str) -> io::Result<()>;
    /// Wait up to `timeout` for a complete line. `Ok(None)` on timeout.
    /// An `Err` means the link is gone (unplugged, closed, reset by the OS).
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>>;
    fn close(&mut self);
    fn describe(&self) -> String;
}
