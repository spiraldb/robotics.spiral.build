//! Rust-owned supervision for Spiral ADBC and Burn-native vision ACT.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use adbc_driver_manager::ManagedConnection;
use spiral_robotics_core::api::{
    ConnectionTestResponse, MAX_TRAINING_HISTORY_RUNS, StartTrainingRequest, TrainingCheckpoint,
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingSnapshot,
    TrainingSource, TrainingUpdate,
};
use spiral_robotics_core::spiral;
use spiral_robotics_inference::act::{
    NativeActTrainer, VisionActCheckpoint, VisionActTrainingConfig,
};
use tokio::sync::{broadcast, mpsc};

use crate::backend::storage::manager::{StorageManager, TrainingCheckpointWrite};
use crate::backend::storage::settings::StoredSpiralSettings;
use crate::backend::storage::sql::{self, ReadOnlyMode};
use crate::backend::training::data::SpiralActLoader;
use crate::backend::training::{ACT_ARTIFACT_FORMAT, ACT_TENSOR_NAMESPACE};

const HISTORY_FILE: &str = "runs.json";
const STOP_GRACE: Duration = Duration::from_secs(30);
// The current checkpoint relation stores one artifact per Binary cell. Stay
// below Spiral's default 256 MiB table-operation limit with headroom for the
// other columns in the ingest batch.
const MAX_CHECKPOINT_BYTES: u64 = 240 * 1024 * 1024;
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingManagerInner>,
}

struct TrainingManagerInner {
    root: PathBuf,
    storage: Arc<StorageManager>,
    snapshot: Mutex<TrainingSnapshot>,
    updates: broadcast::Sender<TrainingUpdate>,
    controls: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
}

enum NativeEvent {
    Configured(Box<StartTrainingRequest>),
    Started(String),
    Metric(TrainingMetricPoint),
    Checkpoint {
        step: u64,
        created_ms: i64,
        validation_loss: Option<f64>,
        manifest: serde_json::Value,
        payload: Vec<u8>,
        acknowledged: std::sync::mpsc::SyncSender<Result<(), String>>,
    },
    Log(String),
}

