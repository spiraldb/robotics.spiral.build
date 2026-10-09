//! Robot lifecycle, calibration, and control changes over the robot
//! connector's saved configurations and hardware, each consequential step in
//! order.

use std::collections::BTreeMap;

use crate::OperationError;
use crate::context::RobotConnectorContext;
use spiral_robotics_common::robots::config::RobotConfigSave;
use spiral_robotics_common::robots::{ModeSelection, RobotSummary};
use spiral_robotics_hardware::{JointGoals, NormalizedPosition};

pub(crate) struct RobotOperations<'a> {
    connector: &'a RobotConnectorContext,
}

impl<'a> RobotOperations<'a> {
    pub(crate) fn new(connector: &'a RobotConnectorContext) -> Self {
        Self { connector }
    }

    /// Try every usable saved robot once, when the robot connector starts.
    ///
    /// A failed bus stays disconnected with its error attached; one invalid or
    /// absent arm never prevents the web interface or another arm from starting.
    pub(crate) async fn auto_connect_saved(&self) {
        let records = match self.connector.workspace().try_list().await {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "could not load saved robots for auto-connect");
                return;
            }
        };
        for record in records {
            let robot = record.id();
            match record.connection_spec() {
                Err(error) => {
                    tracing::info!(robot = %robot, "saved robot is not ready for auto-connect: {error}")
                }
                Ok(specification) => match self.connector.hardware().connect(specification).await {
                    Ok(()) => tracing::info!(robot = %robot, "auto-connected saved robot"),
                    Err(error) => tracing::warn!(robot = %robot, "auto-connect failed: {error}"),
                },
            }
        }
    }

    pub(super) async fn list(&self) -> Result<Vec<RobotSummary>, OperationError> {
        let records = self.connector.workspace().try_list().await?;
        let hardware = self.connector.hardware().state();
        Ok(records
            .iter()
            .map(|record| record.live_summary(&hardware))
            .collect())
    }

    pub(super) async fn summary(&self, robot: &str) -> Result<RobotSummary, OperationError> {
        let record = self.connector.workspace().get(robot).await?;
        Ok(record.live_summary(&self.connector.hardware().state()))
    }

    /// A connected robot keeps running the configuration it opened with, so
    /// its port, motors, and calibration only change while it is disconnected.
    pub(super) async fn save(
        &self,
        request: RobotConfigSave,
    ) -> Result<RobotSummary, OperationError> {
        let config = request.config().clone();
        let connected = self
            .connector
            .hardware()
            .state()
            .connections()
            .get(config.id())
            .is_some_and(|status| status.connected());
        if connected {
            let saved = self.connector.workspace().get(config.id()).await?;
            if config.changes_control_configuration(saved.config()) {
                return Err(OperationError::Conflict(
                    "Disconnect the robot before changing its port, motors, calibration, or motion limits"
                        .into(),
                ));
            }
        }
        let record = self
            .connector
            .workspace()
            .save(config, request.expected_revision())
            .await?;
        Ok(record.live_summary(&self.connector.hardware().state()))
    }

    pub(super) async fn connect(&self, robot: &str) -> Result<(), OperationError> {
        let specification = self
            .connector
            .workspace()
            .get(robot)
            .await?
            .connection_spec()?;
        self.connector
            .hardware()
            .connect(specification)
            .await
            .map_err(OperationError::conflict)
    }

    /// Save the simulated demo bench over any robots of its names, then
    /// connect it, so a demo always starts from the bench defined here.
    #[cfg(feature = "simulation")]
    pub(super) async fn install_demo_bench(&self) -> Result<(), OperationError> {
        let hardware = self.connector.hardware();
        for robot in crate::simulation::DemoBench::ROBOTS {
            hardware
                .forget(robot)
                .await
                .map_err(OperationError::conflict)?;
        }
        crate::simulation::DemoBench::install(self.connector.workspace()).await?;
        for robot in crate::simulation::DemoBench::ROBOTS {
            self.connect(robot).await?;
        }
        Ok(())
    }

    /// Forget live hardware before removing its durable configuration.
    ///
    /// The existence check happens first so a missing configuration cannot
    /// unexpectedly tear down a bus. Once it passes, failure is deliberately
    /// biased toward a saved-but-disconnected robot: deleting the configuration
    /// first could leave a live bus that no configuration or UI card can
    /// identify and stop.
    pub(super) async fn delete(&self, robot: &str) -> Result<(), OperationError> {
        let workspace = self.connector.workspace();
        workspace.get(robot).await?;
        self.connector
            .hardware()
            .forget(robot)
            .await
            .map_err(OperationError::conflict)?;
        Ok(workspace.delete(robot).await?)
    }

    /// Finish a sweep, persist its ranges, then refresh live control.
    ///
    /// The ordering is part of the product contract: a read or write failure
    /// keeps the sweep alive so the operator does not lose collected motion.
    /// Once persistence succeeds, later hardware failures are only logged,
    /// because the next connection loads the already-correct saved robot.
    pub(super) async fn finish_calibration(
        &self,
        robot: &str,
    ) -> Result<RobotSummary, OperationError> {
        let hardware = self.connector.hardware();
        let ranges = hardware
            .calibration_ranges(robot)
            .await
            .map_err(OperationError::conflict)?;
        self.connector
            .workspace()
            .save_calibration(robot, ranges.clone())
            .await?;
        let warnings = [
            hardware
                .cancel_calibration(robot)
                .await
                .err()
                .map(|error| format!("could not close the completed sweep: {error}")),
            hardware
                .apply_calibration(robot, ranges)
                .await
                .err()
                .map(|error| format!("could not refresh the live robot: {error}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !warnings.is_empty() {
            tracing::warn!(
                robot = %robot,
                "saved calibration but live refresh was incomplete: {}",
                warnings.join("; ")
            );
        }
        self.summary(robot).await
    }

    pub(super) async fn set_goals(
        &self,
        robot: &str,
        normalized: BTreeMap<String, f64>,
    ) -> Result<(), OperationError> {
        let goals = Self::joint_goals(normalized)?;
        self.connector
            .hardware()
            .set_goals(robot, goals)
            .await
            .map_err(OperationError::conflict)
    }

    pub(super) async fn set_mode(&self, selection: ModeSelection) -> Result<(), OperationError> {
        Self::admit_operator_mode(&selection)?;
        self.connector
            .hardware()
            .set_mode(selection)
            .await
            .map_err(OperationError::conflict)
    }

    fn joint_goals(normalized: BTreeMap<String, f64>) -> Result<JointGoals, OperationError> {
        if normalized.is_empty() {
            return Err(OperationError::invalid_input(
                "no joints were given a target",
            ));
        }
        let normalized = normalized
            .into_iter()
            .map(|(joint, value)| match NormalizedPosition::new(value) {
                Ok(position) => Ok((joint, position)),
                Err(error) => Err(OperationError::InvalidInput(format!(
                    "joint {joint:?} has an invalid target: {error}"
                ))),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        JointGoals::new(normalized).map_err(|error| OperationError::conflict(error.to_string()))
    }

    /// Modes an operator may enter directly.
    fn admit_operator_mode(selection: &ModeSelection) -> Result<(), OperationError> {
        let refusal = match selection {
            ModeSelection::Idle => None,
            ModeSelection::Manual { robot } if robot.trim().is_empty() => {
                Some("manual control requires a robot")
            }
            ModeSelection::Manual { .. } => None,
            ModeSelection::Teleoperate { leader, follower }
                if leader.trim().is_empty() || follower.trim().is_empty() =>
            {
                Some("teleoperation requires a leader and follower")
            }
            ModeSelection::Teleoperate { leader, follower } if leader == follower => {
                Some("teleoperation leader and follower must be different robots")
            }
            ModeSelection::Teleoperate { .. } => None,
        };
        refusal.map_or(Ok(()), |refusal| {
            Err(OperationError::invalid_input(refusal))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_joint_goals_are_refused() {
        for normalized in [
            BTreeMap::new(),
            BTreeMap::from([("shoulder".to_owned(), f64::NAN)]),
            BTreeMap::from([("shoulder".to_owned(), f64::INFINITY)]),
            BTreeMap::from([("shoulder".to_owned(), f64::from_bits(1.0_f64.to_bits() + 1))]),
            BTreeMap::from([(
                "shoulder".to_owned(),
                f64::from_bits((-1.0_f64).to_bits() + 1),
            )]),
        ] {
            assert!(matches!(
                RobotOperations::joint_goals(normalized),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn exact_normalized_boundaries_are_goals() {
        for value in [-1.0, 1.0] {
            RobotOperations::joint_goals(BTreeMap::from([("shoulder".to_owned(), value)])).unwrap();
        }
    }

    #[test]
    fn operators_enter_only_well_formed_direct_modes() {
        let teleoperate = |leader: &str, follower: &str| ModeSelection::Teleoperate {
            leader: leader.to_owned(),
            follower: follower.to_owned(),
        };
        for admitted in [
            ModeSelection::Idle,
            ModeSelection::Manual {
                robot: "arm".to_owned(),
            },
            teleoperate("leader", "follower"),
        ] {
            RobotOperations::admit_operator_mode(&admitted).unwrap();
        }
        for refused in [
            ModeSelection::Manual {
                robot: " ".to_owned(),
            },
            teleoperate("arm", "arm"),
            teleoperate("", "follower"),
        ] {
            assert!(matches!(
                RobotOperations::admit_operator_mode(&refused),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }
}
