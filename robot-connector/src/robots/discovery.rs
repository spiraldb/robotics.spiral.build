//! Serial ports offered as candidate motor buses.

use crate::OperationError;
use crate::context::RobotConnectorContext;
use spiral_robotics_common::robots::SerialPortSummary;

pub(crate) struct SerialPortDiscovery;

impl SerialPortDiscovery {
    /// The machine's serial ports, each marked with the connected robot
    /// already holding it.
    pub(super) async fn list(
        connector: &RobotConnectorContext,
    ) -> Result<Vec<SerialPortSummary>, OperationError> {
        let serial_lines = connector.serial_lines().clone();
        let ports = tokio::task::spawn_blocking(move || serial_lines.available())
            .await
            .map_err(|error| OperationError::Internal(error.to_string()))?
            .map_err(|error| {
                OperationError::Internal(format!("could not list serial ports: {error}"))
            })?;
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
            .into_iter()
            .map(|port| {
                let owner = in_use.get(port.path()).cloned();
                port.with_owner(owner)
            })
            .collect())
    }
}