impl TrainingManager {
    pub(crate) async fn open(root: PathBuf, storage: Arc<StorageManager>) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|error| format!("could not create {}: {error}", root.display()))?;
        let root = fs::canonicalize(&root)
            .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
        let mut snapshot = load_snapshot(&root).unwrap_or_default();
        for run in &mut snapshot.runs {
            if run.phase.active() {
                run.phase = TrainingRunPhase::Failed;
                run.finished_ms = Some(now_ms());
                run.message = Some("The backend restarted while this run was active.".to_owned());
            }
        }
        snapshot.readiness = readiness(&storage).await;
        let (updates, _) = broadcast::channel(512);
        let manager = Self {
            inner: Arc::new(TrainingManagerInner {
                root,
                storage,
                snapshot: Mutex::new(snapshot),
                updates,
                controls: Mutex::new(BTreeMap::new()),
            }),
        };
        manager.persist();
        Ok(manager)
    }

    pub(crate) fn snapshot(&self) -> TrainingSnapshot {
        lock(&self.inner.snapshot).clone()
    }

    /// Capture the initial state and subscribe to later changes as one ordered
    /// operation. Publishers send while holding the same snapshot lock, so an
    /// update can appear in the snapshot or the receiver, never both.
    pub(crate) fn subscription(&self) -> (TrainingSnapshot, broadcast::Receiver<TrainingUpdate>) {
        let snapshot = lock(&self.inner.snapshot);
        let updates = self.inner.updates.subscribe();
        (snapshot.clone(), updates)
    }

    pub(crate) async fn refresh_readiness(&self) -> TrainingReadiness {
        let next = readiness(&self.inner.storage).await;
        if lock(&self.inner.snapshot).readiness != next {
            self.publish(
                TrainingUpdate::Readiness {
                    readiness: next.clone(),
                },
                true,
            );
        }
        next
    }

    pub(crate) async fn start(&self, request: StartTrainingRequest) -> Result<TrainingRun, String> {
        validate_request(&request)?;
        let readiness = self.refresh_readiness().await;
        if !readiness.ready {
            return Err(readiness.summary);
        }
        let (settings, source) = self.inner.storage.training_source(&request.dataset_id)?;
        self.inner
            .storage
            .validate_training_query(
                settings.clone(),
                &request.sql,
                &request.state_column,
                &request.action_column,
            )
            .await?;
        let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("train-{}-{sequence}", now_ms());
        let run = TrainingRun {
            id: id.clone(),
            config: request.clone(),
            source: source.clone(),
            phase: TrainingRunPhase::Starting,
            current_step: 0,
            started_ms: now_ms(),
            finished_ms: None,
            message: Some("Opening the Spiral ADBC stream and Burn runtime…".to_owned()),
            metrics: Vec::new(),
            checkpoints: Vec::new(),
            logs: Vec::new(),
        };

        // The invariant and insertion share one lock. Two simultaneous POSTs
        // cannot both pass the active-run check.
        {
            let mut snapshot = lock(&self.inner.snapshot);
            if snapshot.runs.iter().any(|run| run.phase.active()) {
                return Err("Stop the active training run before starting another.".to_owned());
            }
            let update = TrainingUpdate::RunAdded {
                run: Box::new(run.clone()),
            };
            snapshot.apply(update.clone());
            snapshot.runs.truncate(MAX_TRAINING_HISTORY_RUNS);
            let _ = self.inner.updates.send(update);
        }
        self.persist();

        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner.controls).insert(id.clone(), cancel.clone());
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .run_native(id, request, settings, source, cancel)
                .await;
        });
        Ok(run)
    }

    pub(crate) fn stop(&self, run_id: &str) -> Result<(), String> {
        let cancel = lock(&self.inner.controls).get(run_id).cloned();
        let Some(cancel) = cancel else {
            return Err(format!("training run {run_id:?} is not active"));
        };
        self.set_phase(
            run_id,
            TrainingRunPhase::Stopping,
            None,
            Some(
                "Finishing the current Arrow read or optimizer step and publishing a final checkpoint…"
                    .to_owned(),
            ),
        );
        cancel.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        let controls = std::mem::take(&mut *lock(&self.inner.controls));
        for (_, cancel) in controls {
            cancel.store(true, Ordering::Release);
        }
        let _ = tokio::time::timeout(STOP_GRACE + Duration::from_secs(2), async {
            while lock(&self.inner.snapshot).active_run().is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    async fn run_native(
        &self,
        run_id: String,
        request: StartTrainingRequest,
        settings: StoredSpiralSettings,
        source: TrainingSource,
        cancel: Arc<AtomicBool>,
    ) {
        let pretrained = if request.act.pretrained_backbone {
            match tokio::task::spawn_blocking(load_pretrained_backbone).await {
                Ok(Ok(bytes)) => Some(bytes),
                Ok(Err(error)) => {
                    self.fail(&run_id, error);
                    return;
                }
                Err(error) => {
                    self.fail(&run_id, format!("pretrained backbone task failed: {error}"));
                    return;
                }
            }
        } else {
            None
        };

        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let training_request = request.clone();
        let training_source = source.clone();
        let training_run_id = run_id.clone();
        let training_cancel = cancel.clone();
        let storage = self.inner.storage.clone();
        let training_settings = settings.clone();
        let mut training = Box::pin(storage.run_training_adbc(
            settings.clone(),
            move |connection| {
                train_blocking(
                    connection,
                    &training_run_id,
                    &training_request,
                    &training_settings,
                    &training_source,
                    training_cancel,
                    pretrained.as_deref(),
                    events_tx,
                )
            },
        ));
        let mut publication_error = None;
        let mut resolved_request = request.clone();
        let mut events_open = true;
        let terminal = loop {
            tokio::select! {
                result = &mut training => break result,
                event = events_rx.recv(), if events_open => {
                    match event {
                        None => events_open = false,
                        Some(event) => match event {
                        NativeEvent::Configured(config) => {
                            resolved_request = (*config).clone();
                            self.publish(
                                TrainingUpdate::RunConfigured {
                                    run_id: run_id.clone(),
                                    config,
                                },
                                true,
                            );
                        }
                        NativeEvent::Started(message) => {
                            if self.phase(&run_id) == Some(TrainingRunPhase::Starting) {
                                self.set_phase(&run_id, TrainingRunPhase::Running, None, Some(message));
                            }
                        }
                        NativeEvent::Metric(point) => self.publish(
                            TrainingUpdate::Metric {
                                run_id: run_id.clone(),
                                point: Box::new(point),
                            },
                            false,
                        ),
                        NativeEvent::Log(message) => self.push_log(&run_id, message),
                        NativeEvent::Checkpoint {
                            step,
                            created_ms,
                            validation_loss,
                            manifest,
                            payload,
                            acknowledged,
                        } => {
                            let result = if publication_error.is_none() {
                                self
                                    .publish_checkpoint_bytes(
                                        &run_id,
                                        &resolved_request,
                                        &settings,
                                        &source,
                                        step,
                                        created_ms,
                                        validation_loss,
                                        &manifest,
                                        payload,
                                    )
                                    .await
                            } else {
                                Err(publication_error.clone().expect("publication error"))
                            };
                            let _ = acknowledged.send(result.clone());
                            if let Err(error) = result {
                                cancel.store(true, Ordering::Release);
                                publication_error = Some(error);
                            }
                        }
                        },
                    }
                }
            }
        };
        while let Ok(event) = events_rx.try_recv() {
            match event {
                NativeEvent::Configured(config) => {
                    self.publish(
                        TrainingUpdate::RunConfigured {
                            run_id: run_id.clone(),
                            config,
                        },
                        true,
                    );
                }
                NativeEvent::Started(message) => {
                    if self.phase(&run_id) == Some(TrainingRunPhase::Starting) {
                        self.set_phase(&run_id, TrainingRunPhase::Running, None, Some(message));
                    }
                }
                NativeEvent::Metric(point) => self.publish(
                    TrainingUpdate::Metric {
                        run_id: run_id.clone(),
                        point: Box::new(point),
                    },
                    false,
                ),
                NativeEvent::Log(message) => self.push_log(&run_id, message),
                NativeEvent::Checkpoint { acknowledged, .. } => {
                    let error = "trainer completed before checkpoint publication".to_owned();
                    let _ = acknowledged.send(Err(error.clone()));
                    publication_error.get_or_insert(error);
                }
            }
        }
        lock(&self.inner.controls).remove(&run_id);
        if let Some(error) = publication_error {
            self.fail(&run_id, error);
            return;
        }
        match terminal {
            Err(error) => self.fail(&run_id, error),
            Ok(outcome) if outcome.stopped && outcome.steps == 0 => self.set_phase(
                &run_id,
                TrainingRunPhase::Stopped,
                Some(now_ms()),
                Some(
                    "Stopped before the first optimizer step; no checkpoint was produced."
                        .to_owned(),
                ),
            ),
            Ok(outcome) if outcome.stopped => self.set_phase(
                &run_id,
                TrainingRunPhase::Stopped,
                Some(now_ms()),
                Some(format!(
                    "Stopped after step {}; the final checkpoint was published to Spiral.",
                    outcome.steps
                )),
            ),
            Ok(outcome) => self.set_phase(
                &run_id,
                TrainingRunPhase::Completed,
                Some(now_ms()),
                Some(format!(
                    "Completed {} steps; the final checkpoint was published to Spiral.",
                    outcome.steps
                )),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_checkpoint_bytes(
        &self,
        run_id: &str,
        request: &StartTrainingRequest,
        settings: &StoredSpiralSettings,
        source: &TrainingSource,
        step: u64,
        created_ms: i64,
        validation_loss: Option<f64>,
        manifest: &serde_json::Value,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        if step == 0 || step > request.steps {
            return Err(format!(
                "trainer returned checkpoint step {step}, outside this run"
            ));
        }
        if validation_loss.is_some_and(|loss| !loss.is_finite()) {
            return Err("trainer returned a non-finite validation loss".to_owned());
        }
        let byte_size = payload.len() as u64;
        if byte_size > MAX_CHECKPOINT_BYTES {
            return Err(
                "checkpoint exceeds the 240 MiB single-artifact publication limit".to_owned(),
            );
        }
        let file_name = format!("checkpoint-{step}.safetensors");
        let config_json =
            validate_checkpoint_artifact(&file_name, &payload, manifest, run_id, step, request)?;
        let policy = serde_json::to_value(request.policy)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "act".to_owned());
        self.inner
            .storage
            .publish_training_checkpoint(
                settings.clone(),
                source.checkpoint_table.clone(),
                TrainingCheckpointWrite {
                    run_id: run_id.to_owned(),
                    run_name: request.run_name.clone(),
                    policy,
                    step,
                    created_ms,
                    validation_loss,
                    config_json,
                    payload,
                },
            )
            .await?;
        self.publish(
            TrainingUpdate::Checkpoint {
                run_id: run_id.to_owned(),
                checkpoint: TrainingCheckpoint {
                    step,
                    created_ms,
                    validation_loss,
                    spiral_table: source.checkpoint_table.clone(),
                    byte_size,
                    inference_ready: true,
                    recommended_action_steps: request.chunk_size,
                    sample_hz: manifest
                        .pointer("/data_contract/sample_hz")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or_default(),
                },
            },
            true,
        );
        Ok(())
    }

    fn phase(&self, run_id: &str) -> Option<TrainingRunPhase> {
        lock(&self.inner.snapshot)
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| run.phase)
    }

    fn set_phase(
        &self,
        run_id: &str,
        phase: TrainingRunPhase,
        finished_ms: Option<i64>,
        message: Option<String>,
    ) {
        let current_step = lock(&self.inner.snapshot)
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map_or(0, |run| run.current_step);
        self.publish(
            TrainingUpdate::RunState {
                run_id: run_id.to_owned(),
                phase,
                current_step,
                finished_ms,
                message,
            },
            true,
        );
    }

    fn fail(&self, run_id: &str, message: String) {
        self.set_phase(
            run_id,
            TrainingRunPhase::Failed,
            Some(now_ms()),
            Some(message),
        );
        lock(&self.inner.controls).remove(run_id);
    }

    fn push_log(&self, run_id: &str, line: String) {
        self.publish(
            TrainingUpdate::Log {
                run_id: run_id.to_owned(),
                line,
            },
            false,
        );
    }

    fn publish(&self, update: TrainingUpdate, persist: bool) {
        {
            let mut snapshot = lock(&self.inner.snapshot);
            snapshot.apply(update.clone());
            let _ = self.inner.updates.send(update);
        }
        if persist {
            self.persist();
        }
    }

    fn persist(&self) {
        let snapshot = lock(&self.inner.snapshot).clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) {
            let path = self.inner.root.join(HISTORY_FILE);
            let temporary = self.inner.root.join("runs.json.tmp");
            if fs::write(&temporary, bytes).is_ok() {
                let _ = fs::rename(temporary, path);
            }
        }
    }
}

struct NativeOutcome {
    steps: u64,
    stopped: bool,
}

#[allow(clippy::too_many_arguments)]
fn train_blocking(
    connection: &mut ManagedConnection,
    run_id: &str,
    request: &StartTrainingRequest,
    settings: &StoredSpiralSettings,
    source: &TrainingSource,
    cancel: Arc<AtomicBool>,
    pretrained: Option<&[u8]>,
    events: mpsc::UnboundedSender<NativeEvent>,
) -> Result<NativeOutcome, String> {
    let _ = events.send(NativeEvent::Log(
        "Opening the dedicated Spiral ADBC connection and indexing eligible ACT anchors…"
            .to_owned(),
    ));
    let mut loader = SpiralActLoader::open(connection, request, &settings.dataset)?;
    let mut resolved_request = request.clone();
    resolved_request.act.camera_streams = loader.camera_streams().to_vec();
    let _ = events.send(NativeEvent::Configured(Box::new(resolved_request.clone())));
    let config = VisionActTrainingConfig {
        state_dimensions: loader.state_dimensions(),
        action_dimensions: loader.action_dimensions(),
        camera_streams: resolved_request.act.camera_streams.clone(),
        image_width: loader.image_width(),
        image_height: loader.image_height(),
        chunk_size: request.chunk_size,
        hidden_dimensions: request.act.hidden_dim,
        latent_dimensions: request.act.latent_dim,
        attention_heads: request.act.attention_heads,
        feedforward_dimensions: request.act.feedforward_dim,
        encoder_layers: request.act.encoder_layers,
        vae_encoder_layers: request.act.vae_encoder_layers,
        decoder_layers: request.act.decoder_layers,
        kl_weight: request.act.kl_weight,
        learning_rate: request.learning_rate,
        total_steps: request.steps,
        seed: request.seed,
    };
    let mut trainer = NativeActTrainer::new(config, &request.device, pretrained)
        .map_err(|error| error.to_string())?;
    let _ = events.send(NativeEvent::Started(format!(
        "Vision ACT is running on Burn/{}: {}; {:.1}M parameters.",
        trainer.backend_name(),
        loader.summary(),
        trainer.parameter_count() as f64 / 1_000_000.0
    )));
    let _ = events.send(NativeEvent::Log(format!(
        "ACT inputs: state={}, action={}, image={}x{}, chunk={}, cameras={}. Spiral owns frame decode, optional resize, CHW conversion, and ImageNet normalization.",
        loader.state_dimensions(),
        loader.action_dimensions(),
        loader.image_width(),
        loader.image_height(),
        request.chunk_size,
        resolved_request.act.camera_streams.join(", ")
    )));

    let started = Instant::now();
    let mut metric_started = started;
    let mut examples_since_metric = 0_usize;
    let validation_every = request.checkpoint_every.saturating_div(4).clamp(25, 250);
    let mut last_validation = None;
    let mut last_checkpoint = 0;
    let mut completed_steps = 0;
    for step in 1..=request.steps {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let loaded = loader.fetch(request.batch_size, false)?;
        let loader_metrics = loaded.metrics;
        let loss = trainer
            .train_step(loaded.batch, step)
            .map_err(|error| error.to_string())?;
        completed_steps = step;
        examples_since_metric += request.batch_size;
        let mut event = Vec::new();
        let validation_loss = if loader.has_validation() && step % validation_every == 0 {
            let batch = loader.fetch(request.batch_size, true)?;
            let loss = trainer
                .validation_step(batch.batch)
                .map_err(|error| error.to_string())?;
            last_validation = Some(loss.total);
            event.push("validation");
            last_validation
        } else {
            None
        };
        if loader_metrics.seconds >= 2.0 {
            event.push("loader stall");
        }
        if step == 1 || step % 10 == 0 || step == request.steps || !event.is_empty() {
            let now = Instant::now();
            let elapsed = now.duration_since(metric_started).as_secs_f64().max(1.0e-9);
            let _ = events.send(NativeEvent::Metric(TrainingMetricPoint {
                step,
                train_loss: loss.total,
                validation_loss,
                learning_rate: trainer.learning_rate(step),
                transfer_bytes_per_second: loader_metrics.source_bytes as f64
                    / loader_metrics.seconds.max(1.0e-9),
                examples_per_second: examples_since_metric as f64 / elapsed,
                elapsed_seconds: now.duration_since(started).as_secs_f64(),
                event: (!event.is_empty()).then(|| event.join(" · ")),
            }));
            if step == 1 {
                let _ = events.send(NativeEvent::Log(format!(
                    "Loss components at step 1: L1={:.4}, KL={:.4}, pad={:.4}; decoded {} frame(s).",
                    loss.l1, loss.kl, loss.pad, loader_metrics.requested_frames
                )));
            }
            metric_started = now;
            examples_since_metric = 0;
        }
        if step % request.checkpoint_every == 0 {
            send_checkpoint(
                &events,
                run_id,
                &resolved_request,
                source,
                &loader,
                &trainer,
                step,
                last_validation,
            )?;
            last_checkpoint = step;
        }
    }
    if completed_steps > 0 && last_checkpoint != completed_steps {
        send_checkpoint(
            &events,
            run_id,
            &resolved_request,
            source,
            &loader,
            &trainer,
            completed_steps,
            last_validation,
        )?;
    }
    Ok(NativeOutcome {
        steps: completed_steps,
        stopped: cancel.load(Ordering::Acquire),
    })
}

#[allow(clippy::too_many_arguments)]
fn send_checkpoint(
    events: &mpsc::UnboundedSender<NativeEvent>,
    run_id: &str,
    request: &StartTrainingRequest,
    source: &TrainingSource,
    loader: &SpiralActLoader<'_>,
    trainer: &NativeActTrainer,
    step: u64,
    validation_loss: Option<f64>,
) -> Result<(), String> {
    let manifest = checkpoint_manifest(run_id, request, source, loader, step);
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|error| format!("encoding ACT checkpoint manifest: {error}"))?;
    let payload = trainer
        .checkpoint(VisionActCheckpoint {
            manifest_json: &manifest_json,
            state_mean: &loader.stats.state_mean,
            state_std: &loader.stats.state_std,
            action_mean: &loader.stats.action_mean,
            action_std: &loader.stats.action_std,
        })
        .map_err(|error| error.to_string())?;
    let (acknowledged, result) = std::sync::mpsc::sync_channel(1);
    events
        .send(NativeEvent::Checkpoint {
            step,
            created_ms: now_ms(),
            validation_loss,
            manifest,
            payload,
            acknowledged,
        })
        .map_err(|_| "training event channel closed before checkpoint publication".to_owned())?;
    result
        .recv()
        .map_err(|_| "checkpoint publisher stopped before acknowledging the artifact".to_owned())?
}

fn checkpoint_manifest(
    run_id: &str,
    request: &StartTrainingRequest,
    source: &TrainingSource,
    loader: &SpiralActLoader<'_>,
    step: u64,
) -> serde_json::Value {
    let mut input_features = serde_json::Map::new();
    input_features.insert(
        "observation.state".to_owned(),
        serde_json::json!({"type": "STATE", "shape": [loader.state_dimensions()]}),
    );
    for camera in &request.act.camera_streams {
        input_features.insert(
            camera.clone(),
            serde_json::json!({
                "type": "VISUAL",
                "shape": [3, loader.image_height(), loader.image_width()]
            }),
        );
    }
    let model_config = serde_json::json!({
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD"
        },
        "input_features": input_features,
        "output_features": {
            "action": {"type": "ACTION", "shape": [loader.action_dimensions()]}
        },
        "chunk_size": request.chunk_size,
        "n_action_steps": request.chunk_size,
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": false,
        "pre_norm": false,
        "dim_model": request.act.hidden_dim,
        "n_heads": request.act.attention_heads,
        "dim_feedforward": request.act.feedforward_dim,
        "feedforward_activation": "relu",
        "n_encoder_layers": request.act.encoder_layers,
        "n_vae_encoder_layers": request.act.vae_encoder_layers,
        "n_decoder_layers": request.act.decoder_layers,
        "use_vae": true,
        "latent_dim": request.act.latent_dim,
        "spiral_camera_embedding": false,
        "spiral_camera_order": request.act.camera_streams,
    });
    serde_json::json!({
        "format": ACT_ARTIFACT_FORMAT,
        "artifact": "inference",
        "run_id": run_id,
        "step": step,
        "request": request,
        "source": {
            "project": source.project,
            "dataset": source.dataset,
        },
        "data_contract": {
            "value_space": "centered_normalized",
            "robot_type": loader.robot_type,
            "joint_names": loader.joint_names,
            "camera_streams": request.act.camera_streams,
            "sample_hz": loader.sample_hz,
            "image_layout": "chw",
            "image_normalization": "imagenet",
            "frame_alignment": "fixed_fps",
            "spiral_schema_version": spiral::SCHEMA_VERSION,
        },
        "runtime": {
            "ml": "burn 0.21",
            "transport": "adbc 0.24 / arrow 58.4",
            "language": "rust",
        },
        "normalization": "embedded_mean_std",
        "model_config": model_config,
        "tensor_namespace": ACT_TENSOR_NAMESPACE,
    })
}

