//! Robot models saved on this connector, one file per content address.
//!
//! A model — a URDF and every mesh it names — is large and never changes, so
//! a robot configuration names it by its SHA-256 and the browser fetches it
//! once. Saving the same model twice is the same file.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use spiral_robotics_common::robots::visualization::{RobotModelFiles, RobotModelId};

use super::workspace::WorkspaceError;

pub(crate) struct RobotModelStore {
    directory: PathBuf,
}

impl RobotModelStore {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Keep `model`, returning the address it is served under.
    pub(crate) fn save(&self, model: &RobotModelFiles) -> Result<RobotModelId, WorkspaceError> {
        let id = model.id();
        let file = self.file(&id);
        if file.exists() {
            return Ok(id);
        }
        let failed = |error: std::io::Error| {
            WorkspaceError::Storage(format!("saving robot model {id}: {error}"))
        };
        fs::create_dir_all(&self.directory).map_err(failed)?;
        let bytes = serde_json::to_vec(model)
            .map_err(|error| WorkspaceError::Storage(error.to_string()))?;
        // A crash leaves a partial file under another name, never a torn model.
        let partial = file.with_extension("json.partial");
        fs::write(&partial, bytes).map_err(failed)?;
        fs::rename(&partial, &file).map_err(failed)?;
        Ok(id)
    }

    pub(crate) fn contains(&self, id: &RobotModelId) -> bool {
        self.file(id).exists()
    }

    /// The model's stored JSON, exactly as it is served.
    pub(crate) fn bytes(&self, id: &RobotModelId) -> Result<Vec<u8>, WorkspaceError> {
        fs::read(self.file(id)).map_err(|error| match error.kind() {
            ErrorKind::NotFound => WorkspaceError::ModelNotFound(id.clone()),
            _ => WorkspaceError::Storage(format!("reading robot model {id}: {error}")),
        })
    }

    pub(crate) fn load(&self, id: &RobotModelId) -> Result<RobotModelFiles, WorkspaceError> {
        serde_json::from_slice(&self.bytes(id)?)
            .map_err(|error| WorkspaceError::Storage(format!("reading robot model {id}: {error}")))
    }

    fn file(&self, id: &RobotModelId) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }
}
