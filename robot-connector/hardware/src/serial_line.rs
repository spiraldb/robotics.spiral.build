//! The serial lines motor buses hang off, as each platform opens them.
//!
//! A computer reaches a bus through a USB serial adapter; a microcontroller
//! wires it to one of its UARTs. The bus protocol above only needs bytes in
//! and out, so the platform supplies the line and the protocol stays shared.

use std::io;
use std::time::Duration;

use spiral_robotics_common::robots::{PortSettings, SerialPortSummary};

/// How long a read waits for bytes before handing control back.
///
/// A bus keeps its own deadline for a whole response, so an open line must
/// return this often even when nothing arrives.
pub const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(20);

/// An open serial line carrying one motor bus.
///
/// `read` returns within [`SERIAL_READ_TIMEOUT`] when nothing has arrived,
/// either with `Ok(0)` or with a `TimedOut`, `WouldBlock`, or `Interrupted`
/// error.
pub trait SerialLine: io::Read + io::Write + Send {
    /// Drop bytes that arrived but were not read.
    fn discard_input(&mut self) -> io::Result<()>;
}

/// The serial ports one machine offers for motor buses.
pub trait SerialLines: Send + Sync {
    /// Ports that could carry a motor bus, described for an operator.
    fn available(&self) -> io::Result<Vec<SerialPortSummary>>;

    /// Open `port` at its baud rate.
    fn open(&self, port: &PortSettings) -> io::Result<Box<dyn SerialLine>>;
}

/// A machine without serial ports, for tests that only drive simulated arms.
#[cfg(any(test, feature = "test-support"))]
pub struct NoSerialLines;

#[cfg(any(test, feature = "test-support"))]
impl SerialLines for NoSerialLines {
    fn available(&self) -> io::Result<Vec<SerialPortSummary>> {
        Ok(Vec::new())
    }

    fn open(&self, port: &PortSettings) -> io::Result<Box<dyn SerialLine>> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("this machine has no serial port {:?}", port.path()),
        ))
    }
}