fn load_pretrained_backbone() -> Result<Vec<u8>, String> {
    spiral_robotics_inference::act::imagenet_resnet18_weights()
        .map_err(|error| format!("loading torchvision ResNet18 IMAGENET1K_V1: {error}"))
}

fn runtime_description() -> String {
    #[cfg(target_os = "macos")]
    let acceleration = if spiral_robotics_inference::act::metal_available() {
        "Metal + CPU"
    } else {
        "CPU"
    };
    #[cfg(not(target_os = "macos"))]
    let acceleration = "CPU";
    format!("Burn 0.21 · {acceleration} · ADBC Arrow tensors verified at run start")
}

async fn readiness(storage: &StorageManager) -> TrainingReadiness {
    let spiral = storage.training_source("default");
    let connection = if spiral.is_ok() {
        Some(
            tokio::time::timeout(Duration::from_secs(8), storage.test_spiral())
                .await
                .unwrap_or_else(|_| ConnectionTestResponse {
                    ok: false,
                    message: "the connection probe timed out after 8 seconds".to_owned(),
                }),
        )
    } else {
        None
    };
    let spiral_ready = connection.as_ref().is_some_and(|result| result.ok);
    let ready = spiral_ready;
    let summary = match (spiral, connection) {
        (Err(error), _) => error,
        (_, Some(result)) if !result.ok => {
            format!("Spiral is not ready for training: {}", result.message)
        }
        (Ok((_, source)), Some(_)) => format!(
            "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
            source.project,
            source.dataset,
            probe = runtime_description(),
        ),
        _ => "Spiral readiness could not be determined.".to_owned(),
    };
    TrainingReadiness {
        ready,
        spiral_ready,
        summary,
    }
}

