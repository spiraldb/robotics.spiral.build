//! Framework-neutral training and policy-rollout capability actions.

use std::collections::BTreeSet;

use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::rollout::{MAX_ROLLOUT_DURATION_SECONDS, RolloutConfig};
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};
use spiral_robotics_common::training::{TrainingConfig, TrainingSnapshot};

use crate::OperationError;
use crate::rollout::service::{MIN_ROLLOUT_DURATION_SECONDS, RolloutDuration};
use crate::training::manager::TrainingManager;

/// One image batch expanded to f32 model inputs must leave enough memory for
/// model activations, optimizer state, and the hardware-control process itself.
pub(crate) const MAX_TRAINING_IMAGE_BATCH_BYTES: usize = 256 * 1024 * 1024;

/// Aggregate ACT compute-session budget. Independently valid dimensions must
/// not combine into a multi-gigabyte optimizer allocation that can terminate
/// the selected remote runtime.
const MAX_ACT_TRAINING_RESOURCE_BYTES: usize = 2 * 1024 * 1024 * 1024;
const ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES: usize = 256 * 1024 * 1024;
const ACT_IMAGE_RUNTIME_MULTIPLIER: usize = 4;
/// Parameter + gradient + two AdamW moments, plus one parameter-sized runtime
/// workspace reserve. All ACT trainable tensors use f32 today.
const ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER: usize = 5 * std::mem::size_of::<f32>();

struct ActTrainingResourceEstimate(Option<usize>);

impl ActTrainingResourceEstimate {
    fn for_command(command: &TrainingConfig, runtime_image_bytes: usize) -> Self {
        Self(command.estimated_act_resource_bytes(runtime_image_bytes))
    }

    fn enforce(self) -> Result<(), OperationError> {
        if self
            .0
            .is_none_or(|bytes| bytes > MAX_ACT_TRAINING_RESOURCE_BYTES)
        {
            return Err(OperationError::invalid_input(format!(
                "ACT model and optimizer may not exceed the {MAX_ACT_TRAINING_RESOURCE_BYTES}-byte remote compute-session budget. Reduce hidden size, feed-forward size, layer count, batch size, cameras, or image dimensions."
            )));
        }
        Ok(())
    }
}

impl TrainingManager {
    pub(crate) async fn admit_rollout(
        &self,
        command: RolloutConfig,
    ) -> Result<RolloutConfig, OperationError> {
        Self::admitted_rollout(&self.snapshot().await?, command)
    }

    pub(super) fn admitted_deletion(
        snapshot: &TrainingSnapshot,
        active_rollout: Option<&str>,
        run_id: &str,
    ) -> Result<String, OperationError> {
        let run_id = run_id.trim().to_owned();
        if run_id.is_empty()
            || run_id.len() > 128
            || !run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(OperationError::invalid_input("Training run ID is invalid."));
        }
        let Some(run) = snapshot.runs().iter().find(|run| run.id() == run_id) else {
            return Err(OperationError::NotFound(format!(
                "training run {run_id:?} is no longer available"
            )));
        };
        if run.phase().active() {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is still active; stop it before deleting it"
            )));
        }
        if active_rollout == Some(run_id.as_str()) {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is in use by Rollout; stop Rollout before deleting it"
            )));
        }
        Ok(run_id)
    }

    fn admitted_rollout(
        snapshot: &TrainingSnapshot,
        command: RolloutConfig,
    ) -> Result<RolloutConfig, OperationError> {
        let command = Self::validated_rollout(command)?;
        let Some(run) = snapshot
            .runs()
            .iter()
            .find(|run| run.id() == command.run_id())
        else {
            return Err(OperationError::invalid_input(
                "the selected training run is no longer available",
            ));
        };
        if run.phase().active() {
            return Err(OperationError::invalid_input(
                "stop or finish training before starting a rollout",
            ));
        }
        if !run.checkpoints().iter().any(|checkpoint| {
            checkpoint.step() == command.checkpoint_step() && checkpoint.inference_ready()
        }) {
            return Err(OperationError::invalid_input(
                "the selected checkpoint is no longer available",
            ));
        }
        Ok(command)
    }

    fn validated_rollout(command: RolloutConfig) -> Result<RolloutConfig, OperationError> {
        let command = RolloutConfig::new(
            command.run_id().trim().to_owned(),
            command.checkpoint_step(),
            command.robot().trim().to_owned(),
        )
        .with_duration(command.duration_seconds());
        if command.run_id().is_empty() || command.robot().is_empty() {
            return Err(OperationError::invalid_input(
                "rollout requires a training run and target robot",
            ));
        }
        if command.checkpoint_step() == 0 {
            return Err(OperationError::invalid_input(
                "checkpoint step must be greater than zero",
            ));
        }
        if RolloutDuration::new(command.duration_seconds()).is_none() {
            return Err(OperationError::invalid_input(format!(
                "rollout duration_seconds must be between {} and {}",
                MIN_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_DURATION_SECONDS,
            )));
        }
        Ok(command)
    }
}

