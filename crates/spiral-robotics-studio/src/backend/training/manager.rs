//! Rust-owned supervision for the Spiral-backed PyTorch worker.
//!
//! Python owns small read-only Spiral metadata reads through Arrow and the
//! per-step `to_torch()` sample path. Rust retains process lifecycle, query
//! authorization, persistence, checkpoint publication, and browser events.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::pending;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use spiral_robotics_core::api::{
    ConnectionTestResponse, MAX_TRAINING_HISTORY_RUNS, StartTrainingRequest, TrainingCheckpoint,
    TrainingCheckpointFormat, TrainingMetricPoint, TrainingReadiness, TrainingRun,
    TrainingRunPhase, TrainingSnapshot, TrainingSource, TrainingUpdate,
};
use spiral_robotics_core::spiral;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, oneshot};

use crate::backend::storage::manager::{StorageManager, TrainingCheckpointWrite};
use crate::backend::storage::settings::StoredSpiralSettings;
use crate::backend::storage::sql::{self, ReadOnlyMode};

const HISTORY_FILE: &str = "runs.json";
const STOP_GRACE: Duration = Duration::from_secs(10);
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
    python: PathBuf,
    worker_script: PathBuf,
    storage: Arc<StorageManager>,
    python_probe: Result<String, String>,
    snapshot: Mutex<TrainingSnapshot>,
    updates: broadcast::Sender<TrainingUpdate>,
    controls: Mutex<BTreeMap<String, oneshot::Sender<()>>>,
}

#[derive(Debug, Serialize)]
struct WorkerConfig {
    run_id: String,
    request: StartTrainingRequest,
    spiral_project: String,
    spiral_dataset: String,
    stop_file: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerEvent {
    Started {
        message: String,
    },
    Metric {
        #[serde(flatten)]
        point: TrainingMetricPoint,
    },
    CheckpointReady {
        step: u64,
        created_ms: i64,
        validation_loss: Option<f64>,
        file_name: String,
        byte_size: u64,
        manifest: serde_json::Value,
    },
    Log {
        message: String,
    },
    Completed {
        message: String,
    },
    Stopped {
        message: String,
    },
    Failed {
        message: String,
    },
}

enum WorkerTerminal {
    Completed(String),
    Stopped(String),
    Failed(String),
}

impl TrainingManager {
    pub(crate) async fn open(
        root: PathBuf,
        mut python: PathBuf,
        mut worker_script: PathBuf,
        storage: Arc<StorageManager>,
    ) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|error| format!("could not create {}: {error}", root.display()))?;
        let root = fs::canonicalize(&root)
            .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
        let launch_directory = std::env::current_dir()
            .map_err(|error| format!("could not resolve the launch directory: {error}"))?;
        if worker_script.is_relative() {
            worker_script = launch_directory.join(worker_script);
        }
        // A bare `python3` intentionally remains a PATH lookup. Relative
        // interpreter paths containing a directory must be frozen before the
        // child changes into its per-run working directory.
        if python.is_relative() && python.components().count() > 1 {
            python = launch_directory.join(python);
        }
        let mut snapshot = load_snapshot(&root).unwrap_or_default();
        for run in &mut snapshot.runs {
            if run.phase.active() {
                run.phase = TrainingRunPhase::Failed;
                run.finished_ms = Some(now_ms());
                run.message = Some("The backend restarted while this run was active.".to_owned());
            }
        }
        snapshot.worker = worker_script.display().to_string();
        let python_probe = probe_worker(&python, &worker_script).await;
        snapshot.readiness = readiness(&python_probe, &storage).await;
        let (updates, _) = broadcast::channel(512);
        let manager = Self {
            inner: Arc::new(TrainingManagerInner {
                root,
                python,
                worker_script,
                storage,
                python_probe,
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
        let next = readiness(&self.inner.python_probe, &self.inner.storage).await;
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
        let (settings, source) = self.inner.storage.training_source()?;
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
            message: Some("Opening the Spiral stream and PyTorch runtime…".to_owned()),
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

        let (cancel_tx, cancel_rx) = oneshot::channel();
        lock(&self.inner.controls).insert(id.clone(), cancel_tx);
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .run_worker(id, request, settings, source, cancel_rx)
                .await;
        });
        Ok(run)
    }

