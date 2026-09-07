//! Serial discovery and identification from an unplugged motor bus.

use crate::OperationError;
use crate::project_session::context::ProjectContext;
use spiral_robotics_common::robots::SerialPortSummary;
use spiral_robotics_hardware::SerialPortCatalog;

trait SerialPortPresentation {
    fn kind(&self) -> &'static str;
    fn description(&self) -> Option<String>;
}

impl SerialPortPresentation for serialport::SerialPortType {
    fn kind(&self) -> &'static str {
        match self {
            Self::UsbPort(_) => "usb",
            Self::BluetoothPort => "bluetooth",
            Self::PciPort => "pci",
            Self::Unknown => "unknown",
        }
    }

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

pub(crate) struct SerialPortDiscovery;

impl SerialPortDiscovery {
    async fn scan() -> Result<SerialPortCatalog, OperationError> {
        tokio::task::spawn_blocking(SerialPortCatalog::discover)
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(|error| {
                OperationError::Internal(format!("could not list serial ports: {error}"))
            })
    }

    pub(crate) async fn identify(
        before: Vec<String>,
    ) -> Result<IdentifiedSerialPort, OperationError> {
        let after = Self::scan()
            .await?
            .into_ports()
            .into_iter()
            .map(|port| port.port_name)
            .collect();
        Ok(IdentifiedSerialPort::between(before, after))
    }
}

impl SerialPortDiscovery {
    pub(crate) async fn list(
        project: &ProjectContext,
    ) -> Result<Vec<SerialPortSummary>, OperationError> {
        let ports = SerialPortDiscovery::scan().await?;
        let hardware = project.hardware().state();
        let in_use = project
            .workspace()
            .list()
            .await
            .into_iter()
            .filter(|record| {
                hardware
                    .connections()
                    .get(record.id())
                    .is_some_and(|connection| connection.connected())
            })
            .filter_map(|record| Some((record.port()?.path().to_owned(), record.id().to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>();
        Ok(ports
            .into_ports()
            .into_iter()
            .map(|port| {
                SerialPortSummary::new(
                    port.port_name.clone(),
                    port.port_type.kind(),
                    port.port_type.description(),
                )
                .with_owner(in_use.get(&port.port_name).cloned())
            })
            .collect())
    }
}

pub(crate) struct IdentifiedSerialPort {
    port: Option<String>,
    message: String,
}

impl IdentifiedSerialPort {
    pub(crate) fn into_parts(self) -> (Option<String>, String) {
        (self.port, self.message)
    }

    fn between(before: Vec<String>, after: Vec<String>) -> Self {
        let mut removed = before
            .into_iter()
            .filter(|port| !after.contains(port))
            .collect::<Vec<_>>();
        removed.sort();
        removed.dedup();
        let (port, message) = match removed.as_slice() {
            [port] => (
                Some(port.clone()),
                format!("Found the motor bus on {port}. Plug the cable back in."),
            ),
            [] => (
                None,
                "No port disappeared. Make sure you unplugged the arm, then try again.".to_owned(),
            ),
            ports => (
                None,
                format!(
                    "{} ports disappeared ({}). Unplug only the arm you are setting up.",
                    ports.len(),
                    ports.join(", ")
                ),
            ),
        };
        Self { port, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identification_explains_zero_one_and_multiple_disconnections() {
        for (before, after, port, guidance) in [
            (vec!["/dev/a"], vec!["/dev/a"], None, "No port disappeared"),
            (
                vec!["/dev/a", "/dev/b"],
                vec!["/dev/b"],
                Some("/dev/a"),
                "Found the motor bus on /dev/a",
            ),
            (
                vec!["/dev/b", "/dev/a", "/dev/a"],
                vec![],
                None,
                "2 ports disappeared (/dev/a, /dev/b)",
            ),
        ] {
            let result = IdentifiedSerialPort::between(
                before.into_iter().map(str::to_owned).collect(),
                after.into_iter().map(str::to_owned).collect(),
            );
            assert_eq!(result.port.as_deref(), port);
            assert!(result.message.contains(guidance));
        }
    }
}
