//! Framework-neutral discovery actions for serial ports and cameras.

use super::error::ApplicationError;

pub(crate) trait SerialPortInventory: Send + Sync {
    async fn available_port_paths(&self) -> Result<Vec<String>, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SerialPortView {
    pub path: String,
    pub kind: String,
    pub description: Option<String>,
    pub used_by: Option<String>,
}

pub(crate) trait SerialPortQueryPort: Send + Sync {
    async fn serial_ports(&self) -> Result<Vec<SerialPortView>, ApplicationError>;
}

pub(crate) async fn list_serial_ports<P: SerialPortQueryPort + ?Sized>(
    query: &P,
) -> Result<Vec<SerialPortView>, ApplicationError> {
    query.serial_ports().await
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentifySerialPortCommand {
    pub before: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SerialPortIdentification {
    pub port: Option<String>,
    pub removed: Vec<String>,
    pub guidance: String,
}

pub(crate) async fn identify_serial_port<P: SerialPortInventory + ?Sized>(
    inventory: &P,
    command: IdentifySerialPortCommand,
) -> Result<SerialPortIdentification, ApplicationError> {
    let after = inventory.available_port_paths().await?;
    let mut removed = command
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
    Ok(SerialPortIdentification {
        port,
        removed,
        guidance,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredCamera {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub streaming: bool,
    pub frames: u64,
    pub error: Option<String>,
}

pub(crate) trait CameraDiscoveryPort: Send + Sync {
    fn cameras(&self) -> Vec<DiscoveredCamera>;
    async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, ApplicationError>;
}

pub(crate) fn list_cameras<P: CameraDiscoveryPort + ?Sized>(
    discovery: &P,
) -> Vec<DiscoveredCamera> {
    discovery.cameras()
}

pub(crate) async fn refresh_cameras<P: CameraDiscoveryPort + ?Sized>(
    discovery: &P,
) -> Result<Vec<DiscoveredCamera>, ApplicationError> {
    discovery.refresh_cameras().await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Inventory {
        calls: Mutex<usize>,
        result: Result<Vec<String>, ApplicationError>,
    }

    impl SerialPortInventory for Inventory {
        async fn available_port_paths(&self) -> Result<Vec<String>, ApplicationError> {
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
            let result = identify_serial_port(
                &inventory,
                IdentifySerialPortCommand {
                    before: before.into_iter().map(str::to_owned).collect(),
                },
            )
            .await
            .unwrap();
            assert_eq!(result.port.as_deref(), port);
            assert_eq!(result.removed, removed);
            assert!(result.guidance.contains(guidance));
            assert_eq!(*inventory.calls.lock().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn inventory_failure_is_preserved_without_fabricating_guidance() {
        let inventory = Inventory {
            calls: Mutex::new(0),
            result: Err(ApplicationError::Internal("serial scan failed".to_owned())),
        };
        assert_eq!(
            identify_serial_port(
                &inventory,
                IdentifySerialPortCommand {
                    before: vec!["/dev/a".to_owned()],
                },
            )
            .await,
            Err(ApplicationError::Internal("serial scan failed".to_owned()))
        );
        assert_eq!(*inventory.calls.lock().unwrap(), 1);
    }

    struct Cameras {
        calls: Mutex<usize>,
        result: Result<Vec<DiscoveredCamera>, ApplicationError>,
    }

    impl CameraDiscoveryPort for Cameras {
        fn cameras(&self) -> Vec<DiscoveredCamera> {
            self.result.clone().unwrap_or_default()
        }

        async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, ApplicationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    struct Ports {
        calls: Mutex<usize>,
        result: Result<Vec<SerialPortView>, ApplicationError>,
    }

    impl SerialPortQueryPort for Ports {
        async fn serial_ports(&self) -> Result<Vec<SerialPortView>, ApplicationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn serial_port_query_delegates_once_and_preserves_plain_projection() {
        let expected = vec![SerialPortView {
            path: "/dev/tty.robot".to_owned(),
            kind: "usb".to_owned(),
            description: Some("arm".to_owned()),
            used_by: Some("follower".to_owned()),
        }];
        let ports = Ports {
            calls: Mutex::new(0),
            result: Ok(expected.clone()),
        };

        assert_eq!(list_serial_ports(&ports).await.unwrap(), expected);
        assert_eq!(*ports.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn camera_refresh_delegates_once_and_preserves_success_or_failure() {
        let cameras = Cameras {
            calls: Mutex::new(0),
            result: Ok(vec![DiscoveredCamera {
                id: "wrist".to_owned(),
                name: "Wrist".to_owned(),
                width: 640,
                height: 480,
                fps: 30,
                streaming: false,
                frames: 0,
                error: None,
            }]),
        };
        assert_eq!(refresh_cameras(&cameras).await.unwrap()[0].id, "wrist");
        assert_eq!(*cameras.calls.lock().unwrap(), 1);

        let cameras = Cameras {
            calls: Mutex::new(0),
            result: Err(ApplicationError::Internal(
                "refresh task stopped".to_owned(),
            )),
        };
        assert_eq!(
            refresh_cameras(&cameras).await,
            Err(ApplicationError::Internal(
                "refresh task stopped".to_owned()
            ))
        );
        assert_eq!(*cameras.calls.lock().unwrap(), 1);
    }
}
