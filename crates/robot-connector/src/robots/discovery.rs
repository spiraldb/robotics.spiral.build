//! Serial ports offered as candidate motor buses.

use crate::OperationError;
use crate::context::RobotConnectorContext;
use spiral_robotics_common::robots::SerialPortSummary;
use spiral_robotics_hardware::SerialPortCatalog;

trait SerialPortPresentation {
    fn description(&self) -> Option<String>;
}

impl SerialPortPresentation for serialport::SerialPortType {
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
}

impl SerialPortDiscovery {
    pub(super) async fn list(
        connector: &RobotConnectorContext,
    ) -> Result<Vec<SerialPortSummary>, OperationError> {
        let ports = SerialPortDiscovery::scan().await?;
        let hardware = connector.hardware().state();
        let in_use = connector
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
            .filter_map(|record| {
                Some((
                    record.config().port()?.path().to_owned(),
                    record.id().to_owned(),
                ))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        Ok(ports
            .into_ports()
            .into_iter()
            .map(|port| {
                SerialPortSummary::new(port.port_name.clone(), port.port_type.description())
                    .with_owner(in_use.get(&port.port_name).cloned())
            })
            .collect())
    }
}
