//! Framework-neutral discovery actions for serial ports and cameras.

use spiral_robotics_core::OperationError;

pub(crate) trait SerialPortInventory: Send + Sync {
    async fn available_port_paths(&self) -> Result<Vec<String>, OperationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SerialPortDescriptor {
    path: String,
    kind: String,
    description: Option<String>,
    used_by: Option<String>,
}

impl SerialPortDescriptor {
    pub(crate) fn new(
        path: impl Into<String>,
        kind: impl Into<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            path: path.into(),
            kind: kind.into(),
            description,
            used_by: None,
        }
    }

    pub(crate) fn with_owner(mut self, used_by: Option<String>) -> Self {
        self.used_by = used_by;
        self
    }

    pub(crate) fn into_parts(self) -> (String, String, Option<String>, Option<String>) {
        (self.path, self.kind, self.description, self.used_by)
    }
}

pub(crate) trait SerialPortQueryPort: Send + Sync {
    async fn serial_ports(&self) -> Result<Vec<SerialPortDescriptor>, OperationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentifySerialPortCommand {
    before: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SerialPortIdentification {
    port: Option<String>,
    removed: Vec<String>,
    guidance: String,
}

impl IdentifySerialPortCommand {
    pub(crate) fn new(before: Vec<String>) -> Self {
        Self { before }
    }
}

impl SerialPortIdentification {
    pub(crate) fn into_parts(self) -> (Option<String>, Vec<String>, String) {
        (self.port, self.removed, self.guidance)
    }
}

impl IdentifySerialPortCommand {
    pub(crate) async fn execute<P: SerialPortInventory + ?Sized>(
        self,
        inventory: &P,
    ) -> Result<SerialPortIdentification, OperationError> {
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
        Ok(SerialPortIdentification {
            port,
            removed,
            guidance,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredCamera {
    id: String,
    name: String,
    format: CameraDiscoveryFormat,
    status: CameraDiscoveryStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CameraDiscoveryFormat {
    width: u32,
    height: u32,
    fps: u32,
}

impl CameraDiscoveryFormat {
    pub(crate) fn new(width: u32, height: u32, fps: u32) -> Self {
        Self { width, height, fps }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CameraDiscoveryStatus {
    streaming: bool,
    frames: u64,
    error: Option<String>,
}

impl CameraDiscoveryStatus {
    pub(crate) fn new(streaming: bool, frames: u64, error: Option<String>) -> Self {
        Self {
            streaming,
            frames,
            error,
        }
    }
}

impl DiscoveredCamera {
    pub(crate) fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        format: CameraDiscoveryFormat,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            format,
            status: CameraDiscoveryStatus::new(false, 0, None),
        }
    }

    pub(crate) fn with_status(mut self, status: CameraDiscoveryStatus) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn width(&self) -> u32 {
        self.format.width
    }
    pub(crate) fn height(&self) -> u32 {
        self.format.height
    }
    pub(crate) fn fps(&self) -> u32 {
        self.format.fps
    }
    pub(crate) fn streaming(&self) -> bool {
        self.status.streaming
    }
    pub(crate) fn frames(&self) -> u64 {
        self.status.frames
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.status.error.as_deref()
    }
}

pub(crate) trait CameraDiscoveryPort: Send + Sync {
    fn cameras(&self) -> Vec<DiscoveredCamera>;
    async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, OperationError>;
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
        result: Result<Vec<DiscoveredCamera>, OperationError>,
    }

    impl CameraDiscoveryPort for Cameras {
        fn cameras(&self) -> Vec<DiscoveredCamera> {
            self.result.clone().unwrap_or_default()
        }

        async fn refresh_cameras(&self) -> Result<Vec<DiscoveredCamera>, OperationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    struct Ports {
        calls: Mutex<usize>,
        result: Result<Vec<SerialPortDescriptor>, OperationError>,
    }

    impl SerialPortQueryPort for Ports {
        async fn serial_ports(&self) -> Result<Vec<SerialPortDescriptor>, OperationError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn serial_port_query_delegates_once_and_preserves_plain_projection() {
        let expected = vec![SerialPortDescriptor {
            path: "/dev/tty.robot".to_owned(),
            kind: "usb".to_owned(),
            description: Some("arm".to_owned()),
            used_by: Some("follower".to_owned()),
        }];
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
            result: Ok(vec![DiscoveredCamera::new(
                "wrist",
                "Wrist",
                CameraDiscoveryFormat::new(640, 480, 30),
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
