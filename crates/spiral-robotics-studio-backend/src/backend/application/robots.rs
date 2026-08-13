//! Robot-configuration actions independent of HTTP, JSON, files, and drivers.

use std::collections::BTreeMap;

use spiral_robotics_core::calibration::{Calibration, MotorSpec};

use super::error::ApplicationError;

const DEFAULT_ROBOT_BAUD_RATE: u32 = 1_000_000;
const MAX_ROBOT_ID_LENGTH: usize = 64;
const MAX_CAMERA_ROLE_LENGTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SerialPortConfiguration {
    pub path: String,
    pub baud_rate: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CreateRobotCommand {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<String>,
    pub baud_rate: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NewRobot {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CameraAssignment {
    pub role: String,
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UpdateRobotCommand {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
    pub cameras: Option<Vec<CameraAssignment>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RobotUpdate<C> {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<SerialPortConfiguration>,
    pub cameras: Option<C>,
}

/// Plain information required to connect a saved robot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RobotConnectionSpec {
    pub id: String,
    pub port: SerialPortConfiguration,
    pub motors: BTreeMap<String, MotorSpec>,
    pub calibration: Option<Calibration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SavedRobot {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportedRobotDocument {
    pub bytes: Vec<u8>,
}

/// Persistence operations needed by robot-configuration actions.
///
/// Camera patches are adapter-minted capabilities consumed by the same
/// repository after application validation.
pub(crate) trait RobotRepository {
    type CameraConfiguration;

    fn create(&self, robot: NewRobot) -> Result<SavedRobot, ApplicationError>;

    fn import_configuration(&self) -> Result<SavedRobot, ApplicationError>;

    fn export_configuration(&self, robot: &str) -> Result<ExportedRobotDocument, ApplicationError>;

    fn camera_configuration(
        &self,
        robot: &str,
        assignments: &[CameraAssignment],
    ) -> Result<Self::CameraConfiguration, ApplicationError>;

    fn update(
        &self,
        command: RobotUpdate<Self::CameraConfiguration>,
    ) -> Result<SavedRobot, ApplicationError>;

    fn ensure_exists(&self, robot: &str) -> Result<(), ApplicationError>;

    fn delete(&self, robot: &str) -> Result<(), ApplicationError>;

    fn saved_robot_ids(&self) -> Vec<String>;

    fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, ApplicationError>;
}

/// Live hardware operations needed by robot-configuration actions.
pub(crate) trait RobotConnectionPort {
    fn is_connected(&self, robot: &str) -> bool;

    async fn connect(&self, specification: RobotConnectionSpec) -> Result<(), ApplicationError>;

    async fn disconnect(&self, robot: &str) -> Result<(), ApplicationError>;

    /// Close the bus and remove its connection history.
    async fn forget(&self, robot: &str) -> Result<(), ApplicationError>;
}

pub(crate) fn create<R: RobotRepository + ?Sized>(
    repository: &R,
    mut command: CreateRobotCommand,
) -> Result<SavedRobot, ApplicationError> {
    command.id = command.id.trim().to_owned();
    validate_plain_name("robot id", &command.id, MAX_ROBOT_ID_LENGTH)?;
    let port = match (command.port, command.baud_rate) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(ApplicationError::InvalidInput(
                "a baud rate can only be set with a serial port".to_owned(),
            ));
        }
        (Some(path), baud_rate) => Some(validated_port(
            path,
            baud_rate.unwrap_or(DEFAULT_ROBOT_BAUD_RATE),
        )?),
    };
    repository.create(NewRobot {
        id: command.id,
        robot_type: command.robot_type,
        port,
    })
}

pub(crate) fn import_configuration<R: RobotRepository + ?Sized>(
    repository: &R,
) -> Result<SavedRobot, ApplicationError> {
    repository.import_configuration()
}

/// Select saved robot identities for a read presenter.
pub(crate) fn list<R: RobotRepository + ?Sized>(repository: &R) -> Vec<SavedRobot> {
    repository
        .saved_robot_ids()
        .into_iter()
        .map(|id| SavedRobot { id })
        .collect()
}

/// Admit one saved identity before an outer presenter hydrates its view.
pub(crate) fn get<R: RobotRepository + ?Sized>(
    repository: &R,
    robot: String,
) -> Result<SavedRobot, ApplicationError> {
    validate_plain_name("robot id", &robot, MAX_ROBOT_ID_LENGTH)?;
    repository.ensure_exists(&robot)?;
    Ok(SavedRobot { id: robot })
}

pub(crate) fn export_configuration<R: RobotRepository + ?Sized>(
    repository: &R,
    robot: &str,
) -> Result<ExportedRobotDocument, ApplicationError> {
    repository.export_configuration(robot)
}

pub(crate) fn update<R, C>(
    repository: &R,
    control: &C,
    mut command: UpdateRobotCommand,
) -> Result<SavedRobot, ApplicationError>
where
    R: RobotRepository + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    if let Some(port) = command.port.take() {
        command.port = Some(validated_port(port.path, port.baud_rate)?);
        if control.is_connected(&command.id) {
            return Err(ApplicationError::Conflict(format!(
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
                    return Err(ApplicationError::InvalidInput(format!(
                        "camera role {:?} is used more than once",
                        assignment.role
                    )));
                }
                if assignment.device_id.is_empty() || assignment.device_id.contains('\0') {
                    return Err(ApplicationError::InvalidInput(format!(
                        "camera role {:?} needs a device id without NUL bytes",
                        assignment.role
                    )));
                }
            }
            Some(repository.camera_configuration(&command.id, &assignments)?)
        }
        None => None,
    };
    repository.update(RobotUpdate {
        id: command.id,
        robot_type: command.robot_type,
        port: command.port,
        cameras,
    })
}

fn validated_port(
    path: String,
    baud_rate: u32,
) -> Result<SerialPortConfiguration, ApplicationError> {
    let path = path.trim().to_owned();
    if path.is_empty() || path.contains('\0') || baud_rate == 0 {
        return Err(ApplicationError::InvalidInput(
            "a port needs a non-empty path without NUL bytes and a positive baud rate".to_owned(),
        ));
    }
    Ok(SerialPortConfiguration { path, baud_rate })
}

fn validate_plain_name(label: &str, value: &str, maximum: usize) -> Result<(), ApplicationError> {
    if value.is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApplicationError::InvalidInput(format!(
            "{label} must be 1 to {maximum} characters of letters, digits, '-', or '_'"
        )));
    }
    Ok(())
}

/// Forget live hardware before removing its durable configuration.
///
/// The existence check happens first so a missing file cannot unexpectedly
/// tear down a bus. Once it passes, failure is deliberately biased toward a
/// saved-but-disconnected robot: deleting the file first could leave a live bus
/// that no configuration or UI card can identify and stop.
pub(crate) async fn delete<R, C>(
    repository: &R,
    control: &C,
    robot: &str,
) -> Result<(), ApplicationError>
where
    R: RobotRepository + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    repository.ensure_exists(robot)?;
    control.forget(robot).await?;
    repository.delete(robot)
}

pub(crate) async fn connect<R, C>(
    repository: &R,
    control: &C,
    robot: &str,
) -> Result<(), ApplicationError>
where
    R: RobotRepository + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    let specification = repository.connection_spec(robot)?;
    control.connect(specification).await
}