fn validate_checkpoint_artifact(
    file_name: &str,
    payload: &[u8],
    manifest: &serde_json::Value,
    run_id: &str,
    step: u64,
    request: &StartTrainingRequest,
) -> Result<String, String> {
    if Path::new(file_name).extension() != Some(std::ffi::OsStr::new("safetensors")) {
        return Err("the native trainer must publish a .safetensors checkpoint".to_owned());
    }
    let tensors = safetensors::SafeTensors::deserialize(payload)
        .map_err(|error| format!("checkpoint {file_name} is not valid SafeTensors: {error}"))?;
    if tensors.is_empty() {
        return Err(format!("checkpoint {file_name} contains no tensors"));
    }
    let (_, metadata) = safetensors::SafeTensors::read_metadata(payload)
        .map_err(|error| format!("checkpoint {file_name} metadata is invalid: {error}"))?;
    let metadata = metadata
        .metadata()
        .as_ref()
        .ok_or_else(|| format!("checkpoint {file_name} has no artifact metadata"))?;
    if metadata.get("format").map(String::as_str) != Some("spiral-act") {
        return Err(format!(
            "checkpoint {file_name} does not declare its tensor framework"
        ));
    }
    let embedded_manifest = metadata
        .get("spiral_robotics")
        .ok_or_else(|| format!("checkpoint {file_name} has no Spiral manifest"))?;
    let embedded_manifest: serde_json::Value =
        serde_json::from_str(embedded_manifest).map_err(|error| {
            format!("checkpoint {file_name} has an invalid Spiral manifest: {error}")
        })?;
    if &embedded_manifest != manifest {
        return Err(format!(
            "checkpoint {file_name} metadata does not match the trainer event"
        ));
    }
    if manifest.get("format").and_then(serde_json::Value::as_str) != Some(ACT_ARTIFACT_FORMAT)
        || manifest.get("artifact").and_then(serde_json::Value::as_str) != Some("inference")
        || manifest.get("run_id").and_then(serde_json::Value::as_str) != Some(run_id)
        || manifest.get("step").and_then(serde_json::Value::as_u64) != Some(step)
    {
        return Err(format!(
            "checkpoint {file_name} has the wrong format, purpose, run, or step"
        ));
    }
    let request_json = serde_json::to_value(request)
        .map_err(|error| format!("could not validate checkpoint configuration: {error}"))?;
    if manifest.get("request") != Some(&request_json) {
        return Err(format!(
            "checkpoint {file_name} configuration does not match the active run"
        ));
    }
    let model_config = manifest
        .get("model_config")
        .ok_or_else(|| format!("checkpoint {file_name} has no native model configuration"))?;
    if manifest
        .get("tensor_namespace")
        .and_then(serde_json::Value::as_str)
        != Some(ACT_TENSOR_NAMESPACE)
    {
        return Err(format!(
            "checkpoint {file_name} does not use the native ACT tensor namespace"
        ));
    }
    serde_json::to_string(model_config)
        .map_err(|error| format!("could not encode native model configuration: {error}"))
}

