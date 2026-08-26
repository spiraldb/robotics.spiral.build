//! Framework-neutral training and policy-rollout capability actions.

use std::collections::BTreeSet;

use spiral_robotics_api::identity::DatasetId;
use spiral_robotics_api::rollout::{
    MAX_ROLLOUT_DURATION_SECONDS, MIN_ROLLOUT_DURATION_SECONDS, RolloutDuration,
};
use spiral_robotics_api::sql::{ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};

use crate::OperationError;
use crate::rollout::service::RolloutRequest;
use spiral_robotics_api::training::{StartTrainingRequest, TrainingRun as ContractTrainingRun};

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
    fn for_command(command: &StartTrainingRequest, runtime_image_bytes: usize) -> Self {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrainingCheckpointStatus {
    step: u64,
    inference_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunStatus {
    active: bool,
    checkpoints: Vec<TrainingCheckpointStatus>,
}

/// Other live work that can still depend on a completed run's artifacts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingRunActivity {
    rollout: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeleteTrainingRunCommand {
    run_id: String,
}

impl TrainingCheckpointStatus {
    pub(crate) const fn new(step: u64, inference_ready: bool) -> Self {
        Self {
            step,
            inference_ready,
        }
    }
}

impl TrainingRunStatus {
    pub(crate) fn new(active: bool, checkpoints: Vec<TrainingCheckpointStatus>) -> Self {
        Self {
            active,
            checkpoints,
        }
    }
}

impl TrainingRunActivity {
    pub(crate) fn new(rollout: Option<String>) -> Self {
        Self { rollout }
    }
}

impl DeleteTrainingRunCommand {
    pub(crate) fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
        }
    }

    fn validated_run_id(self) -> Result<String, OperationError> {
        let run_id = self.run_id.trim().to_owned();
        if run_id.is_empty()
            || run_id.len() > 128
            || !run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(OperationError::InvalidInput(
                "Training run ID is invalid.".to_owned(),
            ));
        }
        Ok(run_id)
    }
}

pub(crate) trait TrainingPort: Send + Sync {
    fn training_run_status(&self, run_id: &str) -> Option<TrainingRunStatus>;

    async fn start_training(
        &self,
        command: StartTrainingRequest,
    ) -> Result<ContractTrainingRun, OperationError>;

    fn stop_training(&self, run_id: &str) -> Result<(), OperationError>;

    async fn delete_training(&self, run_id: String) -> Result<(), OperationError>;
}

pub(crate) struct TrainingActions<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: TrainingPort + ?Sized> TrainingActions<'a, P> {
    pub(crate) fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub(crate) async fn start(
        &self,
        command: StartTrainingRequest,
    ) -> Result<ContractTrainingRun, OperationError> {
        self.port.start_training(command.validated()?).await
    }

    pub(crate) fn stop(&self, run_id: &str) -> Result<(), OperationError> {
        self.port.stop_training(run_id)
    }

    pub(crate) async fn delete(
        &self,
        activity: TrainingRunActivity,
        command: DeleteTrainingRunCommand,
    ) -> Result<(), OperationError> {
        let run_id = command.validated_run_id()?;
        let Some(run) = self.port.training_run_status(&run_id) else {
            return Err(OperationError::NotFound(format!(
                "training run {run_id:?} is no longer available"
            )));
        };
        if run.active {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is still active; stop it before deleting it"
            )));
        }
        if activity.rollout.as_deref() == Some(run_id.as_str()) {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is in use by Rollout; stop Rollout before deleting it"
            )));
        }
        self.port.delete_training(run_id).await
    }

    pub(crate) fn admit_rollout(
        &self,
        command: RolloutRequest,
    ) -> Result<RolloutRequest, OperationError> {
        let command = command.validated()?;
        let Some(run) = self.port.training_run_status(command.run_id()) else {
            return Err(OperationError::invalid_input(
                "the selected training run is no longer available",
            ));
        };
        if run.active {
            return Err(OperationError::invalid_input(
                "stop or finish training before starting a rollout",
            ));
        }
        if !run.checkpoints.iter().any(|checkpoint| {
            checkpoint.step == command.checkpoint_step() && checkpoint.inference_ready
        }) {
            return Err(OperationError::invalid_input(
                "the selected checkpoint is no longer available",
            ));
        }
        Ok(command)
    }
}

