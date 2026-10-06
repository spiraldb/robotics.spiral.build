//! Whether a training configuration may start a run.
//!
//! The configuration already carries every value an operator can set; these
//! rules decide whether that combination may start. The training service
//! checks the model's own shape again before it builds the model.
use std::collections::BTreeSet;

use spiral_robotics_common::curation::ViewName;
use spiral_robotics_common::training::{TrainingConfig, TrainingRun};

use crate::OperationError;

pub(super) trait StartTrainingAdmission: Sized {
    /// This configuration with its run name trimmed, or why it may not start.
    fn validated(self) -> Result<Self, OperationError>;
}

impl StartTrainingAdmission for TrainingConfig {
    fn validated(mut self) -> Result<Self, OperationError> {
        let invalid = |message: &str| Err(OperationError::invalid_input(message));
        let run_name = self.run_name().trim().to_owned();
        if run_name.is_empty()
            || run_name.len() > 80
            || !run_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return invalid("Run name must use 1–80 letters, numbers, '.', '_' or '-' characters.");
        }
        self.set_run_name(run_name);
        ViewName::try_from(self.source().view()).map_err(OperationError::InvalidInput)?;

        let observation = self.observation();
        let cameras = observation.cameras();
        if cameras.is_empty() || cameras.len() > 8 {
            return invalid("Train on one to eight cameras.");
        }
        let distinct = cameras.iter().collect::<BTreeSet<_>>();
        if distinct.len() != cameras.len()
            || cameras
                .iter()
                .any(|camera| camera.trim().is_empty() || camera.chars().any(char::is_control))
        {
            return invalid("Each camera must be a distinct topic.");
        }
        let image = observation.image();
        if !(32..=2048).contains(&image.width()) || !(32..=2048).contains(&image.height()) {
            return invalid("Image width and height must each be 32 to 2048 pixels.");
        }
        if !observation.rate_hz().is_finite() || !(1.0..=120.0).contains(&observation.rate_hz()) {
            return invalid("The training rate must be 1 to 120 Hz.");
        }

        if !(1..=10_000_000).contains(&self.steps()) {
            return invalid("Training steps must be between 1 and 10,000,000.");
        }
        if !(1..=512).contains(&self.batch_size()) {
            return invalid("Batch size must be between 1 and 512.");
        }
        if !self.validation_split().is_finite() || !(0.0..=0.5).contains(&self.validation_split()) {
            return invalid("Validation split must be between 0% and 50%.");
        }
        if !(1..=256).contains(&self.chunk_size()) {
            return invalid("Action horizon must be between 1 and 256.");
        }
        if self.checkpoint_every() == 0 || self.checkpoint_every() > self.steps() {
            return invalid("Checkpoint interval must be between 1 and the run step count.");
        }
        if !self.learning_rate().is_finite() || self.learning_rate() <= 0.0 {
            return invalid("Learning rate must be a finite positive number.");
        }
        let act = self.act();
        if act.attention_heads() == 0 || !act.hidden_dim().is_multiple_of(act.attention_heads()) {
            return invalid("ACT's hidden size must be a multiple of its attention heads.");
        }
        Ok(self)
    }
}

/// Which recorded run may be deleted: a known, settled one.
pub(super) struct RunDeletion;

impl RunDeletion {
    pub(super) fn admitted(runs: &[TrainingRun], run_id: &str) -> Result<String, OperationError> {
        let Some(run) = runs.iter().find(|run| run.id() == run_id) else {
            return Err(OperationError::NotFound(format!(
                "Training run {run_id:?} is no longer available."
            )));
        };
        if run.phase().active() {
            return Err(OperationError::Conflict(format!(
                "Training run {run_id:?} is still active; stop it before deleting it."
            )));
        }
        Ok(run_id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::DatasetSelection;
    use spiral_robotics_common::training::{ImageSize, TrainingSource};

    fn config() -> TrainingConfig {
        let mut config = TrainingConfig::new(
            " act-run-001 ",
            TrainingSource::new(
                DatasetSelection::new(vec!["dataset_abc".into()]).unwrap(),
                "kept",
            ),
        );
        config
            .observation_mut()
            .set_cameras(vec!["/top-camera".into()]);
        config
    }

    #[test]
    fn a_complete_config_starts_with_its_name_trimmed() {
        assert_eq!(config().validated().unwrap().run_name(), "act-run-001");
    }

    #[test]
    fn cameras_size_and_rate_are_bounded() {
        let mut none = config();
        none.observation_mut().set_cameras(Vec::new());
        assert!(none.validated().is_err());
        let mut repeated = config();
        repeated
            .observation_mut()
            .set_cameras(vec!["/top".into(), "/top".into()]);
        assert!(repeated.validated().is_err());
        let mut tiny = config();
        tiny.observation_mut().set_image(ImageSize::new(16, 240));
        assert!(tiny.validated().is_err());
        let mut fast = config();
        fast.observation_mut().set_rate_hz(500.0);
        assert!(fast.validated().is_err());
    }

    #[test]
    fn a_view_must_be_a_plain_name() {
        let mut config = config();
        config.set_source(TrainingSource::new(
            DatasetSelection::new(vec!["dataset_abc".into()]).unwrap(),
            "kept; DROP",
        ));
        assert!(config.validated().is_err());
    }
}
