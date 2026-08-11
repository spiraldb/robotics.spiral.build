//! Rust-owned supervision and data plane for local PyTorch training.
//!
//! Rust owns every consequential boundary: Spiral ADBC, query validation,
//! bounded prefetch, process lifecycle, checkpoint publication, persistence,
//! and browser events. Python receives framed Arrow IPC batches on stdin and
//! returns typed NDJSON metrics plus local checkpoint artifacts on stdout.

use std::collections::BTreeMap;
use std::fs;
use std::future::pending;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arrow_ipc::writer::StreamWriter;
use serde::{Deserialize, Serialize};
use spiral_robotics_core::api::{
    ConnectionTestResponse, MAX_TRAINING_HISTORY_RUNS, StartTrainingRequest, TrainingCheckpoint,
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingSnapshot,
    TrainingSource, TrainingUpdate,
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::backend::storage::manager::{
    StorageManager, TrainingCheckpointWrite, TrainingDataBatch, TrainingDataMessage,
};
use crate::backend::storage::settings::StoredSpiralSettings;
use crate::backend::storage::sql::{self, ReadOnlyMode};

const HISTORY_FILE: &str = "runs.json";
const DATA_QUEUE_DEPTH: usize = 3;
const STOP_GRACE: Duration = Duration::from_secs(10);
const MAX_CHECKPOINT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const FRAME_BATCH: u8 = 1;
const FRAME_ERROR: u8 = 2;
const FRAME_EPOCH_END: u8 = 3;
const FRAME_END: u8 = 4;
const FRAME_HEADER_BYTES: usize = 45;
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct FrameMetadata {
    epoch: u64,
    fetch_ns: u64,
    source_bytes: u64,
    loader_ns: u64,
    queue_depth: usize,
}

#[derive(Clone)]
pub struct TrainingManager {
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
    source: TrainingSource,
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
    pub async fn open(
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

    pub fn snapshot(&self) -> TrainingSnapshot {
        lock(&self.inner.snapshot).clone()
    }

    /// Capture the initial state and subscribe to later changes as one ordered
    /// operation. Publishers send while holding the same snapshot lock, so an
    /// update can appear in the snapshot or the receiver, never both.
    pub fn subscription(&self) -> (TrainingSnapshot, broadcast::Receiver<TrainingUpdate>) {
        let snapshot = lock(&self.inner.snapshot);
        let updates = self.inner.updates.subscribe();
        (snapshot.clone(), updates)
    }

    pub async fn refresh_readiness(&self) -> TrainingReadiness {
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

    pub async fn start(&self, request: StartTrainingRequest) -> Result<TrainingRun, String> {
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

    pub fn stop(&self, run_id: &str) -> Result<(), String> {
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

    pub async fn shutdown(&self) {
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
            source: source.clone(),
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
            .stdin(Stdio::piped())
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

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stop = Arc::new(AtomicBool::new(false));
        let data_task = tokio::spawn(stream_data_to_worker(
            self.inner.storage.clone(),
            settings.clone(),
            request.clone(),
            stop.clone(),
            stdin,
        ));
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
                    stop.store(true, Ordering::Release);
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
                                    stop.store(true, Ordering::Release);
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

        stop.store(true, Ordering::Release);
        let status = child.wait().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), data_task).await;
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
                "checkpoint {file_name} exceeds the 2 GiB publication limit"
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
        let policy = serde_json::to_value(request.policy)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "act".to_owned());
        let config_json = serde_json::to_string(request)
            .map_err(|error| format!("could not encode checkpoint configuration: {error}"))?;
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
    .map_err(|_| "The pinned PyTorch worker probe timed out after 10 seconds.".to_owned())?
    .map_err(|error| format!("Could not launch {}: {error}", python.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            "The pinned PyTorch worker probe failed.".to_owned()
        } else {
            format!("The pinned PyTorch worker probe failed: {detail}")
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

async fn stream_data_to_worker(
    storage: Arc<StorageManager>,
    settings: StoredSpiralSettings,
    request: StartTrainingRequest,
    stop: Arc<AtomicBool>,
    mut stdin: ChildStdin,
) {
    let (sender, mut receiver) = mpsc::channel(DATA_QUEUE_DEPTH);
    let loader = tokio::spawn({
        let stop = stop.clone();
        async move {
            storage
                .stream_training_data(
                    settings,
                    request.sql,
                    request.state_column,
                    request.action_column,
                    stop,
                    sender,
                )
                .await
        }
    });

    while let Some(message) = receiver.recv().await {
        let result = match message {
            TrainingDataMessage::Batch(batch) => write_batch_frame(&mut stdin, batch).await,
            TrainingDataMessage::EpochEnd { epoch } => {
                write_frame(
                    &mut stdin,
                    FRAME_EPOCH_END,
                    &[],
                    FrameMetadata {
                        epoch,
                        ..FrameMetadata::default()
                    },
                )
                .await
            }
        };
        if result.is_err() {
            stop.store(true, Ordering::Release);
            return;
        }
    }
    match loader.await {
        Ok(Ok(())) => {
            let _ = write_frame(&mut stdin, FRAME_END, &[], FrameMetadata::default()).await;
        }
        Ok(Err(error)) => {
            let bytes = error.as_bytes();
            let _ = write_frame(&mut stdin, FRAME_ERROR, bytes, FrameMetadata::default()).await;
        }
        Err(error) => {
            let message = format!("Spiral loader task failed: {error}");
            let _ = write_frame(
                &mut stdin,
                FRAME_ERROR,
                message.as_bytes(),
                FrameMetadata::default(),
            )
            .await;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn write_batch_frame(stdin: &mut ChildStdin, batch: TrainingDataBatch) -> Result<(), String> {
    let mut payload = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut payload, batch.batch.schema().as_ref())
            .map_err(|error| format!("opening the Arrow worker stream: {error}"))?;
        writer
            .write(&batch.batch)
            .and_then(|()| writer.finish())
            .map_err(|error| format!("encoding an Arrow worker batch: {error}"))?;
    }
    let fetch_ns = (batch.fetch_seconds.max(0.0) * 1_000_000_000.0)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64;
    let loader_ns = (batch.loader_seconds.max(0.0) * 1_000_000_000.0)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64;
    write_frame(
        stdin,
        FRAME_BATCH,
        &payload,
        FrameMetadata {
            epoch: batch.epoch,
            fetch_ns,
            source_bytes: batch.source_bytes,
            loader_ns,
            queue_depth: batch.queue_depth,
        },
    )
    .await
}

async fn write_frame(
    stdin: &mut ChildStdin,
    kind: u8,
    payload: &[u8],
    metadata: FrameMetadata,
) -> Result<(), String> {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    header[0] = kind;
    header[1..9].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    header[9..17].copy_from_slice(&metadata.epoch.to_le_bytes());
    header[17..25].copy_from_slice(&metadata.fetch_ns.to_le_bytes());
    header[25..33].copy_from_slice(&metadata.source_bytes.to_le_bytes());
    header[33..41].copy_from_slice(&metadata.loader_ns.to_le_bytes());
    header[41..45]
        .copy_from_slice(&(metadata.queue_depth.min(u32::MAX as usize) as u32).to_le_bytes());
    stdin
        .write_all(&header)
        .await
        .map_err(|error| format!("writing the worker frame header: {error}"))?;
    stdin
        .write_all(payload)
        .await
        .map_err(|error| format!("writing the worker frame payload: {error}"))
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
        if column.is_empty()
            || !column
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!("{label} must be a SQL identifier."));
        }
    }
    if !(1..=10_000_000).contains(&request.steps) {
        return Err("Training steps must be between 1 and 10,000,000.".to_owned());
    }
    if !(1..=8_192).contains(&request.batch_size) {
        return Err("Batch size must be between 1 and 8,192.".to_owned());
    }
    if !(1..=256).contains(&request.chunk_size) {
        return Err("Action chunk size must be between 1 and 256.".to_owned());
    }
    if request.checkpoint_every == 0 || request.checkpoint_every > request.steps {
        return Err("Checkpoint cadence must be between 1 and the run step count.".to_owned());
    }
    if !request.learning_rate.is_finite() || request.learning_rate <= 0.0 {
        return Err("Learning rate must be a finite positive number.".to_owned());
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
    fn checkpoint_paths_must_be_direct_children() {
        let safe = Path::new("checkpoint-10.pt");
        let unsafe_path = Path::new("../checkpoint-10.pt");
        assert_eq!(safe.components().count(), 1);
        assert_ne!(unsafe_path.components().count(), 1);
    }
}