/// Admission rules for a training request.
///
/// The wire type already carries every value an operator can set; these rules
/// decide whether that combination may start a run. Keeping them on the request
/// means no second copy of the request has to exist to hold them.
trait StartTrainingAdmission: Sized {
    fn normalize_identifiers(&mut self);
    fn estimated_act_resource_bytes(&self, runtime_image_bytes: usize) -> Option<usize>;
    fn validated(self) -> Result<Self, OperationError>;
}

impl StartTrainingAdmission for StartTrainingRequest {
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
            // The adapter repeats the exact check after resolving native dataset
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

impl RolloutRequest {
    fn validated(self) -> Result<Self, OperationError> {
        let command = Self::new(
            self.run_id().trim().to_owned(),
            self.checkpoint_step(),
            self.robot().trim().to_owned(),
        )
        .with_duration(self.duration_seconds());
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeTraining {
        run: Option<TrainingRunStatus>,
        starts: Mutex<Vec<StartTrainingRequest>>,
        stops: Mutex<Vec<String>>,
        deletes: Mutex<Vec<String>>,
    }

    impl TrainingPort for FakeTraining {
        fn training_run_status(&self, _: &str) -> Option<TrainingRunStatus> {
            self.run.clone()
        }

        async fn start_training(
            &self,
            command: StartTrainingRequest,
        ) -> Result<ContractTrainingRun, OperationError> {
            self.starts.lock().unwrap().push(command.clone());
            Ok(training_run(command))
        }

        fn stop_training(&self, run_id: &str) -> Result<(), OperationError> {
            self.stops.lock().unwrap().push(run_id.to_owned());
            Ok(())
        }

        async fn delete_training(&self, run_id: String) -> Result<(), OperationError> {
            self.deletes.lock().unwrap().push(run_id);
            Ok(())
        }
    }

    fn training(run: Option<TrainingRunStatus>) -> FakeTraining {
        FakeTraining {
            run,
            starts: Mutex::new(Vec::new()),
            stops: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
        }
    }

    fn rollout_command() -> RolloutRequest {
        RolloutRequest::new("run-1".to_owned(), 20_000, "follower".to_owned()).with_duration(30)
    }

    fn training_command() -> StartTrainingRequest {
        let mut request = StartTrainingRequest::for_dataset(
            " act-run ",
            " demo ",
            "SELECT state, action FROM samples",
        );
        request.set_state_column(" state ");
        request.set_action_column(" action ");
        request.set_device(" CPU ");
        request.set_steps(1_000);
        request.set_batch_size(8);
        request.set_validation_split(0.1);
        request.set_learning_rate(1.0e-5);
        request.set_checkpoint_every(100);
        request.set_chunk_size(16);
        request
            .act_mut()
            .set_camera_streams(vec!["wrist".to_owned()]);
        request
    }

    fn act_config(
        architecture: spiral_robotics_api::training::ActArchitectureConfig,
        attention: spiral_robotics_api::training::ActAttentionConfig,
    ) -> spiral_robotics_api::training::ActTrainingConfig {
        spiral_robotics_api::training::ActTrainingConfig::new(
            spiral_robotics_api::training::ActInputConfig::new(
                vec!["wrist".to_owned()],
                (0, 0),
                "auto",
            ),
            architecture,
            attention,
        )
    }

    fn training_run(config: StartTrainingRequest) -> ContractTrainingRun {
        ContractTrainingRun::new(
            spiral_robotics_api::training::TrainingRunIdentity::new(
                "run",
                config,
                spiral_robotics_api::training::TrainingSource::default(),
            ),
            spiral_robotics_api::training::TrainingRunLifecycle::new(
                spiral_robotics_api::training::TrainingRunPhase::Starting,
                0,
                1,
            ),
            spiral_robotics_api::training::TrainingRunHistory::default(),
        )
    }

    #[tokio::test]
    async fn training_validation_is_pure_and_normalizes_identifiers_before_delegating() {
        let training = training(None);
        TrainingActions::new(&training)
            .start(training_command())
            .await
            .unwrap();
        let starts = training.starts.lock().unwrap();
        assert_eq!(starts[0].run_name(), "act-run");
        assert_eq!(starts[0].dataset_id(), "demo");
        assert_eq!(starts[0].state_column(), "state");
        assert_eq!(starts[0].action_column(), "action");
        assert_eq!(starts[0].device(), "CPU");
    }

    #[tokio::test]
    async fn training_accepts_runtime_advertised_cuda_device_identifiers() {
        let mut command = training_command();
        command.set_device(" cuda:0 ");

        let training = training(None);
        TrainingActions::new(&training)
            .start(command)
            .await
            .unwrap();

        let starts = training.starts.lock().unwrap();
        assert_eq!(starts[0].device(), "cuda:0");
    }

    #[tokio::test]
    async fn invalid_training_never_reaches_the_runtime_adapter() {
        let mut commands = Vec::new();
        let mut mutating = training_command();
        mutating.set_sql("DELETE FROM samples");
        commands.push(mutating);
        let mut duplicate_columns = training_command();
        duplicate_columns.set_action_column("state");
        commands.push(duplicate_columns);
        let mut invalid_vae = training_command();
        *invalid_vae.act_mut() = act_config(
            spiral_robotics_api::training::ActArchitectureConfig::new(
                256,
                8,
                spiral_robotics_api::training::ActLayerCounts::new(4, 0, 4),
            ),
            spiral_robotics_api::training::ActAttentionConfig::new(32, 1_024, 10.0),
        );
        commands.push(invalid_vae);
        let mut non_finite = training_command();
        non_finite.set_learning_rate(f64::NAN);
        commands.push(non_finite);
        let mut invalid_validation_split = training_command();
        invalid_validation_split.set_validation_split(0.75);
        commands.push(invalid_validation_split);

        let training = training(None);
        for command in commands {
            assert!(matches!(
                TrainingActions::new(&training).start(command).await,
                Err(OperationError::InvalidInput(_))
            ));
        }
        assert!(training.starts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn expanded_image_batch_budget_is_checked_at_the_exact_boundary() {
        let mut boundary = training_command();
        boundary.set_batch_size(85);
        boundary.set_chunk_size(1);
        boundary
            .act_mut()
            .set_camera_streams(vec!["wrist".to_owned()]);
        boundary.act_mut().set_image_width(512);
        boundary.act_mut().set_image_height(512);
        // 85 × 1 × 3 × 512 × 512 × 4 = 255 MiB, under the 256 MiB cap.
        let at_boundary = training(None);
        TrainingActions::new(&at_boundary)
            .start(boundary.clone())
            .await
            .unwrap();

        boundary.set_batch_size(86);
        let over_budget = training(None);
        let error = TrainingActions::new(&over_budget)
            .start(boundary)
            .await
            .unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));
        assert!(over_budget.starts.lock().unwrap().is_empty());

        let mut automatic = training_command();
        automatic.set_batch_size(11);
        automatic.set_chunk_size(1);
        automatic.act_mut().set_camera_streams(Vec::new());
        automatic.act_mut().set_image_width(512);
        automatic.act_mut().set_image_height(512);
        let training = training(None);
        let error = TrainingActions::new(&training)
            .start(automatic)
            .await
            .unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));
    }

