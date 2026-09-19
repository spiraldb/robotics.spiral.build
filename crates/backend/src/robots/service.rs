//! Robot-configuration actions independent of HTTP, serialization, storage, and drivers.

use crate::OperationError;
use spiral_robotics_hardware::RobotSpec;

/// Persistence operations needed by robot lifecycle actions.
pub(crate) trait RobotStore {
    async fn ensure_exists(&self, robot: &str) -> Result<(), OperationError>;

    async fn delete(&self, robot: &str) -> Result<(), OperationError>;

    async fn saved_robot_ids(&self) -> Vec<String>;

    async fn connection_spec(&self, robot: &str) -> Result<RobotSpec, OperationError>;
}

/// Live hardware operations needed by robot-configuration actions.
pub(crate) trait RobotConnectionPort {
    async fn connect(&self, specification: RobotSpec) -> Result<(), OperationError>;

    /// Close the bus and remove its connection history.
    async fn forget(&self, robot: &str) -> Result<(), OperationError>;
}

pub(crate) struct RobotActions<'a, R: RobotStore + ?Sized, C: RobotConnectionPort + ?Sized> {
    store: &'a R,
    control: &'a C,
}

impl<'a, R, C> RobotActions<'a, R, C>
where
    R: RobotStore + ?Sized,
    C: RobotConnectionPort + ?Sized,
{
    pub(super) const fn new(store: &'a R, control: &'a C) -> Self {
        Self { store, control }
    }

    /// Forget live hardware before removing its durable configuration.
    ///
    /// The existence check happens first so a missing configuration cannot unexpectedly
    /// tear down a bus. Once it passes, failure is deliberately biased toward a
    /// saved-but-disconnected robot: deleting the SpiralDB row first could leave a
    /// live bus that no configuration or UI card can identify and stop.
    pub(super) async fn delete(&self, robot: &str) -> Result<(), OperationError> {
        self.store.ensure_exists(robot).await?;
        self.control.forget(robot).await?;
        self.store.delete(robot).await
    }

    pub(super) async fn connect(&self, robot: &str) -> Result<(), OperationError> {
        let specification = self.store.connection_spec(robot).await?;
        self.control.connect(specification).await
    }

    /// Try one saved robot without turning an unavailable or incomplete arm into
    /// a failure for the persistence action that introduced it.
    async fn auto_connect_robot(&self, robot: String) -> AutoConnectOutcome {
        let specification = match self.store.connection_spec(&robot).await {
            Ok(specification) => specification,
            Err(error) => {
                return AutoConnectOutcome {
                    robot,
                    status: AutoConnectStatus::Skipped(error.to_string()),
                };
            }
        };
        let status = match self.control.connect(specification).await {
            Ok(()) => AutoConnectStatus::Connected,
            Err(error) => AutoConnectStatus::Failed(error.to_string()),
        };
        AutoConnectOutcome { robot, status }
    }

    /// Try every saved robot independently; one invalid or absent arm must not
    /// prevent other usable arms or the Studio itself from starting.
    pub(super) async fn auto_connect(&self) -> Vec<AutoConnectOutcome> {
        let mut outcomes = Vec::new();
        for robot in self.store.saved_robot_ids().await {
            outcomes.push(self.auto_connect_robot(robot).await);
        }
        outcomes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AutoConnectStatus {
    Connected,
    Skipped(String),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AutoConnectOutcome {
    robot: String,
    status: AutoConnectStatus,
}

impl AutoConnectOutcome {
    pub(super) fn into_parts(self) -> (String, AutoConnectStatus) {
        (self.robot, self.status)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn specification(id: &str) -> RobotSpec {
        RobotSpec::new(
            id.to_owned(),
            spiral_robotics_hardware::PortSettings::try_new(format!("/dev/{id}"), 1_000_000)
                .unwrap(),
            spiral_robotics_hardware::MotorSpec::so101_arm(),
        )
        .unwrap()
    }

    struct Store {
        events: Arc<Mutex<Vec<String>>>,
        ids: Vec<String>,
        invalid: Option<String>,
        fail_delete: bool,
    }

    impl RobotStore for Store {
        async fn ensure_exists(&self, robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!("exists:{robot}"));
            Ok(())
        }

        async fn delete(&self, robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!("delete:{robot}"));
            if self.fail_delete {
                Err(OperationError::Internal(
                    "storage refused deletion".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        async fn saved_robot_ids(&self) -> Vec<String> {
            self.ids.clone()
        }

        async fn connection_spec(&self, robot: &str) -> Result<RobotSpec, OperationError> {
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
        fail_connect: Option<String>,
        fail_forget: bool,
    }

    impl RobotConnectionPort for Control {
        async fn connect(&self, specification: RobotSpec) -> Result<(), OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("connect:{}", specification.id()));
            if self.fail_connect.as_deref() == Some(specification.id()) {
                Err(OperationError::Conflict("arm absent".to_owned()))
            } else {
                Ok(())
            }
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
                fail_connect: None,
                fail_forget: false,
            },
            events,
        )
    }

    #[tokio::test]
    async fn deletion_forgets_live_state_before_removing_the_configuration() {
        let (store, control, events) = setup();

        RobotActions::new(&store, &control)
            .delete("arm")
            .await
            .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
    }

    #[tokio::test]
    async fn deletion_never_removes_the_configuration_if_forgetting_live_state_fails() {
        let (store, mut control, events) = setup();
        control.fail_forget = true;

        RobotActions::new(&store, &control)
            .delete("arm")
            .await
            .unwrap_err();

        assert_eq!(*events.lock().unwrap(), ["exists:arm", "forget:arm"]);
    }

    #[tokio::test]
    async fn delete_failure_leaves_a_saved_but_safely_forgotten_robot() {
        let (mut store, control, events) = setup();
        store.fail_delete = true;

        RobotActions::new(&store, &control)
            .delete("arm")
            .await
            .unwrap_err();

        assert_eq!(
            *events.lock().unwrap(),
            ["exists:arm", "forget:arm", "delete:arm"]
        );
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

        let outcomes = RobotActions::new(&store, &control).auto_connect().await;

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

        let outcome = RobotActions::new(&store, &control)
            .auto_connect_robot("arm".to_owned())
            .await;

        assert!(matches!(outcome.status, AutoConnectStatus::Skipped(_)));
        assert_eq!(outcome.robot, "arm");
        assert_eq!(*events.lock().unwrap(), ["spec:arm"]);
    }

    #[tokio::test]
    async fn connect_loads_saved_configuration() {
        let (store, control, events) = setup();

        RobotActions::new(&store, &control)
            .connect("arm")
            .await
            .unwrap();
        assert_eq!(*events.lock().unwrap(), ["spec:arm", "connect:arm"]);
    }
}
