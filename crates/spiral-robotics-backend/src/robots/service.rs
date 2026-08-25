//! Robot-configuration actions independent of HTTP, serialization, storage, and drivers.

use std::collections::BTreeMap;

use spiral_robotics_core::calibration::{Calibration, MotorSpec};

use spiral_robotics_core::error::OperationError;

const DEFAULT_ROBOT_BAUD_RATE: u32 = 1_000_000;
const MAX_ROBOT_ID_LENGTH: usize = 64;
const MAX_CAMERA_ROLE_LENGTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialPortConfiguration {
    pub path: String,
    pub baud_rate: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateRobotCommand {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<String>,
    pub baud_rate: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewRobot {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraAssignment {
    pub role: String,
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateRobotCommand {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
    pub cameras: Option<Vec<CameraAssignment>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RobotUpdate<C> {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
    pub cameras: Option<C>,
}

/// Plain information required to connect a saved robot.
#[derive(Clone, Debug, PartialEq)]
pub struct RobotConnectionSpec {
    pub id: String,
    pub port: SerialPortConfiguration,
    pub motors: BTreeMap<String, MotorSpec>,
    pub calibration: Option<Calibration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedRobot {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportedRobotDocument {
    pub bytes: Vec<u8>,
}

/// Persistence operations needed by robot-configuration actions.
///
/// Camera patches are adapter-minted capabilities consumed by the same
/// store after capability validation.
pub trait RobotStore {
    type CameraConfiguration;

    fn create(&self, robot: NewRobot) -> Result<SavedRobot, OperationError>;

    fn import_configuration(&self) -> Result<SavedRobot, OperationError>;

    fn export_configuration(&self, robot: &str) -> Result<ExportedRobotDocument, OperationError>;

    fn camera_configuration(
        &self,
        robot: &str,
        assignments: &[CameraAssignment],
    ) -> Result<Self::CameraConfiguration, OperationError>;

    fn update(
        &self,
        command: RobotUpdate<Self::CameraConfiguration>,
    ) -> Result<SavedRobot, OperationError>;

    fn ensure_exists(&self, robot: &str) -> Result<(), OperationError>;

    fn delete(&self, robot: &str) -> Result<(), OperationError>;

    fn saved_robot_ids(&self) -> Vec<String>;

    fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, OperationError>;
}

/// Live hardware operations needed by robot-configuration actions.
pub trait RobotConnectionPort {
    fn is_connected(&self, robot: &str) -> bool;

    async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), OperationError>;

    async fn disconnect(&self, robot: &str) -> Result<(), OperationError>;

    /// Close the bus and remove its connection history.
    async fn forget(&self, robot: &str) -> Result<(), OperationError>;
}

pub fn create<R: RobotStore + ?Sized>(
    store: &R,
    mut command: CreateRobotCommand,
) -> Result<SavedRobot, OperationError> {
    command.id = command.id.trim().to_owned();
    validate_plain_name("robot id", &command.id, MAX_ROBOT_ID_LENGTH)?;
    let port = match (command.port, command.baud_rate) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(OperationError::InvalidInput(
                "a baud rate can only be set with a serial port".to_owned(),
            ));
        }
        (Some(path), baud_rate) => Some(validated_port(
            path,
            baud_rate.unwrap_or(DEFAULT_ROBOT_BAUD_RATE),
        )?),
    };
    store.create(NewRobot {
        id: command.id,
        robot_type: command.robot_type,
        port,
    })
}

pub fn import_configuration<R: RobotStore + ?Sized>(
    store: &R,
) -> Result<SavedRobot, OperationError> {
    store.import_configuration()
}

/// Select saved robot identities for a read presenter.
pub fn list<R: RobotStore + ?Sized>(store: &R) -> Vec<SavedRobot> {
    store
        .saved_robot_ids()
        .into_iter()
        .map(|id| SavedRobot { id })
        .collect()
}

/// Admit one saved identity before an outer presenter hydrates its view.
pub fn get<R: RobotStore + ?Sized>(store: &R, robot: String) -> Result<SavedRobot, OperationError> {
    validate_plain_name("robot id", &robot, MAX_ROBOT_ID_LENGTH)?;
    store.ensure_exists(&robot)?;
    Ok(SavedRobot { id: robot })
}

pub fn export_configuration<R: RobotStore + ?Sized>(
    store: &R,
    robot: &str,
) -> Result<ExportedRobotDocument, OperationError> {
    store.export_configuration(robot)
}

pub fn update<R, C>(
    store: &R,
    control: &C,
    mut command: UpdateRobotCommand,
) -> Result<SavedRobot, OperationError>
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    if let Some(port) = command.port.take() {
        command.port = Some(validated_port(port.path, port.baud_rate)?);
        if control.is_connected(&command.id) {
            return Err(OperationError::Conflict(format!(
                "disconnect robot {:?} before changing its serial port",
                command.id
            )));
        }
    }
    let cameras = match command.cameras {
        Some(mut assignments) => {
            let mut roles = std::collections::BTreeSet::new();
            for assignment in &mut assignments {
                assignment.role = assignment.role.trim().to_owned();
                assignment.device_id = assignment.device_id.trim().to_owned();
                validate_plain_name("camera role", &assignment.role, MAX_CAMERA_ROLE_LENGTH)?;
                if !roles.insert(assignment.role.clone()) {
                    return Err(OperationError::InvalidInput(format!(
                        "camera role {:?} is used more than once",
                        assignment.role
                    )));
                }
                if assignment.device_id.is_empty() || assignment.device_id.contains('\0') {
                    return Err(OperationError::InvalidInput(format!(
                        "camera role {:?} needs a device id without NUL bytes",
                        assignment.role
                    )));
                }
            }
            Some(store.camera_configuration(&command.id, &assignments)?)
        }
        None => None,
    };
    store.update(RobotUpdate {
        id: command.id,
        robot_type: command.robot_type,
        port: command.port,
        cameras,
    })
}

fn validated_port(path: String, baud_rate: u32) -> Result<SerialPortConfiguration, OperationError> {
    let path = path.trim().to_owned();
    if path.is_empty() || path.contains('\0') || baud_rate == 0 {
        return Err(OperationError::InvalidInput(
            "a port needs a non-empty path without NUL bytes and a positive baud rate".to_owned(),
        ));
    }
    Ok(SerialPortConfiguration { path, baud_rate })
}

fn validate_plain_name(label: &str, value: &str, maximum: usize) -> Result<(), OperationError> {
    if value.is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(OperationError::InvalidInput(format!(
            "{label} must be 1 to {maximum} characters of letters, digits, '-', or '_'"
        )));
    }
    Ok(())
}

/// Forget live hardware before removing its durable configuration.
///
/// The existence check happens first so a missing configuration cannot unexpectedly
/// tear down a bus. Once it passes, failure is deliberately biased toward a
/// saved-but-disconnected robot: deleting the SpiralDB row first could leave a
/// live bus that no configuration or UI card can identify and stop.
pub async fn delete<R, C>(store: &R, control: &C, robot: &str) -> Result<(), OperationError>
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    store.ensure_exists(robot)?;
    control.forget(robot).await?;
    store.delete(robot)
}

pub async fn connect<R, C>(store: &R, control: &C, robot: &str) -> Result<(), OperationError>
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    let specification = store.connection_spec(robot)?;
    control.connect(specification).await
}