    #[tokio::test]
    async fn aggregate_act_budget_rejects_individually_valid_dimensions_before_runtime() {
        let mut command = training_command();
        command.set_batch_size(1);
        command.set_chunk_size(1);
        *command.act_mut() = act_config(
            spiral_robotics_api::training::ActArchitectureConfig::new(
                1_024,
                512,
                spiral_robotics_api::training::ActLayerCounts::new(12, 12, 12),
            ),
            spiral_robotics_api::training::ActAttentionConfig::new(16, 16_384, 10.0),
        );

        let training = training(None);
        let error = TrainingActions::new(&training)
            .start(command)
            .await
            .unwrap_err();
        assert!(error.message().contains("model and optimizer"));
        assert!(training.starts.lock().unwrap().is_empty());
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
            spiral_robotics_api::training::ActArchitectureConfig::new(
                usize::MAX,
                512,
                spiral_robotics_api::training::ActLayerCounts::new(usize::MAX, 4, 4),
            ),
            spiral_robotics_api::training::ActAttentionConfig::new(16, usize::MAX, 10.0),
        );
        assert!(
            overflow.estimated_act_resource_bytes(usize::MAX).is_none(),
            "checked resource arithmetic must never wrap to an admitted estimate"
        );
    }

    #[tokio::test]
    async fn malformed_rollout_is_rejected_before_training_or_runtime_lookup() {
        let training = training(run(false, 20_000, true));
        for command in [
            RolloutRequest::new(String::new(), 20_000, "follower".to_owned()).with_duration(30),
            RolloutRequest::new("run-1".to_owned(), 0, "follower".to_owned()).with_duration(30),
            RolloutRequest::new("run-1".to_owned(), 20_000, "follower".to_owned())
                .with_duration(MAX_ROLLOUT_DURATION_SECONDS + 1),
        ] {
            assert!(matches!(
                TrainingActions::new(&training).admit_rollout(command),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }

    fn run(active: bool, step: u64, inference_ready: bool) -> Option<TrainingRunStatus> {
        Some(TrainingRunStatus::new(
            active,
            vec![TrainingCheckpointStatus::new(step, inference_ready)],
        ))
    }

    #[tokio::test]
    async fn rollout_rejects_missing_active_or_unready_training_without_delegating() {
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
            let training = training(run);
            let error = TrainingActions::new(&training)
                .admit_rollout(rollout_command())
                .unwrap_err();
            assert!(error.message().contains(expected));
        }
    }

    #[tokio::test]
    async fn ready_rollout_returns_the_exact_admitted_semantic_command() {
        let training = training(run(false, 20_000, true));
        let command = rollout_command();

        assert_eq!(
            TrainingActions::new(&training).admit_rollout(command.clone()),
            Ok(command)
        );
    }

    #[tokio::test]
    async fn deleting_a_run_rejects_training_and_rollout_activity() {
        let command = || DeleteTrainingRunCommand::new("run-1");
        let active_training = training(run(true, 20_000, true));
        let error = TrainingActions::new(&active_training)
            .delete(TrainingRunActivity::default(), command())
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(active_training.deletes.lock().unwrap().is_empty());

        let active_rollout = training(run(false, 20_000, true));
        let error = TrainingActions::new(&active_rollout)
            .delete(
                TrainingRunActivity::new(Some("run-1".to_owned())),
                command(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(active_rollout.deletes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_a_missing_run_is_not_found_and_an_idle_run_delegates_once() {
        let missing = training(None);
        let error = TrainingActions::new(&missing)
            .delete(
                TrainingRunActivity::default(),
                DeleteTrainingRunCommand::new("run-1"),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::NotFound(_)));

        let idle = training(run(false, 20_000, true));
        TrainingActions::new(&idle)
            .delete(
                TrainingRunActivity::default(),
                DeleteTrainingRunCommand::new("run-1"),
            )
            .await
            .unwrap();
        assert_eq!(idle.deletes.lock().unwrap().as_slice(), ["run-1"]);
    }
}
