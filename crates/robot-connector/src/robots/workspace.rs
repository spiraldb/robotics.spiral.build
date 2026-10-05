//! Robot configurations saved on this connector.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::OperationError;

use spiral_robotics_common::robots::config::{MotorRange, RobotConfig};
use spiral_robotics_common::robots::{ConnectionStatus, RobotSummary};

use crate::sync::MutexRecovery;

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

/// A saved robot and the revision it was read at.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
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

/// The robots saved on this connector, kept in one JSON file that every
/// change rewrites whole.
pub(crate) struct RobotWorkspace {
    file: PathBuf,
    saved: Mutex<BTreeMap<String, RobotRecord>>,
}

impl RobotWorkspace {
    /// The robots saved in `file`; none before the first save.
    pub(crate) fn open(file: PathBuf) -> Result<Self, WorkspaceError> {
        let saved = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                WorkspaceError::Storage(format!("reading {}: {error}", file.display()))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                return Err(WorkspaceError::Storage(format!(
                    "reading {}: {error}",
                    file.display()
                )));
            }
        };
        Ok(Self {
            file,
            saved: Mutex::new(saved),
        })
    }

    pub(crate) async fn list(&self) -> Vec<RobotRecord> {
        self.try_list().await.unwrap_or_else(|error| {
            tracing::warn!(%error, "Could not load robot configurations");
            Vec::new()
        })
    }

    pub(crate) async fn try_list(&self) -> Result<Vec<RobotRecord>, WorkspaceError> {
        Ok(self.saved.recover_lock().values().cloned().collect())
    }

    pub(crate) async fn get(&self, id: &str) -> Result<RobotRecord, WorkspaceError> {
        self.saved
            .recover_lock()
            .get(id)
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound(id.to_owned()))
    }

    /// Save a complete configuration over the revision its editor started from.
    pub(super) async fn save(
        &self,
        config: RobotConfig,
        expected_revision: Option<u64>,
    ) -> Result<RobotRecord, WorkspaceError> {
        let mut saved = self.saved.recover_lock();
        let current = saved.get(config.id()).map(|record| record.revision);
        if current != expected_revision {
            return Err(WorkspaceError::Conflict(
                "Robot configuration changed; reload it before saving".into(),
            ));
        }
        let record = RobotRecord {
            config,
            revision: expected_revision.map_or(1, |revision| revision + 1),
        };
        let mut next = saved.clone();
        next.insert(record.id().to_owned(), record.clone());
        self.write(&next)?;
        *saved = next;
        Ok(record)
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
        let mut saved = self.saved.recover_lock();
        if !saved.contains_key(id) {
            return Err(WorkspaceError::NotFound(id.to_owned()));
        }
        let mut next = saved.clone();
        next.remove(id);
        self.write(&next)?;
        *saved = next;
        Ok(())
    }

    /// Replace the file whole, so a crash leaves either version but never a
    /// torn one.
    fn write(&self, robots: &BTreeMap<String, RobotRecord>) -> Result<(), WorkspaceError> {
        let failed =
            |error: std::io::Error| WorkspaceError::Storage(format!("saving robots: {error}"));
        let parent = self
            .file
            .parent()
            .ok_or_else(|| WorkspaceError::Storage("robot file has no directory".into()))?;
        fs::create_dir_all(parent).map_err(failed)?;
        let bytes = serde_json::to_vec_pretty(robots)
            .map_err(|error| WorkspaceError::Storage(error.to_string()))?;
        let partial = self.file.with_extension("json.partial");
        fs::write(&partial, bytes).map_err(failed)?;
        fs::rename(&partial, &self.file).map_err(failed)
    }
}

#[cfg(test)]
mod tests;
