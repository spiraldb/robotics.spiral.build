//! Whether a training configuration may start a run, which run may be
//! deleted, and which checkpoint may be published as a model.
//!
//! The configuration already carries every value an operator can set; these
//! rules decide whether that combination may start. The training service
//! checks the model's own shape again before it builds the model.
use std::collections::BTreeSet;

use spiral_robotics_common::curation::ViewName;
use spiral_robotics_common::training::{
    ActModel, ModelPublication, ProgressModel, TrainingConfig, TrainingModel, TrainingRun,
    TrainingRunIdentity,
};

use super::service::RunStatus;
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
        let largest_batch = match self.model() {
            TrainingModel::Act(act) => act.admitted()?,
            TrainingModel::Progress(progress) => progress.admitted()?,
        };

        if !(1..=10_000_000).contains(&self.steps()) {
            return invalid("Training steps must be between 1 and 10,000,000.");
        }
        if !(1..=largest_batch).contains(&self.batch_size()) {
            return Err(OperationError::invalid_input(format!(
                "Batch size must be between 1 and {largest_batch}."
            )));
        }
        if !self.validation_split().is_finite() || !(0.0..=0.5).contains(&self.validation_split()) {
            return invalid("Validation split must be between 0% and 50%.");
        }
        if self.checkpoint_every() == 0 || self.checkpoint_every() > self.steps() {
            return invalid("Checkpoint interval must be between 1 and the run step count.");
        }
        if !self.learning_rate().is_finite() || self.learning_rate() <= 0.0 {
            return invalid("Learning rate must be a finite positive number.");
        }
        Ok(self)
    }
}

/// The rules only one family of models has.
trait ModelAdmission {
    /// The largest batch this model trains on, or why it may not start.
    fn admitted(&self) -> Result<usize, OperationError>;
}

impl ModelAdmission for ActModel {
    fn admitted(&self) -> Result<usize, OperationError> {
        let invalid = |message: &str| Err(OperationError::invalid_input(message));
        let observation = self.observation();
        let cameras = observation.cameras();
        if cameras.is_empty() || cameras.len() > 8 {
            return invalid("Train on one to eight cameras.");
        }
        let distinct = cameras.iter().collect::<BTreeSet<_>>();
        if distinct.len() != cameras.len() || cameras.iter().any(|camera| !camera.is_topic()) {
            return invalid("Each camera must be a distinct topic.");
        }
        let image = observation.image();
        if !(32..=2048).contains(&image.width()) || !(32..=2048).contains(&image.height()) {
            return invalid("Image width and height must each be 32 to 2048 pixels.");
        }
        if !observation.rate_hz().is_finite() || !(1.0..=120.0).contains(&observation.rate_hz()) {
            return invalid("The training rate must be 1 to 120 Hz.");
        }
        if !(1..=256).contains(&self.chunk_size()) {
            return invalid("Action horizon must be between 1 and 256.");
        }
        let architecture = self.architecture();
        if architecture.attention_heads() == 0
            || !architecture
                .hidden_dim()
                .is_multiple_of(architecture.attention_heads())
        {
            return invalid("ACT's hidden size must be a multiple of its attention heads.");
        }
        // Each sample carries every camera's frames.
        Ok(512)
    }
}

impl ModelAdmission for ProgressModel {
    fn admitted(&self) -> Result<usize, OperationError> {
        if !self.camera().is_topic() {
            return Err(OperationError::invalid_input(
                "Choose the camera the progress model reads.",
            ));
        }
        // Each sample is one window of the frozen backbone's cached
        // features; WARP-RM's recipe takes 1,024 at once.
        Ok(4_096)
    }
}

/// Whether a camera names a topic at all.
trait CameraTopic {
    fn is_topic(&self) -> bool;
}