    pub(crate) fn stop(&self, run_id: &str) -> Result<(), String> {
        let sender = lock(&self.inner.controls).remove(run_id);
        let Some(sender) = sender else {
            return Err(format!("training run {run_id:?} is not active"));
        };
        self.set_phase(
            run_id,
            TrainingRunPhase::Stopping,
            None,
            Some(
                "Finishing the current optimizer step and publishing a final checkpoint…"
                    .to_owned(),
            ),
        );
        let _ = sender.send(());
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        let controls = std::mem::take(&mut *lock(&self.inner.controls));
        for (_, sender) in controls {
            let _ = sender.send(());
        }
        let _ = tokio::time::timeout(STOP_GRACE + Duration::from_secs(2), async {
            while lock(&self.inner.snapshot).active_run().is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    async fn run_worker(
        &self,
        run_id: String,
        request: StartTrainingRequest,
        settings: StoredSpiralSettings,
        source: TrainingSource,
        mut cancel: oneshot::Receiver<()>,
    ) {
        let run_directory = self.inner.root.join(&run_id);
        if let Err(error) = tokio::fs::create_dir_all(&run_directory).await {
            self.fail(
                &run_id,
                format!("could not create the run directory: {error}"),
            );
            return;
        }
        let stop_path = run_directory.join("stop-requested");
        let _ = tokio::fs::remove_file(&stop_path).await;
        let config_path = run_directory.join("config.json");
        let config = WorkerConfig {
            run_id: run_id.clone(),
            request: request.clone(),
            spiral_project: settings.project_id.clone(),
            spiral_dataset: settings.dataset.clone(),
            stop_file: stop_path.display().to_string(),
        };
        let config_bytes = match serde_json::to_vec_pretty(&config) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(
                    &run_id,
                    format!("could not encode the run configuration: {error}"),
                );
                return;
            }
        };
        if let Err(error) = tokio::fs::write(&config_path, config_bytes).await {
            self.fail(
                &run_id,
                format!("could not write the run configuration: {error}"),
            );
            return;
        }

        let mut command = Command::new(&self.inner.python);
        command
            .arg(&self.inner.worker_script)
            .arg("--config")
            .arg(&config_path)
            .current_dir(&run_directory)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONNOUSERSITE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.fail(
                    &run_id,
                    format!(
                        "could not launch {}: {error}. Enter the Nix development shell or set --training-python.",
                        self.inner.python.display()
                    ),
                );
                return;
            }
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_manager = self.clone();
        let stderr_run = run_id.clone();
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_manager.push_log(&stderr_run, format!("stderr · {line}"));
            }
        });

        let mut lines = BufReader::new(stdout).lines();
        let mut stop_requested = false;
        let mut stop_deadline = None;
        let mut forced = false;
        let mut terminal = None;
        loop {
            let deadline = stop_deadline;
            tokio::select! {
                _ = &mut cancel, if !stop_requested => {
                    stop_requested = true;
                    let _ = tokio::fs::write(&stop_path, b"stop\n").await;
                    stop_deadline = Some(tokio::time::Instant::now() + STOP_GRACE);
                }
                _ = async {
                    if let Some(deadline) = deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        pending::<()>().await;
                    }
                } => {
                    forced = true;
                    let _ = child.start_kill();
                    break;
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if let Some(next) = self
                                .handle_worker_line(
                                    &run_id,
                                    &run_directory,
                                    &request,
                                    &settings,
                                    &source,
                                    &line,
                                )
                                .await
                            {
                                let failed = matches!(&next, WorkerTerminal::Failed(_));
                                if failed && !stop_requested {
                                    stop_requested = true;
                                    let _ = tokio::fs::write(&stop_path, b"stop\n").await;
                                    stop_deadline = Some(tokio::time::Instant::now() + STOP_GRACE);
                                }
                                if !matches!(terminal.as_ref(), Some(WorkerTerminal::Failed(_))) {
                                    terminal = Some(next);
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            terminal = Some(WorkerTerminal::Failed(format!(
                                "worker output failed: {error}"
                            )));
                            break;
                        }
                    }
                }
            }
        }

        let status = child.wait().await;
        let _ = stderr_task.await;
        lock(&self.inner.controls).remove(&run_id);
        let _ = tokio::fs::remove_file(&stop_path).await;

        if forced {
            if let Some(WorkerTerminal::Failed(message)) = terminal {
                self.fail(&run_id, message);
                return;
            }
            self.set_phase(
                &run_id,
                TrainingRunPhase::Stopped,
                Some(now_ms()),
                Some("The worker did not stop within 10 seconds and was terminated.".to_owned()),
            );
            return;
        }
        match (status, terminal) {
            (_, Some(WorkerTerminal::Failed(message))) => self.fail(&run_id, message),
            (Ok(status), Some(WorkerTerminal::Stopped(message))) if status.success() => {
                self.set_phase(
                    &run_id,
                    TrainingRunPhase::Stopped,
                    Some(now_ms()),
                    Some(message),
                );
            }
            (Ok(status), Some(WorkerTerminal::Completed(message))) if status.success() => {
                self.set_phase(
                    &run_id,
                    TrainingRunPhase::Completed,
                    Some(now_ms()),
                    Some(message),
                );
            }
            (Ok(status), _) if status.success() && stop_requested => {
                self.set_phase(
                    &run_id,
                    TrainingRunPhase::Stopped,
                    Some(now_ms()),
                    Some("Stopped after the current optimizer step.".to_owned()),
                );
            }
            (Ok(status), _) if status.success() => {
                self.set_phase(
                    &run_id,
                    TrainingRunPhase::Completed,
                    Some(now_ms()),
                    Some("Training completed.".to_owned()),
                );
            }
            (Ok(status), _) => {
                self.fail(&run_id, format!("the training worker exited with {status}"))
            }
            (Err(error), _) => {
                self.fail(&run_id, format!("could not wait for the worker: {error}"))
            }
        }
    }

    async fn handle_worker_line(
        &self,
        run_id: &str,
        run_directory: &Path,
        request: &StartTrainingRequest,
        settings: &StoredSpiralSettings,
        source: &TrainingSource,
        line: &str,
    ) -> Option<WorkerTerminal> {
        let event = match serde_json::from_str::<WorkerEvent>(line) {
            Ok(event) => event,
            Err(_) => {
                self.push_log(run_id, line.to_owned());
                return None;
            }
        };
        match event {
            WorkerEvent::Started { message } => {
                if self.phase(run_id) == Some(TrainingRunPhase::Starting) {
                    self.set_phase(run_id, TrainingRunPhase::Running, None, Some(message));
                }
                None
            }
            WorkerEvent::Metric { point } => {
                self.publish(
                    TrainingUpdate::Metric {
                        run_id: run_id.to_owned(),
                        point: Box::new(point),
                    },
                    false,
                );
                None
            }
            WorkerEvent::CheckpointReady {
                step,
                created_ms,
                validation_loss,
                file_name,
                byte_size,
                manifest,
            } => match self
                .publish_checkpoint(
                    run_id,
                    run_directory,
                    request,
                    settings,
                    source,
                    step,
                    created_ms,
                    validation_loss,
                    &file_name,
                    byte_size,
                    &manifest,
                )
                .await
            {
                Ok(()) => None,
                Err(error) => Some(WorkerTerminal::Failed(error)),
            },
            WorkerEvent::Log { message } => {
                self.push_log(run_id, message);
                None
            }
            WorkerEvent::Completed { message } => Some(WorkerTerminal::Completed(message)),
            WorkerEvent::Stopped { message } => Some(WorkerTerminal::Stopped(message)),
            WorkerEvent::Failed { message } => {
                self.push_log(run_id, format!("fatal · {message}"));
                Some(WorkerTerminal::Failed(message))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_checkpoint(
        &self,
        run_id: &str,
        run_directory: &Path,
        request: &StartTrainingRequest,
        settings: &StoredSpiralSettings,
        source: &TrainingSource,
        step: u64,
        created_ms: i64,
        validation_loss: Option<f64>,
        file_name: &str,
        byte_size: u64,
        manifest: &serde_json::Value,
    ) -> Result<(), String> {
        if step == 0 || step > request.steps {
            return Err(format!(
                "the worker returned checkpoint step {step}, outside this run"
            ));
        }
        if validation_loss.is_some_and(|loss| !loss.is_finite()) {
            return Err("the worker returned a non-finite validation loss".to_owned());
        }
        let relative = Path::new(file_name);
        if relative.components().count() != 1
            || !matches!(relative.components().next(), Some(Component::Normal(_)))
        {
            return Err("the worker returned an invalid checkpoint path".to_owned());
        }
        let path = run_directory.join(relative);
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|error| format!("could not inspect checkpoint {file_name}: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != byte_size {
            return Err(format!(
                "checkpoint {file_name} is not the stable regular file the worker declared"
            ));
        }
        if byte_size > MAX_CHECKPOINT_BYTES {
            return Err(format!(
                "checkpoint {file_name} exceeds the 240 MiB single-artifact publication limit"
            ));
        }
        let payload = tokio::fs::read(&path)
            .await
            .map_err(|error| format!("could not read checkpoint {file_name}: {error}"))?;
        if payload.len() as u64 != byte_size {
            return Err(format!(
                "checkpoint {file_name} changed while it was being read"
            ));
        }
        let config_json =
            validate_checkpoint_artifact(file_name, &payload, manifest, run_id, step, request)?;
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
                    format: TrainingCheckpointFormat::SafeTensors,
                },
            },
            true,
        );
        if let Err(error) = tokio::fs::remove_file(&path).await {
            self.push_log(
                run_id,
                format!("checkpoint published, but local cleanup failed: {error}"),
            );
        }
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

async fn probe_worker(python: &Path, worker_script: &Path) -> Result<String, String> {
    if !worker_script.is_file() {
        return Err(format!(
            "The training worker is missing at {}.",
            worker_script.display()
        ));
    }
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(python)
            .arg(worker_script)
            .arg("--probe")
            .env("PYTHONNOUSERSITE", "1")
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "The training worker probe timed out after 10 seconds using {}.",
            python.display()
        )
    })?
    .map_err(|error| format!("Could not launch {}: {error}", python.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            format!(
                "The training worker probe failed using {}.",
                python.display()
            )
        } else {
            format!(
                "The training worker probe failed using {}: {detail}",
                python.display()
            )
        });
    }
    let detail = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(if detail.is_empty() {
        "PyTorch worker verified".to_owned()
    } else {
        detail
    })
}