fn validate_request(request: &StartTrainingRequest) -> Result<(), String> {
    let name = request.run_name.trim();
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(
            "Run name must use 1–80 letters, numbers, '.', '_' or '-' characters.".to_owned(),
        );
    }
    if !spiral::is_dataset_id(request.dataset_id.trim()) {
        return Err(
            "Dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters.".to_owned(),
        );
    }
    sql::ensure_read_only(&request.sql, ReadOnlyMode::Training)?;
    for (label, column) in [
        ("State column", request.state_column.trim()),
        ("Action column", request.action_column.trim()),
    ] {
        if !spiral::is_identifier(column) {
            return Err(format!("{label} must be a SQL identifier."));
        }
    }
    if !(1..=10_000_000).contains(&request.steps) {
        return Err("Training steps must be between 1 and 10,000,000.".to_owned());
    }
    if !(1..=512).contains(&request.batch_size) {
        return Err("Batch size must be between 1 and 512.".to_owned());
    }
    if !(1..=256).contains(&request.chunk_size) {
        return Err("Action chunk size must be between 1 and 256.".to_owned());
    }
    if request
        .batch_size
        .checked_mul(request.chunk_size)
        .is_none_or(|rows| rows > 8_192)
    {
        return Err(
            "Batch size × action chunk size may not exceed 8,192 manifest rows.".to_owned(),
        );
    }
    if request.checkpoint_every == 0 || request.checkpoint_every > request.steps {
        return Err("Checkpoint cadence must be between 1 and the run step count.".to_owned());
    }
    if !request.learning_rate.is_finite() || request.learning_rate <= 0.0 {
        return Err("Learning rate must be a finite positive number.".to_owned());
    }
    if request.act.camera_streams.len() > 8 {
        return Err("ACT supports at most eight camera streams.".to_owned());
    }
    let mut camera_streams = BTreeSet::new();
    for stream in &request.act.camera_streams {
        if stream.is_empty()
            || !stream
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(format!("Invalid ACT camera stream {stream:?}."));
        }
        if !camera_streams.insert(stream) {
            return Err(format!("ACT camera stream {stream:?} is repeated."));
        }
    }
    let native_images = request.act.image_width == 0 && request.act.image_height == 0;
    if !native_images
        && (!(32..=1_024).contains(&request.act.image_width)
            || !(32..=1_024).contains(&request.act.image_height))
    {
        return Err(
            "ACT image width and height must both be zero (native) or between 32 and 1,024."
                .to_owned(),
        );
    }
    if !matches!(
        request.act.video_decoder.as_str(),
        "auto" | "ffmpeg" | "videotoolbox" | "nvdec"
    ) {
        return Err("ACT video decoder must be auto, ffmpeg, videotoolbox, or nvdec.".to_owned());
    }
    if !(1..=16).contains(&request.act.attention_heads) {
        return Err("ACT attention heads must be between 1 and 16.".to_owned());
    }
    if !(64..=1_024).contains(&request.act.hidden_dim)
        || !request.act.hidden_dim.is_multiple_of(4)
        || !request
            .act
            .hidden_dim
            .is_multiple_of(request.act.attention_heads)
    {
        return Err(
            "ACT hidden size must be 64–1,024 and divisible by four and the attention-head count."
                .to_owned(),
        );
    }
    if !(1..=12).contains(&request.act.encoder_layers)
        || !(1..=12).contains(&request.act.decoder_layers)
        || !(1..=512).contains(&request.act.latent_dim)
        || request.act.feedforward_dim < request.act.hidden_dim
        || request.act.feedforward_dim > 16_384
    {
        return Err("ACT transformer configuration is outside the supported range.".to_owned());
    }
    if !request.act.kl_weight.is_finite() || request.act.kl_weight < 0.0 {
        return Err("ACT KL weight must be finite and non-negative.".to_owned());
    }
    Ok(())
}