impl CameraTopic for str {
    fn is_topic(&self) -> bool {
        !self.trim().is_empty() && !self.chars().any(char::is_control)
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

/// Which checkpoint may be published as a model: one a progress run saved,
/// under a name of its own.
pub(super) struct ModelPublishing;

impl ModelPublishing {
    /// `publication` with its name trimmed, when `run` trained a progress
    /// model and the service reports `status` holding its checkpoint.
    pub(super) fn admitted(
        publication: ModelPublication,
        run: &TrainingRunIdentity,
        status: Option<&RunStatus>,
    ) -> Result<ModelPublication, OperationError> {
        let name = publication.name().trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(OperationError::invalid_input(
                "Name the model in 1 to 80 characters.",
            ));
        }
        let run_name = run.config().run_name();
        if !matches!(run.config().model(), TrainingModel::Progress(_)) {
            return Err(OperationError::invalid_input(format!(
                "Training run {run_name:?} trained no progress model to publish."
            )));
        }
        let step = publication.step();
        if !status.is_some_and(|status| {
            status
                .checkpoints()
                .iter()
                .any(|checkpoint| checkpoint.step() == step)
        }) {
            return Err(OperationError::NotFound(format!(
                "Training run {run_name:?} saved no checkpoint at step {step}."
            )));
        }
        Ok(ModelPublication::new(publication.run_id(), step, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::DatasetSelection;
    use spiral_robotics_common::training::{ImageSize, TrainingModelKind, TrainingSource};

    fn config(kind: TrainingModelKind) -> TrainingConfig {
        let mut config = TrainingConfig::new(
            " act-run-001 ",
            TrainingSource::new(
                DatasetSelection::new(vec!["dataset_abc".into()]).unwrap(),
                "kept",
            ),
            kind,
        );
        match config.model_mut() {
            TrainingModel::Act(act) => act
                .observation_mut()
                .set_cameras(vec!["/top-camera".into()]),
            TrainingModel::Progress(progress) => progress.set_camera("/top-camera"),
        }
        config
    }

    fn act(edit: impl FnOnce(&mut ActModel)) -> TrainingConfig {
        let mut config = config(TrainingModelKind::Act);
        if let TrainingModel::Act(act) = config.model_mut() {
            edit(act);
        }
        config
    }

    #[test]
    fn a_complete_config_starts_with_its_name_trimmed() {
        for kind in TrainingModelKind::ALL {
            assert_eq!(config(kind).validated().unwrap().run_name(), "act-run-001");
        }
    }

    #[test]
    fn cameras_size_and_rate_are_bounded() {
        let none = act(|act| act.observation_mut().set_cameras(Vec::new()));
        assert!(none.validated().is_err());
        let repeated = act(|act| {
            act.observation_mut()
                .set_cameras(vec!["/top".into(), "/top".into()]);
        });
        assert!(repeated.validated().is_err());
        let tiny = act(|act| act.observation_mut().set_image(ImageSize::new(16, 240)));
        assert!(tiny.validated().is_err());
        let fast = act(|act| act.observation_mut().set_rate_hz(500.0));
        assert!(fast.validated().is_err());
    }

    /// A progress model reads one chosen camera and takes WARP-RM's batch,
    /// which an ACT policy may not.
    #[test]
    fn a_progress_model_needs_its_camera_and_takes_a_larger_batch() {
        let mut unchosen = config(TrainingModelKind::Progress);
        if let TrainingModel::Progress(progress) = unchosen.model_mut() {
            progress.set_camera("");
        }
        assert!(unchosen.validated().is_err());
        assert_eq!(
            config(TrainingModelKind::Progress)
                .validated()
                .unwrap()
                .batch_size(),
            1_024
        );
        let mut large = config(TrainingModelKind::Act);
        large.set_batch_size(1_024);
        assert!(large.validated().is_err());
    }

    #[test]
    fn a_view_must_be_a_plain_name() {
        let mut config = config(TrainingModelKind::Act);
        config.set_source(TrainingSource::new(
            DatasetSelection::new(vec!["dataset_abc".into()]).unwrap(),
            "kept; DROP",
        ));
        assert!(config.validated().is_err());
    }
}
