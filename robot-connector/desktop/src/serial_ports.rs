//! The serial ports a computer offers for motor buses: USB serial adapters,
//! listed and opened through the operating system.

use std::io;

use serialport::{ClearBuffer, SerialPort, SerialPortInfo, SerialPortType};
use spiral_robotics_common::robots::{PortSettings, SerialPortSummary};
use spiral_robotics_hardware::{SERIAL_READ_TIMEOUT, SerialLine, SerialLines};

/// The serial ports the operating system reports.
pub(crate) struct SystemSerialPorts;

impl SerialLines for SystemSerialPorts {
    fn available(&self) -> io::Result<Vec<SerialPortSummary>> {
        let catalog = SerialPortCatalog::discover().map_err(io::Error::from)?;
        Ok(catalog
            .into_ports()
            .into_iter()
            .map(|port| SerialPortSummary::new(port.port_name, port.port_type.description()))
            .collect())
    }

    fn open(&self, port: &PortSettings) -> io::Result<Box<dyn SerialLine>> {
        let opened = serialport::new(port.path(), port.baud_rate())
            .timeout(SERIAL_READ_TIMEOUT)
            .open()?;
        Ok(Box::new(OpenSerialPort(opened)))
    }
}

/// A serial port the operating system opened.
struct OpenSerialPort(Box<dyn SerialPort>);

impl io::Read for OpenSerialPort {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl io::Write for OpenSerialPort {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl SerialLine for OpenSerialPort {
    fn discard_input(&mut self) -> io::Result<()> {
        Ok(self.0.clear(ClearBuffer::Input)?)
    }
}

/// How an operator recognizes a port: the adapter behind it.
trait SerialPortPresentation {
    fn description(&self) -> Option<String>;
}

impl SerialPortPresentation for SerialPortType {
    fn description(&self) -> Option<String> {
        match self {
            Self::UsbPort(info) => {
                let mut details = vec![format!("USB {:04x}:{:04x}", info.vid, info.pid)];
                if let Some(product) = &info.product {
                    details.push(product.clone());
                }
                if let Some(serial) = &info.serial_number {
                    details.push(format!("serial {serial}"));
                }
                Some(details.join(", "))
            }
            Self::BluetoothPort => Some("Bluetooth".to_owned()),
            Self::PciPort => Some("PCI".to_owned()),
            Self::Unknown => None,
        }
    }
}

/// A stable, de-duplicated snapshot of the serial devices visible to Studio.
#[derive(Clone, Debug, Default)]
pub(crate) struct SerialPortCatalog(Vec<SerialPortInfo>);

impl SerialPortCatalog {
    /// Discover the serial ports that could carry a motor bus.
    ///
    /// On macOS the paired `/dev/cu.*` and `/dev/tty.*` names for one device are
    /// collapsed to the `/dev/tty.*` form that LeRobot uses.
    pub(crate) fn discover() -> Result<Self, serialport::Error> {
        let mut ports = serialport::available_ports()?;

        #[cfg(target_os = "macos")]
        {
            let preferred_names = Self::collapse_macos_aliases(
                ports.iter().map(|port| port.port_name.clone()).collect(),
            );
            ports.retain(|port| preferred_names.binary_search(&port.port_name).is_ok());
        }

        ports.sort_by(|left, right| left.port_name.cmp(&right.port_name));
        ports.dedup_by(|left, right| left.port_name == right.port_name);
        Ok(Self(ports))
    }

    /// Consume the catalog and return the underlying serial-port records.
    pub(crate) fn into_ports(self) -> Vec<SerialPortInfo> {
        self.0
    }

    /// Collapse macOS's paired `/dev/tty.*` and `/dev/cu.*` names.
    ///
    /// `serialport` reports both names for a physical serial device. LeRobot uses
    /// the `/dev/tty.*` namespace, so prefer it whenever both aliases are present.
    #[cfg(any(target_os = "macos", test))]
    fn collapse_macos_aliases(ports: Vec<String>) -> Vec<String> {
        let mut by_device = std::collections::BTreeMap::<String, String>::new();

        for port in ports {
            let key = port
                .strip_prefix("/dev/cu.")
                .or_else(|| port.strip_prefix("/dev/tty."))
                .map_or_else(|| port.clone(), ToOwned::to_owned);

            by_device
                .entry(key)
                .and_modify(|current| {
                    if port.starts_with("/dev/tty.") && !current.starts_with("/dev/tty.") {
                        current.clone_from(&port);
                    }
                })
                .or_insert(port);
        }

        let mut collapsed = by_device.into_values().collect::<Vec<_>>();
        collapsed.sort();
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::SerialPortCatalog;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn prefers_tty_alias_on_macos() {
        let ports = strings(&[
            "/dev/cu.usbmodem1",
            "/dev/tty.usbmodem1",
            "/dev/cu.Bluetooth-Incoming-Port",
            "/dev/tty.Bluetooth-Incoming-Port",
        ]);

        assert_eq!(
            SerialPortCatalog::collapse_macos_aliases(ports),
            ["/dev/tty.Bluetooth-Incoming-Port", "/dev/tty.usbmodem1"]
        );
    }

    #[test]
    fn retains_an_unpaired_cu_alias() {
        let ports = strings(&["/dev/cu.usbserial-only"]);

        assert_eq!(
            SerialPortCatalog::collapse_macos_aliases(ports),
            ["/dev/cu.usbserial-only"]
        );
    }

    #[test]
    fn sorts_mixed_paired_and_unpaired_aliases() {
        let ports = strings(&["/dev/cu.alpha", "/dev/tty.alpha", "/dev/cu.zulu"]);

        assert_eq!(
            SerialPortCatalog::collapse_macos_aliases(ports),
            ["/dev/cu.zulu", "/dev/tty.alpha"]
        );
    }
}