fn load_snapshot(root: &Path) -> Option<TrainingSnapshot> {
    serde_json::from_slice(&fs::read(root.join(HISTORY_FILE)).ok()?).ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_vision_act_request_discovers_dataset_camera_streams() {
        let request = StartTrainingRequest {
            sql: "SELECT 1".to_owned(),
            ..StartTrainingRequest::default()
        };

        validate_request(&request).unwrap();
        assert!(request.act.camera_streams.is_empty());
    }

    #[test]
    fn training_request_rejects_mutating_sql() {
        let request = StartTrainingRequest {
            sql: "DELETE FROM spiral_robotics.joint_samples".to_owned(),
            ..StartTrainingRequest::default()
        };
        assert!(
            validate_request(&request)
                .unwrap_err()
                .contains("read-only")
        );
    }

    #[test]
    fn training_request_requires_a_single_query() {
        let request = StartTrainingRequest {
            sql: "SELECT state, action FROM spiral_robotics.training_samples; DROP TABLE episodes"
                .to_owned(),
            ..StartTrainingRequest::default()
        };
        assert!(
            validate_request(&request)
                .unwrap_err()
                .contains("exactly one")
        );
    }

    #[test]
    fn training_columns_must_start_like_sql_identifiers() {
        let request = StartTrainingRequest {
            state_column: "1state".to_owned(),
            sql: "SELECT 1".to_owned(),
            ..StartTrainingRequest::default()
        };

        assert!(
            validate_request(&request)
                .unwrap_err()
                .contains("identifier")
        );
    }

    #[test]
    fn checkpoint_publication_accepts_only_matching_safetensors_artifacts() {
        use std::collections::HashMap;

        use safetensors::tensor::{Dtype, TensorView, serialize};

        let request = StartTrainingRequest::default();
        let manifest = serde_json::json!({
            "format": ACT_ARTIFACT_FORMAT,
            "artifact": "inference",
            "run_id": "train-1",
            "step": 10,
            "request": request,
            "model_config": {"type": "act"},
            "tensor_namespace": ACT_TENSOR_NAMESPACE,
        });
        let metadata = HashMap::from([
            ("format".to_owned(), "spiral-act".to_owned()),
            (
                "spiral_robotics".to_owned(),
                serde_json::to_string(&manifest).unwrap(),
            ),
        ]);
        let data = [0_u8; 4];
        let tensor = TensorView::new(Dtype::F32, vec![1], &data).unwrap();
        let payload = serialize([("weight", tensor)], Some(metadata)).unwrap();

        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.safetensors",
                &payload,
                &manifest,
                "train-1",
                10,
                &request,
            )
            .is_ok()
        );
        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.pt",
                &payload,
                &manifest,
                "train-1",
                10,
                &request,
            )
            .unwrap_err()
            .contains(".safetensors")
        );
        assert!(
            validate_checkpoint_artifact(
                "checkpoint-10.safetensors",
                b"pickle",
                &manifest,
                "train-1",
                10,
                &request,
            )
            .unwrap_err()
            .contains("not valid SafeTensors")
        );
    }
}
