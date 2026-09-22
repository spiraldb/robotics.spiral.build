//! Serial-port discovery helpers.

use std::collections::BTreeMap;

use serialport::SerialPortInfo;

/// A stable, de-duplicated snapshot of the serial devices visible to Studio.
#[derive(Clone, Debug, Default)]
pub struct SerialPortCatalog(Vec<SerialPortInfo>);

impl SerialPortCatalog {
    /// Discover the serial ports that could carry a motor bus.
    ///
    /// On macOS the paired `/dev/cu.*` and `/dev/tty.*` names for one device are
    /// collapsed to the `/dev/tty.*` form that LeRobot and the calibration CLI use.
    pub fn discover() -> Result<Self, serialport::Error> {
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

    /// Borrow the discovered ports in their stable display order.
    fn ports(&self) -> &[SerialPortInfo] {
        &self.0
    }

    /// Consume the catalog and return the underlying serial-port records.
    pub fn into_ports(self) -> Vec<SerialPortInfo> {
        self.0
    }

    /// Collapse macOS's paired `/dev/tty.*` and `/dev/cu.*` names.
    ///
    /// `serialport` reports both names for a physical serial device. LeRobot uses
    /// the `/dev/tty.*` namespace, so prefer it whenever both aliases are present.
    fn collapse_macos_aliases(ports: Vec<String>) -> Vec<String> {
        let mut by_device = BTreeMap::<String, String>::new();

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

impl AsRef<[SerialPortInfo]> for SerialPortCatalog {
    fn as_ref(&self) -> &[SerialPortInfo] {
        self.ports()
    }
}

impl IntoIterator for SerialPortCatalog {
    type Item = SerialPortInfo;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
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