pub async fn disconnect<C: RobotConnectionPort + ?Sized>(
    control: &C,
    robot: &str,
) -> Result<(), OperationError> {
    control.disconnect(robot).await
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutoConnectStatus {
    Connected,
    Skipped(String),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoConnectOutcome {
    pub robot: String,
    pub status: AutoConnectStatus,
}

/// Try one saved robot without turning an unavailable or incomplete arm into
/// a failure for the persistence action that introduced it.
pub async fn auto_connect_robot<R, C>(store: &R, control: &C, robot: String) -> AutoConnectOutcome
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    let specification = match store.connection_spec(&robot) {
        Ok(specification) => specification,
        Err(error) => {
            return AutoConnectOutcome {
                robot,
                status: AutoConnectStatus::Skipped(error.to_string()),
            };
        }
    };
    let status = match control.connect(specification).await {
        Ok(()) => AutoConnectStatus::Connected,
        Err(error) => AutoConnectStatus::Failed(error.to_string()),
    };
    AutoConnectOutcome { robot, status }
}

/// Try every saved robot independently; one invalid or absent arm must not
/// prevent other usable arms or the Studio itself from starting.
pub async fn auto_connect<R, C>(store: &R, control: &C) -> Vec<AutoConnectOutcome>
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    let mut outcomes = Vec::new();
    for robot in store.saved_robot_ids() {
        outcomes.push(auto_connect_robot(store, control, robot).await);
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn specification(id: &str) -> RobotConnectionSpec {
        RobotConnectionSpec {
            id: id.to_owned(),
            port: SerialPortConfiguration {
                path: format!("/dev/{id}"),
                baud_rate: 1_000_000,
            },
            motors: BTreeMap::new(),
            calibration: None,
        }
    }

    struct Store {
        events: Arc<Mutex<Vec<String>>>,
        ids: Vec<String>,
        invalid: Option<String>,
        fail_delete: bool,
    }

    impl RobotStore for Store {
        type CameraConfiguration = Vec<String>;

        fn create(&self, robot: NewRobot) -> Result<SavedRobot, OperationError> {
            let port = robot
                .port
                .map(|port| format!("{}@{}", port.path, port.baud_rate))
                .unwrap_or_else(|| "none".to_owned());
            self.events
                .lock()
                .unwrap()
                .push(format!("create:{}:{port}", robot.id));
            Ok(SavedRobot { id: robot.id })
        }

        fn import_configuration(&self) -> Result<SavedRobot, OperationError> {
            self.events.lock().unwrap().push("import".to_owned());
            Ok(SavedRobot {
                id: "imported".to_owned(),
            })
        }

        fn export_configuration(
            &self,
            robot: &str,
        ) -> Result<ExportedRobotDocument, OperationError> {
            self.events.lock().unwrap().push(format!("export:{robot}"));
            Ok(ExportedRobotDocument {
                bytes: format!("configuration:{robot}").into_bytes(),
            })
        }

        fn camera_configuration(
            &self,
            robot: &str,
            assignments: &[CameraAssignment],
        ) -> Result<Vec<String>, OperationError> {
            self.events.lock().unwrap().push(format!(
                "cameras:{robot}:{}",
                assignments
                    .iter()
                    .map(|assignment| format!("{}={}", assignment.role, assignment.device_id))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            Ok(assignments
                .iter()
                .map(|assignment| assignment.role.clone())
                .collect())
        }

        fn update(&self, command: RobotUpdate<Vec<String>>) -> Result<SavedRobot, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("update:{}", command.id));
            Ok(SavedRobot { id: command.id })
        }

        fn ensure_exists(&self, robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!("exists:{robot}"));
            Ok(())
        }

        fn delete(&self, robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!("delete:{robot}"));
            if self.fail_delete {
                Err(OperationError::Internal(
                    "storage refused deletion".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        fn saved_robot_ids(&self) -> Vec<String> {
            self.ids.clone()
        }

        fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, OperationError> {
            self.events.lock().unwrap().push(format!("spec:{robot}"));
            if self.invalid.as_deref() == Some(robot) {
                Err(OperationError::InvalidInput("missing port".to_owned()))
            } else {
                Ok(specification(robot))
            }
        }
    }

    struct Control {
        events: Arc<Mutex<Vec<String>>>,
        connected: bool,
        fail_connect: Option<String>,
        fail_forget: bool,
    }

    impl RobotConnectionPort for Control {
        fn is_connected(&self, _robot: &str) -> bool {
            self.connected
        }

        async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("connect:{}", specification.id));
            if self.fail_connect.as_deref() == Some(&specification.id) {
                Err(OperationError::Conflict("arm absent".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn disconnect(&self, robot: &str) -> Result<(), OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("disconnect:{robot}"));
            Ok(())
        }

        async fn forget(&self, robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!("forget:{robot}"));
            if self.fail_forget {
                Err(OperationError::Internal(
                    "control thread stopped".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
    }

    fn setup() -> (Store, Control, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Store {
                events: events.clone(),
                ids: Vec::new(),
                invalid: None,
                fail_delete: false,
            },
            Control {
                events: events.clone(),
                connected: false,
                fail_connect: None,
                fail_forget: false,
            },
            events,
        )
    }

    #[tokio::test]
    async fn deletion_forgets_live_state_before_removing_the_configuration() {
        let (store, control, events) = setup();

        delete(&store, &control, "arm").await.unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
    }

    #[tokio::test]
    async fn deletion_never_removes_the_configuration_if_forgetting_live_state_fails() {
        let (store, mut control, events) = setup();
        control.fail_forget = true;

        delete(&store, &control, "arm").await.unwrap_err();

        assert_eq!(*events.lock().unwrap(), ["exists:arm", "forget:arm"]);
    }

    #[tokio::test]
    async fn delete_failure_leaves_a_saved_but_safely_forgotten_robot() {
        let (mut store, control, events) = setup();
        store.fail_delete = true;

        delete(&store, &control, "arm").await.unwrap_err();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
    }

    #[test]
    fn changing_a_live_port_is_refused_before_persistence() {
        let (store, mut control, events) = setup();
        control.connected = true;
        let error = update(
            &store,
            &control,
            UpdateRobotCommand {
                id: "arm".to_owned(),
                robot_type: None,
                port: Some(SerialPortConfiguration {
                    path: "/dev/new".to_owned(),
                    baud_rate: 1_000_000,
                }),
                cameras: None,
            },
        )
        .unwrap_err();

        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_port_updates_never_check_connection_or_persist() {
        for port in [
            SerialPortConfiguration {
                path: "  ".to_owned(),
                baud_rate: 1_000_000,
            },
            SerialPortConfiguration {
                path: "/dev/arm".to_owned(),
                baud_rate: 0,
            },
        ] {
            let (store, control, events) = setup();
            assert!(matches!(
                update(
                    &store,
                    &control,
                    UpdateRobotCommand {
                        id: "arm".to_owned(),
                        robot_type: None,
                        port: Some(port),
                        cameras: None,
                    }
                ),
                Err(OperationError::InvalidInput(_))
            ));
            assert!(events.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn auto_connect_continues_after_invalid_and_absent_robots() {
        let (mut store, mut control, events) = setup();
        store.ids = vec![
            "invalid".to_owned(),
            "absent".to_owned(),
            "ready".to_owned(),
        ];
        store.invalid = Some("invalid".to_owned());
        control.fail_connect = Some("absent".to_owned());

        let outcomes = auto_connect(&store, &control).await;

        assert!(matches!(outcomes[0].status, AutoConnectStatus::Skipped(_)));
        assert!(matches!(outcomes[1].status, AutoConnectStatus::Failed(_)));
        assert_eq!(outcomes[2].status, AutoConnectStatus::Connected);
        assert_eq!(
            *events.lock().unwrap(),
            [
                "spec:invalid",
                "spec:absent",
                "connect:absent",
                "spec:ready",
                "connect:ready"
            ]
        );
    }

    #[tokio::test]
    async fn auto_connect_one_reports_an_incomplete_robot_without_opening_hardware() {
        let (mut store, control, events) = setup();
        store.invalid = Some("arm".to_owned());

        let outcome = auto_connect_robot(&store, &control, "arm".to_owned()).await;

        assert!(matches!(outcome.status, AutoConnectStatus::Skipped(_)));
        assert_eq!(outcome.robot, "arm");
        assert_eq!(*events.lock().unwrap(), ["spec:arm"]);
    }

    #[tokio::test]
    async fn connect_loads_saved_configuration_and_disconnect_targets_the_live_robot() {
        let (store, control, events) = setup();

        connect(&store, &control, "arm").await.unwrap();
        disconnect(&control, "arm").await.unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["spec:arm", "connect:arm", "disconnect:arm"]
        );
    }

    #[test]
    fn update_persists_plain_changes_when_the_robot_is_disconnected() {
        let (store, control, events) = setup();

        let robot = update(
            &store,
            &control,
            UpdateRobotCommand {
                id: "arm".to_owned(),
                robot_type: Some("so101_leader".to_owned()),
                port: Some(SerialPortConfiguration {
                    path: "/dev/arm".to_owned(),
                    baud_rate: 1_000_000,
                }),
                cameras: Some(vec![CameraAssignment {
                    role: " wrist ".to_owned(),
                    device_id: " camera-1 ".to_owned(),
                }]),
            },
        )
        .unwrap();

        assert_eq!(
            robot,
            SavedRobot {
                id: "arm".to_owned()
            }
        );
        assert_eq!(
            *events.lock().unwrap(),
            ["cameras:arm:wrist=camera-1", "update:arm"]
        );
    }

    #[test]
    fn simple_configuration_actions_delegate_without_learning_serialization_or_storage() {
        let (store, _control, events) = setup();
        assert_eq!(
            create(
                &store,
                CreateRobotCommand {
                    id: "arm".to_owned(),
                    robot_type: None,
                    port: None,
                    baud_rate: None,
                },
            )
            .unwrap(),
            SavedRobot {
                id: "arm".to_owned()
            }
        );
        assert_eq!(
            import_configuration(&store).unwrap(),
            SavedRobot {
                id: "imported".to_owned()
            }
        );
        assert_eq!(
            export_configuration(&store, "arm").unwrap(),
            ExportedRobotDocument {
                bytes: b"configuration:arm".to_vec()
            }
        );
        assert_eq!(
            *events.lock().unwrap(),
            ["create:arm:none", "import", "export:arm"]
        );
    }

    #[test]
    fn creation_normalizes_identity_and_applies_the_default_baud() {
        let (store, _control, events) = setup();
        let robot = create(
            &store,
            CreateRobotCommand {
                id: " arm_1 ".to_owned(),
                robot_type: None,
                port: Some(" /dev/robot ".to_owned()),
                baud_rate: None,
            },
        )
        .unwrap();
        assert_eq!(
            robot,
            SavedRobot {
                id: "arm_1".to_owned()
            }
        );
        assert_eq!(*events.lock().unwrap(), ["create:arm_1:/dev/robot@1000000"]);
    }

    #[test]
    fn invalid_creation_input_never_reaches_persistence() {
        for command in [
            CreateRobotCommand {
                id: "../arm".to_owned(),
                robot_type: None,
                port: None,
                baud_rate: None,
            },
            CreateRobotCommand {
                id: "arm".to_owned(),
                robot_type: None,
                port: Some("/dev/arm".to_owned()),
                baud_rate: Some(0),
            },
            CreateRobotCommand {
                id: "arm".to_owned(),
                robot_type: None,
                port: None,
                baud_rate: Some(1_000_000),
            },
        ] {
            let (store, _control, events) = setup();
            assert!(matches!(
                create(&store, command),
                Err(OperationError::InvalidInput(_))
            ));
            assert!(events.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn invalid_camera_assignments_never_assemble_json_or_persist() {
        for cameras in [
            vec![CameraAssignment {
                role: "bad role".to_owned(),
                device_id: "camera-1".to_owned(),
            }],
            vec![
                CameraAssignment {
                    role: "wrist".to_owned(),
                    device_id: "camera-1".to_owned(),
                },
                CameraAssignment {
                    role: " wrist ".to_owned(),
                    device_id: "camera-2".to_owned(),
                },
            ],
            vec![CameraAssignment {
                role: "wrist".to_owned(),
                device_id: "  ".to_owned(),
            }],
        ] {
            let (store, control, events) = setup();
            assert!(matches!(
                update(
                    &store,
                    &control,
                    UpdateRobotCommand {
                        id: "arm".to_owned(),
                        robot_type: None,
                        port: None,
                        cameras: Some(cameras),
                    }
                ),
                Err(OperationError::InvalidInput(_))
            ));
            assert!(events.lock().unwrap().is_empty());
        }
    }
}