pub(crate) async fn disconnect<C: RobotConnectionPort + ?Sized>(
    control: &C,
    robot: &str,
) -> Result<(), ApplicationError> {
    control.disconnect(robot).await
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AutoConnectStatus {
    Connected,
    Skipped(String),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AutoConnectOutcome {
    pub robot: String,
    pub status: AutoConnectStatus,
}

/// Try every saved robot independently; one invalid or absent arm must not
/// prevent other usable arms or the Studio itself from starting.
pub(crate) async fn auto_connect<R, C>(repository: &R, control: &C) -> Vec<AutoConnectOutcome>
where
    R: RobotRepository + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    let mut outcomes = Vec::new();
    for robot in repository.saved_robot_ids() {
        let specification = match repository.connection_spec(&robot) {
            Ok(specification) => specification,
            Err(error) => {
                outcomes.push(AutoConnectOutcome {
                    robot,
                    status: AutoConnectStatus::Skipped(error.to_string()),
                });
                continue;
            }
        };
        let status = match control.connect(specification).await {
            Ok(()) => AutoConnectStatus::Connected,
            Err(error) => AutoConnectStatus::Failed(error.to_string()),
        };
        outcomes.push(AutoConnectOutcome { robot, status });
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

    struct Repository {
        events: Arc<Mutex<Vec<String>>>,
        ids: Vec<String>,
        invalid: Option<String>,
        fail_delete: bool,
    }

    impl RobotRepository for Repository {
        type CameraConfiguration = Vec<String>;

        fn create(&self, robot: NewRobot) -> Result<SavedRobot, ApplicationError> {
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

        fn import_configuration(&self) -> Result<SavedRobot, ApplicationError> {
            self.events.lock().unwrap().push("import".to_owned());
            Ok(SavedRobot {
                id: "imported".to_owned(),
            })
        }

        fn export_configuration(
            &self,
            robot: &str,
        ) -> Result<ExportedRobotDocument, ApplicationError> {
            self.events.lock().unwrap().push(format!("export:{robot}"));
            Ok(ExportedRobotDocument {
                bytes: format!("configuration:{robot}").into_bytes(),
            })
        }

        fn camera_configuration(
            &self,
            robot: &str,
            assignments: &[CameraAssignment],
        ) -> Result<Vec<String>, ApplicationError> {
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

        fn update(
            &self,
            command: RobotUpdate<Vec<String>>,
        ) -> Result<SavedRobot, ApplicationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("update:{}", command.id));
            Ok(SavedRobot { id: command.id })
        }

        fn ensure_exists(&self, robot: &str) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push(format!("exists:{robot}"));
            Ok(())
        }

        fn delete(&self, robot: &str) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push(format!("delete:{robot}"));
            if self.fail_delete {
                Err(ApplicationError::Internal(
                    "disk refused deletion".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        fn saved_robot_ids(&self) -> Vec<String> {
            self.ids.clone()
        }

        fn connection_spec(&self, robot: &str) -> Result<RobotConnectionSpec, ApplicationError> {
            self.events.lock().unwrap().push(format!("spec:{robot}"));
            if self.invalid.as_deref() == Some(robot) {
                Err(ApplicationError::InvalidInput("missing port".to_owned()))
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

        async fn connect(
            &self,
            specification: RobotConnectionSpec,
        ) -> Result<(), ApplicationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("connect:{}", specification.id));
            if self.fail_connect.as_deref() == Some(&specification.id) {
                Err(ApplicationError::Conflict("arm absent".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn disconnect(&self, robot: &str) -> Result<(), ApplicationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("disconnect:{robot}"));
            Ok(())
        }

        async fn forget(&self, robot: &str) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push(format!("forget:{robot}"));
            if self.fail_forget {
                Err(ApplicationError::Internal(
                    "control thread stopped".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
    }

    fn setup() -> (Repository, Control, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Repository {
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
    async fn deletion_forgets_live_state_before_removing_the_file() {
        let (repository, control, events) = setup();

        delete(&repository, &control, "arm").await.unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
    }

    #[tokio::test]
    async fn deletion_never_removes_the_file_if_forgetting_live_state_fails() {
        let (repository, mut control, events) = setup();
        control.fail_forget = true;

        delete(&repository, &control, "arm").await.unwrap_err();

        assert_eq!(*events.lock().unwrap(), ["exists:arm", "forget:arm"]);
    }

    #[tokio::test]
    async fn delete_failure_leaves_a_saved_but_safely_forgotten_robot() {
        let (mut repository, control, events) = setup();
        repository.fail_delete = true;

        delete(&repository, &control, "arm").await.unwrap_err();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
    }

    #[test]
    fn changing_a_live_port_is_refused_before_persistence() {
        let (repository, mut control, events) = setup();
        control.connected = true;
        let error = update(
            &repository,
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

        assert!(matches!(error, ApplicationError::Conflict(_)));
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
            let (repository, control, events) = setup();
            assert!(matches!(
                update(
                    &repository,
                    &control,
                    UpdateRobotCommand {
                        id: "arm".to_owned(),
                        robot_type: None,
                        port: Some(port),
                        cameras: None,
                    }
                ),
                Err(ApplicationError::InvalidInput(_))
            ));
            assert!(events.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn auto_connect_continues_after_invalid_and_absent_robots() {
        let (mut repository, mut control, events) = setup();
        repository.ids = vec![
            "invalid".to_owned(),
            "absent".to_owned(),
            "ready".to_owned(),
        ];
        repository.invalid = Some("invalid".to_owned());
        control.fail_connect = Some("absent".to_owned());

        let outcomes = auto_connect(&repository, &control).await;

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
    async fn connect_loads_saved_configuration_and_disconnect_targets_the_live_robot() {
        let (repository, control, events) = setup();

        connect(&repository, &control, "arm").await.unwrap();
        disconnect(&control, "arm").await.unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["spec:arm", "connect:arm", "disconnect:arm"]
        );
    }

    #[test]
    fn update_persists_plain_changes_when_the_robot_is_disconnected() {
        let (repository, control, events) = setup();

        let robot = update(
            &repository,
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
    fn simple_configuration_actions_delegate_without_learning_json_or_files() {
        let (repository, _control, events) = setup();
        assert_eq!(
            create(
                &repository,
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
            import_configuration(&repository).unwrap(),
            SavedRobot {
                id: "imported".to_owned()
            }
        );
        assert_eq!(
            export_configuration(&repository, "arm").unwrap(),
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
        let (repository, _control, events) = setup();
        let robot = create(
            &repository,
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
            let (repository, _control, events) = setup();
            assert!(matches!(
                create(&repository, command),
                Err(ApplicationError::InvalidInput(_))
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
            let (repository, control, events) = setup();
            assert!(matches!(
                update(
                    &repository,
                    &control,
                    UpdateRobotCommand {
                        id: "arm".to_owned(),
                        robot_type: None,
                        port: None,
                        cameras: Some(cameras),
                    }
                ),
                Err(ApplicationError::InvalidInput(_))
            ));
            assert!(events.lock().unwrap().is_empty());
        }
    }
}
