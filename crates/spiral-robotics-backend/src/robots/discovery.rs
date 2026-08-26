//! Framework-neutral discovery actions for serial ports and cameras.

use spiral_robotics_api::robots::{CameraStatus, IdentifyPortResponse, SerialPortSummary};
use spiral_robotics_core::OperationError;

pub(crate) trait SerialPortInventory: Send + Sync {
    async fn available_port_paths(&self) -> Result<Vec<String>, OperationError>;
}

pub(crate) trait SerialPortQueryPort: Send + Sync {
    async fn serial_ports(&self) -> Result<Vec<SerialPortSummary>, OperationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentifySerialPortCommand {
    before: Vec<String>,
}

impl IdentifySerialPortCommand {
    pub(crate) fn new(before: Vec<String>) -> Self {
        Self { before }
    }
}

impl IdentifySerialPortCommand {
    pub(crate) async fn execute<P: SerialPortInventory + ?Sized>(
        self,
        inventory: &P,
    ) -> Result<IdentifyPortResponse, OperationError> {
        let after = inventory.available_port_paths().await?;
        let mut removed = self
            .before
            .into_iter()
            .filter(|port| !after.contains(port))
            .collect::<Vec<_>>();
        removed.sort();
        removed.dedup();

        let (port, guidance) = match removed.as_slice() {
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
        Ok(IdentifyPortResponse::new(port, removed, guidance))
    }
}

pub(crate) trait CameraDiscoveryPort: Send + Sync {
    fn cameras(&self) -> Vec<CameraStatus>;
    async fn refresh_cameras(&self) -> Result<Vec<CameraStatus>, OperationError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Inventory {
        calls: Mutex<usize>,
        result: Result<Vec<String>, OperationError>,
    }

    impl SerialPortInventory for Inventory {
        async fn available_port_paths(&self) -> Result<Vec<String>, OperationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    fn inventory(after: &[&str]) -> Inventory {
        Inventory {
            calls: Mutex::new(0),
            result: Ok(after.iter().map(|port| (*port).to_owned()).collect()),
        }
    }

    #[tokio::test]
    async fn identification_explains_zero_one_and_multiple_disconnections() {
        let cases = [
            (
                vec!["/dev/a"],
                vec!["/dev/a"],
                None,
                Vec::<String>::new(),
                "No port disappeared",
            ),
            (
                vec!["/dev/a", "/dev/b"],
                vec!["/dev/b"],
                Some("/dev/a"),
                vec!["/dev/a".to_owned()],
                "Found the motor bus",
            ),
            (
                vec!["/dev/b", "/dev/a", "/dev/a"],
                vec![],
                None,
                vec!["/dev/a".to_owned(), "/dev/b".to_owned()],
                "2 ports disappeared",
            ),
        ];

        for (before, after, port, removed, guidance) in cases {
            let inventory = inventory(&after);
            let result = IdentifySerialPortCommand {
                before: before.into_iter().map(str::to_owned).collect(),
            }
            .execute(&inventory)
            .await
            .unwrap();
            assert_eq!(result.port(), port);
            assert_eq!(result.removed(), removed);
            assert!(result.message().contains(guidance));
            assert_eq!(*inventory.calls.lock().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn inventory_failure_is_preserved_without_fabricating_guidance() {
        let inventory = Inventory {
            calls: Mutex::new(0),
            result: Err(OperationError::Internal("serial scan failed".to_owned())),
        };
        assert_eq!(
            IdentifySerialPortCommand {
                before: vec!["/dev/a".to_owned()],
            }
            .execute(&inventory)
            .await,
            Err(OperationError::Internal("serial scan failed".to_owned()))
        );
        assert_eq!(*inventory.calls.lock().unwrap(), 1);
    }

    struct Cameras {
        calls: Mutex<usize>,
        result: Result<Vec<CameraStatus>, OperationError>,
    }

    impl CameraDiscoveryPort for Cameras {
        fn cameras(&self) -> Vec<CameraStatus> {
            self.result.clone().unwrap_or_default()
        }

        async fn refresh_cameras(&self) -> Result<Vec<CameraStatus>, OperationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    struct Ports {
        calls: Mutex<usize>,
        result: Result<Vec<SerialPortSummary>, OperationError>,
    }

    impl SerialPortQueryPort for Ports {
        async fn serial_ports(&self) -> Result<Vec<SerialPortSummary>, OperationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn serial_port_query_delegates_once_and_preserves_plain_projection() {
        let expected = vec![
            SerialPortSummary::new("/dev/tty.robot", "usb", Some("arm".to_owned()))
                .with_owner(Some("follower".to_owned())),
        ];
        let ports = Ports {
            calls: Mutex::new(0),
            result: Ok(expected.clone()),
        };

        assert_eq!(ports.serial_ports().await.unwrap(), expected);
        assert_eq!(*ports.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn camera_refresh_delegates_once_and_preserves_success_or_failure() {
        let cameras = Cameras {
            calls: Mutex::new(0),
            result: Ok(vec![CameraStatus::new(
                "wrist",
                "Wrist",
                spiral_robotics_api::robots::CameraStreamFormat::new(640, 480, 30),
            )]),
        };
        assert_eq!(cameras.refresh_cameras().await.unwrap()[0].id(), "wrist");
        assert_eq!(*cameras.calls.lock().unwrap(), 1);

        let cameras = Cameras {
            calls: Mutex::new(0),
            result: Err(OperationError::Internal("refresh task stopped".to_owned())),
        };
        assert_eq!(
            cameras.refresh_cameras().await,
            Err(OperationError::Internal("refresh task stopped".to_owned()))
        );
        assert_eq!(*cameras.calls.lock().unwrap(), 1);
    }
}