async fn readiness(
    python_probe: &Result<String, String>,
    storage: &StorageManager,
) -> TrainingReadiness {
    let python_ready = python_probe.is_ok();
    let spiral = storage.training_source();
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
    let ready = python_ready && spiral_ready;
    let summary = match (python_probe, spiral, connection) {
        (Err(error), _, _) => error.clone(),
        (_, Err(error), _) => error,
        (_, _, Some(result)) if !result.ok => {
            format!("Spiral is not ready for training: {}", result.message)
        }
        (Ok(probe), Ok((_, source)), Some(_)) => format!(
            "Runtime ready · {probe} · Spiral project {} · dataset {} · query checked at run start",
            source.project, source.dataset
        ),
        _ => "Spiral readiness could not be determined.".to_owned(),
    };
    TrainingReadiness {
        ready,
        python_ready,
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
        return Err("the training worker must publish a .safetensors checkpoint".to_owned());
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
            "checkpoint {file_name} metadata does not match the worker event"
        ));
    }
    if manifest.get("format").and_then(serde_json::Value::as_str)
        != Some("spiral_robotics.act.safetensors.v2")
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
        != Some("lerobot_act_state_dict")
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
    if request.act.camera_streams.is_empty() || request.act.camera_streams.len() > 8 {
        return Err("ACT requires between one and eight camera streams.".to_owned());
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
    if !(32..=1_024).contains(&request.act.image_width)
        || !(32..=1_024).contains(&request.act.image_height)
    {
        return Err("ACT image width and height must be between 32 and 1,024.".to_owned());
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
    fn default_vision_act_request_matches_recorded_camera_stream_ids() {
        let request = StartTrainingRequest {
            sql: "SELECT 1".to_owned(),
            ..StartTrainingRequest::default()
        };

        validate_request(&request).unwrap();
        assert_eq!(request.act.camera_streams, ["follower.camera.gripper"]);
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
    fn checkpoint_paths_must_be_direct_children() {
        let safe = Path::new("checkpoint-10.safetensors");
        let unsafe_path = Path::new("../checkpoint-10.safetensors");
        assert_eq!(safe.components().count(), 1);
        assert_ne!(unsafe_path.components().count(), 1);
    }

    #[test]
    fn checkpoint_publication_accepts_only_matching_safetensors_artifacts() {
        use std::collections::HashMap;

        use safetensors::tensor::{Dtype, TensorView, serialize};

        let request = StartTrainingRequest::default();
        let manifest = serde_json::json!({
            "format": "spiral_robotics.act.safetensors.v2",
            "artifact": "inference",
            "run_id": "train-1",
            "step": 10,
            "request": request,
            "model_config": {"type": "act"},
            "tensor_namespace": "lerobot_act_state_dict",
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