/// Admission rules for a training configuration.
///
/// The configuration already carries every value an operator can set; these
/// rules decide whether that combination may start a run. Keeping them on the
/// configuration means no second copy has to exist to hold them.
pub(super) trait StartTrainingAdmission: Sized {
    fn normalize_identifiers(&mut self);
    fn estimated_act_resource_bytes(&self, runtime_image_bytes: usize) -> Option<usize>;
    fn validated(self) -> Result<Self, OperationError>;
}

impl StartTrainingAdmission for TrainingConfig {
    fn normalize_identifiers(&mut self) {
        let run_name = self.run_name().trim().to_owned();
        self.set_run_name(run_name);
        let dataset_id = self.dataset_id().trim().to_owned();
        self.set_dataset_id(dataset_id);
        let state_column = self.state_column().trim().to_owned();
        self.set_state_column(state_column);
        let action_column = self.action_column().trim().to_owned();
        self.set_action_column(action_column);
        let device = self.device().trim().to_owned();
        self.set_device(device);
    }

    fn validated(mut self) -> Result<Self, OperationError> {
        let command = &mut self;
        command.normalize_identifiers();

        if command.run_name().is_empty()
            || command.run_name().len() > 80
            || !command
                .run_name()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(OperationError::invalid_input(
                "Run name must use 1–80 letters, numbers, '.', '_' or '-' characters.",
            ));
        }
        DatasetId::try_from(command.dataset_id()).map_err(|_| {
            OperationError::InvalidInput(
                "Dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters.".to_owned(),
            )
        })?;
        ReadOnlyQuery::new(command.sql(), ReadOnlyQueryMode::Training)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        for (label, column) in [
            ("State column", command.state_column()),
            ("Action column", command.action_column()),
        ] {
            if UnquotedSqlIdentifier::try_from(column).is_err() {
                return Err(OperationError::invalid_input(format!(
                    "{label} must be a SQL identifier."
                )));
            }
        }
        if command.state_column() == command.action_column() {
            return Err(OperationError::invalid_input(
                "State and action columns must be different.",
            ));
        }
        if command.device().is_empty()
            || command.device().len() > 64
            || !command.device().bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/' | b'+')
            })
        {
            return Err(OperationError::invalid_input(
                "Training device must be a 1–64 character identifier advertised by the runtime.",
            ));
        }
        if !(1..=10_000_000).contains(&command.steps()) {
            return Err(OperationError::invalid_input(
                "Training steps must be between 1 and 10,000,000.",
            ));
        }
        if !(1..=512).contains(&command.batch_size()) {
            return Err(OperationError::invalid_input(
                "Batch size must be between 1 and 512.",
            ));
        }
        if !command.validation_split().is_finite()
            || !(0.0..=0.5).contains(&command.validation_split())
        {
            return Err(OperationError::invalid_input(
                "Validation split must be between 0% and 50%.",
            ));
        }
        if !(1..=256).contains(&command.chunk_size()) {
            return Err(OperationError::invalid_input(
                "Action chunk size must be between 1 and 256.",
            ));
        }
        if command
            .batch_size()
            .checked_mul(command.chunk_size())
            .is_none_or(|rows| rows > 8_192)
        {
            return Err(OperationError::invalid_input(
                "Batch size × action chunk size may not exceed 8,192 manifest rows.",
            ));
        }
        if command.checkpoint_every() == 0 || command.checkpoint_every() > command.steps() {
            return Err(OperationError::invalid_input(
                "Checkpoint cadence must be between 1 and the run step count.",
            ));
        }
        if !command.learning_rate().is_finite() || command.learning_rate() <= 0.0 {
            return Err(OperationError::invalid_input(
                "Learning rate must be a finite positive number.",
            ));
        }
        if command.act().camera_streams().len() > 8 {
            return Err(OperationError::invalid_input(
                "ACT supports at most eight camera streams.",
            ));
        }
        let mut camera_streams = BTreeSet::new();
        for stream in command.act().camera_streams() {
            if stream.is_empty()
                || !stream
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return Err(OperationError::invalid_input(format!(
                    "Invalid ACT camera stream {stream:?}."
                )));
            }
            if !camera_streams.insert(stream) {
                return Err(OperationError::invalid_input(format!(
                    "ACT camera stream {stream:?} is repeated."
                )));
            }
        }
        let native_images = command.act().image_width() == 0 && command.act().image_height() == 0;
        if !native_images
            && (!(32..=1_024).contains(&command.act().image_width())
                || !(32..=1_024).contains(&command.act().image_height()))
        {
            return Err(OperationError::invalid_input(
                "ACT image width and height must both be zero (native) or between 32 and 1,024.",
            ));
        }
        let runtime_image_bytes = if native_images {
            // The adapter repeats the exact check after resolving dataset
            // dimensions. Budget the largest batch it is allowed to admit here.
            MAX_TRAINING_IMAGE_BATCH_BYTES
        } else {
            // An empty list means automatic selection, not zero cameras. Admission
            // must budget the maximum the use case permits; the adapter repeats the
            // check with the exact resolved dataset camera count.
            let camera_count = if command.act().camera_streams().is_empty() {
                8
            } else {
                command.act().camera_streams().len()
            };
            // Training transports one byte per channel, but the runtime expands
            // every image to f32 before model execution. Admission budgets that
            // expanded representation rather than the smaller HTTP request body.
            let image_bytes = command
                .batch_size()
                .checked_mul(camera_count)
                .and_then(|values| values.checked_mul(3))
                .and_then(|values| values.checked_mul(command.act().image_width()))
                .and_then(|values| values.checked_mul(command.act().image_height()))
                .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()));
            if image_bytes.is_none_or(|bytes| bytes > MAX_TRAINING_IMAGE_BATCH_BYTES) {
                return Err(OperationError::invalid_input(format!(
                    "ACT expanded f32 image batch may not exceed {MAX_TRAINING_IMAGE_BATCH_BYTES} bytes. Reduce batch size, camera count, or image dimensions."
                )));
            }
            image_bytes.expect("checked runtime image byte count")
        };
        if !matches!(
            command.act().video_decoder(),
            "auto" | "ffmpeg" | "videotoolbox" | "nvdec"
        ) {
            return Err(OperationError::invalid_input(
                "ACT video decoder must be auto, ffmpeg, videotoolbox, or nvdec.",
            ));
        }
        if !(1..=16).contains(&command.act().attention_heads()) {
            return Err(OperationError::invalid_input(
                "ACT attention heads must be between 1 and 16.",
            ));
        }
        if !(64..=1_024).contains(&command.act().hidden_dim())
            || !command.act().hidden_dim().is_multiple_of(4)
            || !command
                .act()
                .hidden_dim()
                .is_multiple_of(command.act().attention_heads())
        {
            return Err(OperationError::invalid_input(
                "ACT hidden size must be 64–1,024 and divisible by four and the attention-head count.",
            ));
        }
        if !(1..=12).contains(&command.act().encoder_layers())
            || !(1..=12).contains(&command.act().vae_encoder_layers())
            || !(1..=12).contains(&command.act().decoder_layers())
            || !(1..=512).contains(&command.act().latent_dim())
            || command.act().feedforward_dim() < command.act().hidden_dim()
            || command.act().feedforward_dim() > 16_384
        {
            return Err(OperationError::invalid_input(
                "ACT transformer configuration is outside the supported range.",
            ));
        }
        ActTrainingResourceEstimate::for_command(command, runtime_image_bytes).enforce()?;
        if !command.act().kl_weight().is_finite() || command.act().kl_weight() < 0.0 {
            return Err(OperationError::invalid_input(
                "ACT KL weight must be finite and non-negative.",
            ));
        }
        Ok(self)
    }

    /// Conservatively estimate memory owned while one ACT optimizer step is
    /// live, including transformer parameters and expanded image tensors.
    fn estimated_act_resource_bytes(&self, runtime_image_bytes: usize) -> Option<usize> {
        let hidden = self.act().hidden_dim();
        let feedforward = self.act().feedforward_dim();
        let hidden_squared = hidden.checked_mul(hidden)?;
        let hidden_feedforward = hidden.checked_mul(feedforward)?;

        // Encoder/CVAE encoder: one self-attention, one feed-forward pair, two
        // norms. Decoder: self + cross attention, one feed-forward pair, 3 norms.
        let encoder_parameters = hidden_squared
            .checked_mul(4)?
            .checked_add(hidden_feedforward.checked_mul(2)?)?
            .checked_add(hidden.checked_mul(9)?)?
            .checked_add(feedforward)?;
        let decoder_parameters = hidden_squared
            .checked_mul(8)?
            .checked_add(hidden_feedforward.checked_mul(2)?)?
            .checked_add(hidden.checked_mul(15)?)?
            .checked_add(feedforward)?;
        let encoder_count = self
            .act()
            .encoder_layers()
            .checked_add(self.act().vae_encoder_layers())?;
        let transformer_parameters = encoder_parameters
            .checked_mul(encoder_count)?
            .checked_add(decoder_parameters.checked_mul(self.act().decoder_layers())?)?
            .checked_add(hidden.checked_mul(self.act().latent_dim())?)?
            .checked_add(hidden.checked_mul(self.chunk_size())?)?;
        let transformer_training_bytes =
            transformer_parameters.checked_mul(ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER)?;
        let image_runtime_bytes = runtime_image_bytes.checked_mul(ACT_IMAGE_RUNTIME_MULTIPLIER)?;
        transformer_training_bytes
            .checked_add(image_runtime_bytes)?
            .checked_add(ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::training::{TrainingCheckpoint, TrainingRun, TrainingRunPhase};

    fn rollout_command() -> RolloutConfig {
        RolloutConfig::new("run-1", 20_000, "follower").with_duration(30)
    }

    fn training_command() -> TrainingConfig {
        let mut config =
            TrainingConfig::for_dataset(" act-run ", " demo ", "SELECT state, action FROM samples");
        config.set_state_column(" state ");
        config.set_action_column(" action ");
        config.set_device(" CPU ");
        config.set_steps(1_000);
        config.set_batch_size(8);
        config.set_validation_split(0.1);
        config.set_learning_rate(1.0e-5);
        config.set_checkpoint_every(100);
        config.set_chunk_size(16);
        config
            .act_mut()
            .set_camera_streams(vec!["wrist".to_owned()]);
        config
    }

    fn act_config(
        architecture: spiral_robotics_common::training::ActArchitectureConfig,
        attention: spiral_robotics_common::training::ActAttentionConfig,
    ) -> spiral_robotics_common::training::ActTrainingConfig {
        spiral_robotics_common::training::ActTrainingConfig::new(
            spiral_robotics_common::training::ActInputConfig::new(
                vec!["wrist".to_owned()],
                (0, 0),
                "auto",
            ),
            architecture,
            attention,
        )
    }

    fn snapshot(run: Option<TrainingRun>) -> TrainingSnapshot {
        TrainingSnapshot::new(Default::default(), run.into_iter().collect())
    }

    #[test]
    fn training_validation_normalizes_identifiers() {
        let command = training_command().validated().unwrap();
        assert_eq!(command.run_name(), "act-run");
        assert_eq!(command.dataset_id(), "demo");
        assert_eq!(command.state_column(), "state");
        assert_eq!(command.action_column(), "action");
        assert_eq!(command.device(), "CPU");
    }

    #[test]
    fn training_accepts_runtime_advertised_cuda_device_identifiers() {
        let mut command = training_command();
        command.set_device(" cuda:0 ");

        assert_eq!(command.validated().unwrap().device(), "cuda:0");
    }

    #[test]
    fn invalid_training_is_rejected_by_admission() {
        let mut commands = Vec::new();
        let mut mutating = training_command();
        mutating.set_sql("DELETE FROM samples");
        commands.push(mutating);
        let mut duplicate_columns = training_command();
        duplicate_columns.set_action_column("state");
        commands.push(duplicate_columns);
        let mut invalid_vae = training_command();
        *invalid_vae.act_mut() = act_config(
            spiral_robotics_common::training::ActArchitectureConfig::new(
                256,
                8,
                spiral_robotics_common::training::ActLayerCounts::new(4, 0, 4),
            ),
            spiral_robotics_common::training::ActAttentionConfig::new(32, 1_024, 10.0),
        );
        commands.push(invalid_vae);
        let mut non_finite = training_command();
        non_finite.set_learning_rate(f64::NAN);
        commands.push(non_finite);
        let mut invalid_validation_split = training_command();
        invalid_validation_split.set_validation_split(0.75);
        commands.push(invalid_validation_split);

        for command in commands {
            assert!(matches!(
                command.validated(),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn expanded_image_batch_budget_is_checked_at_the_exact_boundary() {
        let mut boundary = training_command();
        boundary.set_batch_size(85);
        boundary.set_chunk_size(1);
        boundary
            .act_mut()
            .set_camera_streams(vec!["wrist".to_owned()]);
        boundary.act_mut().set_image_width(512);
        boundary.act_mut().set_image_height(512);
        // 85 × 1 × 3 × 512 × 512 × 4 = 255 MiB, under the 256 MiB cap.
        boundary.clone().validated().unwrap();

        boundary.set_batch_size(86);
        let error = boundary.validated().unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));

        let mut automatic = training_command();
        automatic.set_batch_size(11);
        automatic.set_chunk_size(1);
        automatic.act_mut().set_camera_streams(Vec::new());
        automatic.act_mut().set_image_width(512);
        automatic.act_mut().set_image_height(512);
        let error = automatic.validated().unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));
    }

    #[test]
    fn aggregate_act_budget_rejects_individually_valid_dimensions() {
        let mut command = training_command();
        command.set_batch_size(1);
        command.set_chunk_size(1);
        *command.act_mut() = act_config(
            spiral_robotics_common::training::ActArchitectureConfig::new(
                1_024,
                512,
                spiral_robotics_common::training::ActLayerCounts::new(12, 12, 12),
            ),
            spiral_robotics_common::training::ActAttentionConfig::new(16, 16_384, 10.0),
        );

        let error = command.validated().unwrap_err();
        assert!(error.message().contains("model and optimizer"));
    }

    #[test]
    fn aggregate_act_budget_is_inclusive_and_overflow_is_fail_closed() {
        assert!(
            ActTrainingResourceEstimate(Some(MAX_ACT_TRAINING_RESOURCE_BYTES))
                .enforce()
                .is_ok()
        );
        assert!(
            ActTrainingResourceEstimate(MAX_ACT_TRAINING_RESOURCE_BYTES.checked_add(1))
                .enforce()
                .is_err()
        );
        assert!(ActTrainingResourceEstimate(None).enforce().is_err());

        let mut overflow = training_command();
        *overflow.act_mut() = act_config(
            spiral_robotics_common::training::ActArchitectureConfig::new(
                usize::MAX,
                512,
                spiral_robotics_common::training::ActLayerCounts::new(usize::MAX, 4, 4),
            ),
            spiral_robotics_common::training::ActAttentionConfig::new(16, usize::MAX, 10.0),
        );
        assert!(
            overflow.estimated_act_resource_bytes(usize::MAX).is_none(),
            "checked resource arithmetic must never wrap to an admitted estimate"
        );
    }

    #[test]
    fn malformed_rollout_is_rejected_before_training_lookup() {
        let snapshot = snapshot(run(false, 20_000, true));
        for command in [
            RolloutConfig::new(String::new(), 20_000, "follower").with_duration(30),
            RolloutConfig::new("run-1", 0, "follower").with_duration(30),
            RolloutConfig::new("run-1", 20_000, "follower")
                .with_duration(MAX_ROLLOUT_DURATION_SECONDS + 1),
        ] {
            assert!(matches!(
                TrainingManager::admitted_rollout(&snapshot, command),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }

    fn run(active: bool, step: u64, inference_ready: bool) -> Option<TrainingRun> {
        let phase = if active {
            TrainingRunPhase::Running
        } else {
            TrainingRunPhase::Completed
        };
        Some(
            TrainingRun::starting("run-1", TrainingConfig::default())
                .with_phase(phase, step)
                .with_checkpoints(vec![
                    TrainingCheckpoint::new(step, 1).with_artifact(1, inference_ready),
                ])
                .with_started_ms(1),
        )
    }

    #[test]
    fn rollout_rejects_missing_active_or_unready_training() {
        for (run, expected) in [
            (None, "no longer available"),
            (run(true, 20_000, true), "stop or finish"),
            (
                run(false, 10_000, true),
                "checkpoint is no longer available",
            ),
            (
                run(false, 20_000, false),
                "checkpoint is no longer available",
            ),
        ] {
            let error =
                TrainingManager::admitted_rollout(&snapshot(run), rollout_command()).unwrap_err();
            assert!(error.message().contains(expected));
        }
    }

    #[test]
    fn ready_rollout_returns_the_exact_admitted_command() {
        let command = rollout_command();

        assert_eq!(
            TrainingManager::admitted_rollout(&snapshot(run(false, 20_000, true)), command.clone(),),
            Ok(command)
        );
    }

    #[test]
    fn deleting_a_run_rejects_training_and_rollout_activity() {
        let error =
            TrainingManager::admitted_deletion(&snapshot(run(true, 20_000, true)), None, "run-1")
                .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));

        let error = TrainingManager::admitted_deletion(
            &snapshot(run(false, 20_000, true)),
            Some("run-1"),
            "run-1",
        )
        .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));
    }

    #[test]
    fn deleting_a_missing_run_is_not_found_and_an_idle_run_is_admitted() {
        let error = TrainingManager::admitted_deletion(&snapshot(None), None, "run-1").unwrap_err();
        assert!(matches!(error, OperationError::NotFound(_)));

        let run_id =
            TrainingManager::admitted_deletion(&snapshot(run(false, 20_000, true)), None, "run-1")
                .unwrap();
        assert_eq!(run_id, "run-1");
    }
}
