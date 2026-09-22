//! Saved robot configurations in the local Studio database.

use std::collections::BTreeMap;
use std::fmt;

use crate::OperationError;

use spiral_robotics_common::robots::config::{MotorRange, RobotConfig};
use spiral_robotics_common::robots::{ConnectionStatus, RobotSummary};
use spiral_robotics_storage::StudioProject;

#[derive(Debug)]
pub(crate) enum WorkspaceError {
    NotFound(String),
    Conflict(String),
    Invalid(String),
    Storage(String),
}

impl From<WorkspaceError> for OperationError {
    fn from(error: WorkspaceError) -> Self {
        let message = error.to_string();
        match error {
            WorkspaceError::Invalid(_) => Self::InvalidInput(message),
            WorkspaceError::NotFound(_) => Self::NotFound(message),
            WorkspaceError::Conflict(_) => Self::Conflict(message),
            WorkspaceError::Storage(_) => Self::Internal(message),
        }
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(formatter, "no robot named {id:?}"),
            Self::Conflict(detail) | Self::Invalid(detail) | Self::Storage(detail) => {
                formatter.write_str(detail)
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// A saved robot and the storage revision it was read at.
#[derive(Clone, Debug)]
pub(crate) struct RobotRecord {
    config: RobotConfig,
    revision: u64,
}

impl RobotRecord {
    pub(crate) fn id(&self) -> &str {
        self.config.id()
    }

    pub(crate) const fn config(&self) -> &RobotConfig {
        &self.config
    }

    pub(crate) fn summary(&self, connection: ConnectionStatus) -> RobotSummary {
        RobotSummary::new(self.config.clone(), self.revision, connection)
    }

    pub(super) fn live_summary(
        &self,
        hardware: &spiral_robotics_hardware::HardwareState,
    ) -> RobotSummary {
        let connection = hardware
            .connections()
            .get(self.id())
            .cloned()
            .unwrap_or_default();
        let summary = self.summary(connection);
        match hardware.limits().get(self.id()) {
            Some(limits) => summary.with_motor_limits(limits.clone()),
            None => summary,
        }
    }

    pub(super) fn connection_spec(
        &self,
    ) -> Result<spiral_robotics_hardware::RobotSpec, OperationError> {
        spiral_robotics_hardware::RobotSpec::try_from(self.config.clone())
            .map_err(|error| OperationError::invalid_input(error.to_string()))
    }
}

pub(crate) struct RobotWorkspace {
    project_store: StudioProject,
}

impl RobotWorkspace {
    pub(crate) fn new(project_store: StudioProject) -> Self {
        Self { project_store }
    }

    pub(crate) async fn list(&self) -> Vec<RobotRecord> {
        self.try_list().await.unwrap_or_else(|error| {
            tracing::warn!(%error, "Could not load robot configurations");
            Vec::new()
        })
    }

    pub(crate) async fn try_list(&self) -> Result<Vec<RobotRecord>, WorkspaceError> {
        Ok(self
            .project_store
            .robot_configs()
            .await
            .map_err(WorkspaceError::Storage)?
            .into_iter()
            .map(|(config, revision)| RobotRecord { config, revision })
            .collect())
    }

    pub(crate) async fn get(&self, id: &str) -> Result<RobotRecord, WorkspaceError> {
        let (config, revision) = self
            .project_store
            .robot_config(id)
            .await
            .map_err(WorkspaceError::Storage)?
            .ok_or_else(|| WorkspaceError::NotFound(id.to_owned()))?;
        Ok(RobotRecord { config, revision })
    }

    /// Save a complete configuration over the revision its editor started from.
    pub(super) async fn save(
        &self,
        config: RobotConfig,
        expected_revision: Option<u64>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let revision = self
            .project_store
            .save_robot_config(&config, expected_revision)
            .await
            .map_err(WorkspaceError::Storage)?
            .ok_or_else(|| {
                WorkspaceError::Conflict(
                    "Robot configuration changed; reload it before saving".into(),
                )
            })?;
        Ok(RobotRecord { config, revision })
    }

    /// Record a sweep's ranges on the saved robot.
    pub(super) async fn save_calibration(
        &self,
        id: &str,
        ranges: BTreeMap<String, MotorRange>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let record = self.get(id).await?;
        let config = record
            .config
            .with_calibration(ranges)
            .map_err(WorkspaceError::Invalid)?;
        self.save(config, Some(record.revision)).await
    }

    pub(super) async fn delete(&self, id: &str) -> Result<(), WorkspaceError> {
        if self
            .project_store
            .delete_robot_config(id)
            .await
            .map_err(WorkspaceError::Storage)?
        {
            Ok(())
        } else {
            Err(WorkspaceError::NotFound(id.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests;
