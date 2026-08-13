//! Framework-neutral training and policy-rollout application actions.

use std::collections::BTreeSet;

use spiral_robotics_core::identity::DatasetId;
use spiral_robotics_core::operation::rollout_duration_is_valid;
use spiral_robotics_core::query::{ReadOnlyMode, ensure_read_only, is_identifier};

use crate::backend::application::error::ApplicationError;
use crate::backend::application::rollout::RolloutRequest;

/// One decoded image batch must leave enough memory for model activations,
/// optimizer state, and the hardware-control process itself.
pub(crate) const MAX_TRAINING_IMAGE_BATCH_BYTES: usize = 256 * 1024 * 1024;

/// Aggregate in-process ACT training budget. Studio trains beside the hardware
/// control loop, so independently valid dimensions must not combine into a
/// multi-gigabyte optimizer allocation that can terminate the whole backend.
const MAX_ACT_TRAINING_RESOURCE_BYTES: usize = 2 * 1024 * 1024 * 1024;
const ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES: usize = 256 * 1024 * 1024;
const ACT_IMAGE_RUNTIME_MULTIPLIER: usize = 4;
/// Parameter + gradient + two AdamW moments, plus one parameter-sized runtime
/// workspace reserve. All ACT trainable tensors use f32 today.
const ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER: usize = 5 * std::mem::size_of::<f32>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrainingPolicy {
    Act,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActTrainingSettings {
    pub camera_streams: Vec<String>,
    pub image_width: usize,
    pub image_height: usize,
    pub video_decoder: String,
    pub pretrained_backbone: bool,
    pub hidden_dim: usize,
    pub latent_dim: usize,
    pub encoder_layers: usize,
    pub vae_encoder_layers: usize,
    pub decoder_layers: usize,
    pub attention_heads: usize,
    pub feedforward_dim: usize,
    pub kl_weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StartTrainingCommand {
    pub run_name: String,
    pub dataset_id: String,
    pub policy: TrainingPolicy,
    pub query: String,
    pub state_column: String,
    pub action_column: String,
    pub device: String,
    pub steps: u64,
    pub batch_size: usize,
    pub validation_split: f64,
    pub learning_rate: f64,
    pub checkpoint_every: u64,
    pub chunk_size: usize,
    pub seed: u64,
    pub act: ActTrainingSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrainingCheckpointStatus {
    pub step: u64,
    pub inference_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunStatus {
    pub active: bool,
    pub checkpoints: Vec<TrainingCheckpointStatus>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TrainingRunPhase {
    #[default]
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetric {
    pub step: u64,
    pub train_loss: f64,
    pub validation_loss: Option<f64>,
    pub learning_rate: f64,
    pub transfer_bytes_per_second: f64,
    pub examples_per_second: f64,
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub elapsed_seconds: f64,
    pub event: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingCheckpoint {
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub byte_size: u64,
    pub inference_ready: bool,
    pub recommended_action_steps: usize,
    pub sample_hz: f64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingSource {
    pub project: String,
    pub dataset: String,
    pub dataset_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingReadiness {
    pub ready: bool,
    pub spiral_ready: bool,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingRun {
    pub id: String,
    pub config: StartTrainingCommand,
    pub source: TrainingSource,
    pub phase: TrainingRunPhase,
    pub current_step: u64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    pub metrics: Vec<TrainingMetric>,
    pub checkpoints: Vec<TrainingCheckpoint>,
    pub logs: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingSnapshot {
    pub readiness: TrainingReadiness,
    pub runs: Vec<TrainingRun>,
}

pub(crate) trait TrainingPort: Send + Sync {
    fn training_snapshot(&self) -> TrainingSnapshot;

    fn training_run_status(&self, run_id: &str) -> Option<TrainingRunStatus>;

    async fn start_training(
        &self,
        command: StartTrainingCommand,
    ) -> Result<TrainingRun, ApplicationError>;

    fn stop_training(&self, run_id: &str) -> Result<(), ApplicationError>;
}

pub(crate) fn list_training<P: TrainingPort + ?Sized>(port: &P) -> TrainingSnapshot {
    port.training_snapshot()
}

pub(crate) async fn start_training<P: TrainingPort + ?Sized>(
    port: &P,
    command: StartTrainingCommand,
) -> Result<TrainingRun, ApplicationError> {
    let command = validate_training_command(command)?;
    port.start_training(command).await
}

pub(crate) fn stop_training<P: TrainingPort + ?Sized>(
    port: &P,
    run_id: &str,
) -> Result<(), ApplicationError> {
    port.stop_training(run_id)
}

pub(crate) fn admit_rollout<T>(
    training: &T,
    command: RolloutRequest,
) -> Result<RolloutRequest, ApplicationError>
where
    T: TrainingPort + ?Sized,
{
    let command = validate_rollout_command(command)?;
    let Some(run) = training.training_run_status(&command.run_id) else {
        return Err(ApplicationError::InvalidInput(
            "the selected training run is no longer available".to_owned(),
        ));
    };
    if run.active {
        return Err(ApplicationError::InvalidInput(
            "stop or finish training before starting a rollout".to_owned(),
        ));
    }
    if !run
        .checkpoints
        .iter()
        .any(|checkpoint| checkpoint.step == command.checkpoint_step && checkpoint.inference_ready)
    {
        return Err(ApplicationError::InvalidInput(
            "the selected checkpoint is no longer available".to_owned(),
        ));
    }
    Ok(command)
}

fn validate_training_command(
    mut command: StartTrainingCommand,
) -> Result<StartTrainingCommand, ApplicationError> {
    command.run_name = command.run_name.trim().to_owned();
    command.dataset_id = command.dataset_id.trim().to_owned();
    command.state_column = command.state_column.trim().to_owned();
    command.action_column = command.action_column.trim().to_owned();
    command.device = command.device.trim().to_ascii_lowercase();

    if command.run_name.is_empty()
        || command.run_name.len() > 80
        || !command
            .run_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return invalid("Run name must use 1–80 letters, numbers, '.', '_' or '-' characters.");
    }
    DatasetId::parse(&command.dataset_id).map_err(|_| {
        ApplicationError::InvalidInput(
            "Dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters.".to_owned(),
        )
    })?;
    ensure_read_only(&command.query, ReadOnlyMode::Training)
        .map_err(|error| ApplicationError::InvalidInput(error.to_string()))?;
    for (label, column) in [
        ("State column", command.state_column.as_str()),
        ("Action column", command.action_column.as_str()),
    ] {
        if !is_identifier(column) {
            return invalid(format!("{label} must be a SQL identifier."));
        }
    }
    if command.state_column == command.action_column {
        return invalid("State and action columns must be different.");
    }
    if !matches!(command.device.as_str(), "auto" | "cpu" | "mps" | "metal") {
        return invalid("Training device must be auto, CPU, or Apple MPS/Metal.");
    }
    if !(1..=10_000_000).contains(&command.steps) {
        return invalid("Training steps must be between 1 and 10,000,000.");
    }
    if !(1..=512).contains(&command.batch_size) {
        return invalid("Batch size must be between 1 and 512.");
    }
    if !command.validation_split.is_finite() || !(0.0..=0.5).contains(&command.validation_split) {
        return invalid("Validation split must be between 0% and 50%.");
    }
    if !(1..=256).contains(&command.chunk_size) {
        return invalid("Action chunk size must be between 1 and 256.");
    }
    if command
        .batch_size
        .checked_mul(command.chunk_size)
        .is_none_or(|rows| rows > 8_192)
    {
        return invalid("Batch size × action chunk size may not exceed 8,192 manifest rows.");
    }
    if command.checkpoint_every == 0 || command.checkpoint_every > command.steps {
        return invalid("Checkpoint cadence must be between 1 and the run step count.");
    }
    if !command.learning_rate.is_finite() || command.learning_rate <= 0.0 {
        return invalid("Learning rate must be a finite positive number.");
    }
    if command.act.camera_streams.len() > 8 {
        return invalid("ACT supports at most eight camera streams.");
    }
    let mut camera_streams = BTreeSet::new();
    for stream in &command.act.camera_streams {
        if stream.is_empty()
            || !stream
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return invalid(format!("Invalid ACT camera stream {stream:?}."));
        }
        if !camera_streams.insert(stream) {
            return invalid(format!("ACT camera stream {stream:?} is repeated."));
        }
    }
    let native_images = command.act.image_width == 0 && command.act.image_height == 0;
    if !native_images
        && (!(32..=1_024).contains(&command.act.image_width)
            || !(32..=1_024).contains(&command.act.image_height))
    {
        return invalid(
            "ACT image width and height must both be zero (native) or between 32 and 1,024.",
        );
    }
    let decoded_image_bytes = if native_images {
        // The adapter repeats the exact check after resolving native dataset
        // dimensions. Budget the largest batch it is allowed to admit here.
        MAX_TRAINING_IMAGE_BATCH_BYTES
    } else {
        // An empty list means automatic selection, not zero cameras. Admission
        // must budget the maximum the use case permits; the adapter repeats the
        // check with the exact resolved dataset camera count.
        let camera_count = if command.act.camera_streams.is_empty() {
            8
        } else {
            command.act.camera_streams.len()
        };
        let image_bytes = command
            .batch_size
            .checked_mul(camera_count)
            .and_then(|values| values.checked_mul(3))
            .and_then(|values| values.checked_mul(command.act.image_width))
            .and_then(|values| values.checked_mul(command.act.image_height))
            .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()));
        if image_bytes.is_none_or(|bytes| bytes > MAX_TRAINING_IMAGE_BATCH_BYTES) {
            return invalid(format!(
                "ACT decoded image batch may not exceed {MAX_TRAINING_IMAGE_BATCH_BYTES} bytes. Reduce batch size, camera count, or image dimensions."
            ));
        }
        image_bytes.expect("checked decoded image byte count")
    };
    if !matches!(
        command.act.video_decoder.as_str(),
        "auto" | "ffmpeg" | "videotoolbox" | "nvdec"
    ) {
        return invalid("ACT video decoder must be auto, ffmpeg, videotoolbox, or nvdec.");
    }
    if !(1..=16).contains(&command.act.attention_heads) {
        return invalid("ACT attention heads must be between 1 and 16.");
    }
    if !(64..=1_024).contains(&command.act.hidden_dim)
        || !command.act.hidden_dim.is_multiple_of(4)
        || !command
            .act
            .hidden_dim
            .is_multiple_of(command.act.attention_heads)
    {
        return invalid(
            "ACT hidden size must be 64–1,024 and divisible by four and the attention-head count.",
        );
    }
    if !(1..=12).contains(&command.act.encoder_layers)
        || !(1..=12).contains(&command.act.vae_encoder_layers)
        || !(1..=12).contains(&command.act.decoder_layers)
        || !(1..=512).contains(&command.act.latent_dim)
        || command.act.feedforward_dim < command.act.hidden_dim
        || command.act.feedforward_dim > 16_384
    {
        return invalid("ACT transformer configuration is outside the supported range.");
    }
    enforce_act_training_resource_budget(estimate_act_training_resource_bytes(
        &command,
        decoded_image_bytes,
    ))?;
    if !command.act.kl_weight.is_finite() || command.act.kl_weight < 0.0 {
        return invalid("ACT KL weight must be finite and non-negative.");
    }
    Ok(command)
}

/// Conservative, checked estimate for memory owned while one ACT optimizer
/// step is live. The exact state/action projections are dataset-dependent, so
/// a fixed non-transformer reserve covers those, ResNet-18, CVAE projections,
/// and backend bookkeeping. Known transformer parameters are counted from the
/// command, including attention, feed-forward, bias, and norm terms.
fn estimate_act_training_resource_bytes(
    command: &StartTrainingCommand,
    decoded_image_bytes: usize,
) -> Option<usize> {
    let hidden = command.act.hidden_dim;
    let feedforward = command.act.feedforward_dim;
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
    let encoder_count = command
        .act
        .encoder_layers
        .checked_add(command.act.vae_encoder_layers)?;
    let transformer_parameters = encoder_parameters
        .checked_mul(encoder_count)?
        .checked_add(decoder_parameters.checked_mul(command.act.decoder_layers)?)?
        .checked_add(hidden.checked_mul(command.act.latent_dim)?)?
        .checked_add(hidden.checked_mul(command.chunk_size)?)?;
    let transformer_training_bytes =
        transformer_parameters.checked_mul(ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER)?;
    let image_runtime_bytes = decoded_image_bytes.checked_mul(ACT_IMAGE_RUNTIME_MULTIPLIER)?;
    transformer_training_bytes
        .checked_add(image_runtime_bytes)?
        .checked_add(ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES)
}

fn enforce_act_training_resource_budget(estimate: Option<usize>) -> Result<(), ApplicationError> {
    if estimate.is_none_or(|bytes| bytes > MAX_ACT_TRAINING_RESOURCE_BYTES) {
        return invalid(format!(
            "ACT model and optimizer may not exceed the {MAX_ACT_TRAINING_RESOURCE_BYTES}-byte in-process training budget. Reduce hidden size, feed-forward size, layer count, batch size, cameras, or image dimensions."
        ));
    }
    Ok(())
}

fn validate_rollout_command(
    mut command: RolloutRequest,
) -> Result<RolloutRequest, ApplicationError> {
    command.run_id = command.run_id.trim().to_owned();
    command.robot = command.robot.trim().to_owned();
    if command.run_id.is_empty() || command.robot.is_empty() {
        return invalid("rollout requires a training run and target robot");
    }
    if command.checkpoint_step == 0 {
        return invalid("checkpoint step must be greater than zero");
    }
    if !rollout_duration_is_valid(command.duration_seconds) {
        return invalid(format!(
            "rollout duration_seconds must be between {} and {}",
            spiral_robotics_core::operation::MIN_ROLLOUT_DURATION_SECONDS,
            spiral_robotics_core::operation::MAX_ROLLOUT_DURATION_SECONDS,
        ));
    }
    Ok(command)
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ApplicationError> {
    Err(ApplicationError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeTraining {
        run: Option<TrainingRunStatus>,
        starts: Mutex<Vec<StartTrainingCommand>>,
        stops: Mutex<Vec<String>>,
    }

    impl TrainingPort for FakeTraining {
        fn training_snapshot(&self) -> TrainingSnapshot {
            TrainingSnapshot::default()
        }

        fn training_run_status(&self, _: &str) -> Option<TrainingRunStatus> {
            self.run.clone()
        }

        async fn start_training(
            &self,
            command: StartTrainingCommand,
        ) -> Result<TrainingRun, ApplicationError> {
            self.starts.lock().unwrap().push(command.clone());
            Ok(training_run(command))
        }

        fn stop_training(&self, run_id: &str) -> Result<(), ApplicationError> {
            self.stops.lock().unwrap().push(run_id.to_owned());
            Ok(())
        }
    }

    fn training(run: Option<TrainingRunStatus>) -> FakeTraining {
        FakeTraining {
            run,
            starts: Mutex::new(Vec::new()),
            stops: Mutex::new(Vec::new()),
        }
    }

    fn rollout_command() -> RolloutRequest {
        RolloutRequest {
            run_id: "run-1".to_owned(),
            checkpoint_step: 20_000,
            robot: "follower".to_owned(),
            duration_seconds: 30,
        }
    }

    fn training_command() -> StartTrainingCommand {
        StartTrainingCommand {
            run_name: " act-run ".to_owned(),
            dataset_id: " demo ".to_owned(),
            policy: TrainingPolicy::Act,
            query: "SELECT state, action FROM samples".to_owned(),
            state_column: " state ".to_owned(),
            action_column: " action ".to_owned(),
            device: " CPU ".to_owned(),
            steps: 1_000,
            batch_size: 8,
            validation_split: 0.1,
            learning_rate: 1.0e-5,
            checkpoint_every: 100,
            chunk_size: 16,
            seed: 7,
            act: ActTrainingSettings {
                camera_streams: vec!["wrist".to_owned()],
                image_width: 0,
                image_height: 0,
                video_decoder: "auto".to_owned(),
                pretrained_backbone: true,
                hidden_dim: 256,
                latent_dim: 32,
                encoder_layers: 4,
                vae_encoder_layers: 4,
                decoder_layers: 4,
                attention_heads: 8,
                feedforward_dim: 1_024,
                kl_weight: 10.0,
            },
        }
    }

    fn training_run(config: StartTrainingCommand) -> TrainingRun {
        TrainingRun {
            id: "run".to_owned(),
            config,
            source: TrainingSource::default(),
            phase: TrainingRunPhase::Starting,
            current_step: 0,
            started_ms: 1,
            finished_ms: None,
            message: None,
            metrics: Vec::new(),
            checkpoints: Vec::new(),
            logs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn training_validation_is_pure_and_normalizes_identifiers_before_delegating() {
        let training = training(None);
        start_training(&training, training_command()).await.unwrap();
        let starts = training.starts.lock().unwrap();
        assert_eq!(starts[0].run_name, "act-run");
        assert_eq!(starts[0].dataset_id, "demo");
        assert_eq!(starts[0].state_column, "state");
        assert_eq!(starts[0].action_column, "action");
        assert_eq!(starts[0].device, "cpu");
    }

    #[tokio::test]
    async fn invalid_training_never_reaches_the_runtime_adapter() {
        let mut commands = Vec::new();
        let mut mutating = training_command();
        mutating.query = "DELETE FROM samples".to_owned();
        commands.push(mutating);
        let mut duplicate_columns = training_command();
        duplicate_columns.action_column = "state".to_owned();
        commands.push(duplicate_columns);
        let mut invalid_vae = training_command();
        invalid_vae.act.vae_encoder_layers = 0;
        commands.push(invalid_vae);
        let mut non_finite = training_command();
        non_finite.learning_rate = f64::NAN;
        commands.push(non_finite);
        let mut invalid_validation_split = training_command();
        invalid_validation_split.validation_split = 0.75;
        commands.push(invalid_validation_split);

        let training = training(None);
        for command in commands {
            assert!(matches!(
                start_training(&training, command).await,
                Err(ApplicationError::InvalidInput(_))
            ));
        }
        assert!(training.starts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn decoded_image_batch_budget_is_checked_at_the_exact_boundary() {
        let mut boundary = training_command();
        boundary.batch_size = 85;
        boundary.chunk_size = 1;
        boundary.act.camera_streams = vec!["wrist".to_owned()];
        boundary.act.image_width = 512;
        boundary.act.image_height = 512;
        // 85 × 1 × 3 × 512 × 512 × 4 = 255 MiB, under the 256 MiB cap.
        let at_boundary = training(None);
        start_training(&at_boundary, boundary.clone())
            .await
            .unwrap();

        boundary.batch_size = 86;
        let over_budget = training(None);
        let error = start_training(&over_budget, boundary).await.unwrap_err();
        assert!(error.message().contains("decoded image batch"));
        assert!(over_budget.starts.lock().unwrap().is_empty());

        let mut automatic = training_command();
        automatic.batch_size = 11;
        automatic.chunk_size = 1;
        automatic.act.camera_streams.clear();
        automatic.act.image_width = 512;
        automatic.act.image_height = 512;
        let training = training(None);
        let error = start_training(&training, automatic).await.unwrap_err();
        assert!(error.message().contains("decoded image batch"));
    }

    #[tokio::test]
    async fn aggregate_act_budget_rejects_individually_valid_dimensions_before_runtime() {
        let mut command = training_command();
        command.batch_size = 1;
        command.chunk_size = 1;
        command.act.hidden_dim = 1_024;
        command.act.latent_dim = 512;
        command.act.encoder_layers = 12;
        command.act.vae_encoder_layers = 12;
        command.act.decoder_layers = 12;
        command.act.attention_heads = 16;
        command.act.feedforward_dim = 16_384;

        let training = training(None);
        let error = start_training(&training, command).await.unwrap_err();
        assert!(error.message().contains("model and optimizer"));
        assert!(training.starts.lock().unwrap().is_empty());
    }

    #[test]
    fn aggregate_act_budget_is_inclusive_and_overflow_is_fail_closed() {
        assert!(
            enforce_act_training_resource_budget(Some(MAX_ACT_TRAINING_RESOURCE_BYTES)).is_ok()
        );
        assert!(
            enforce_act_training_resource_budget(MAX_ACT_TRAINING_RESOURCE_BYTES.checked_add(1))
                .is_err()
        );
        assert!(enforce_act_training_resource_budget(None).is_err());

        let mut overflow = training_command();
        overflow.act.hidden_dim = usize::MAX;
        overflow.act.feedforward_dim = usize::MAX;
        overflow.act.encoder_layers = usize::MAX;
        assert!(
            estimate_act_training_resource_bytes(&overflow, usize::MAX).is_none(),
            "checked resource arithmetic must never wrap to an admitted estimate"
        );
    }

    #[tokio::test]
    async fn malformed_rollout_is_rejected_before_training_or_runtime_lookup() {
        let training = training(run(false, 20_000, true));
        for command in [
            RolloutRequest {
                run_id: String::new(),
                ..rollout_command()
            },
            RolloutRequest {
                checkpoint_step: 0,
                ..rollout_command()
            },
            RolloutRequest {
                duration_seconds: spiral_robotics_core::operation::MAX_ROLLOUT_DURATION_SECONDS + 1,
                ..rollout_command()
            },
        ] {
            assert!(matches!(
                admit_rollout(&training, command),
                Err(ApplicationError::InvalidInput(_))
            ));
        }
    }

    fn run(active: bool, step: u64, inference_ready: bool) -> Option<TrainingRunStatus> {
        Some(TrainingRunStatus {
            active,
            checkpoints: vec![TrainingCheckpointStatus {
                step,
                inference_ready,
            }],
        })
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
            let error = admit_rollout(&training, rollout_command()).unwrap_err();
            assert!(error.message().contains(expected));
        }
    }

    #[tokio::test]
    async fn ready_rollout_returns_the_exact_admitted_semantic_command() {
        let training = training(run(false, 20_000, true));
        let command = rollout_command();

        assert_eq!(admit_rollout(&training, command.clone()), Ok(command));
    }
}
