//! Live camera streams owned by the robotics backend.
//!
//! Each open camera gets one thread that owns the device and publishes the
//! latest encoded frame to a [`watch`] channel. Watch semantics are exactly
//! right for video preview: a slow browser silently misses frames instead of
//! building an unbounded backlog.
//!
//! Streams start on the first viewer and stop themselves once nobody has been
//! watching for [`IDLE_TIMEOUT`], so leaving the cameras page does not leave a
//! webcam running for the rest of the session.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rerobot::cameras::{self, DEFAULT_JPEG_QUALITY, DetectedCamera};
use rerobot_core::api::CameraStatus;
use tokio::sync::watch;

/// How long a stream runs with nobody watching before it releases the camera.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// A healthy capture loop observes `stop` between frames. A driver that blocks
/// forever inside `frame()` must not make the whole studio ignore Ctrl+C.
const STREAM_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// One encoded frame, ready to be written to an HTTP response.
#[derive(Clone, Debug)]
pub struct Frame {
    pub bytes: Bytes,
    pub sequence: u64,
}

#[derive(Debug, Default)]
struct StreamStatus {
    running: bool,
    frames: u64,
    error: Option<String>,
}

struct Stream {
    stop: Arc<AtomicBool>,
    frames: Arc<watch::Sender<Option<Arc<Frame>>>>,
    status: Arc<Mutex<StreamStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl Stream {
    fn is_running(&self) -> bool {
        lock(&self.status).running
    }

    /// Ask the capture thread to stop and wait for it to release the device.
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let deadline = Instant::now() + STREAM_SHUTDOWN_GRACE;
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                // Dropping a JoinHandle detaches the thread. During process
                // shutdown the OS releases the camera; during a normal close
                // this is still preferable to freezing the server forever on
                // a misbehaving platform driver.
                tracing::warn!("camera capture thread did not stop; detaching it");
            }
        }
    }
}

#[derive(Default)]
struct ManagerState {
    discovered: BTreeMap<String, DetectedCamera>,
    /// Problems found by the most recent discovery pass.
    issues: Vec<String>,
    /// Problems that outlive discovery, such as a denied permission prompt.
    persistent_issues: Vec<String>,
    streams: BTreeMap<String, Stream>,
}

/// Discovers cameras and owns their capture threads.
#[derive(Default)]
pub struct CameraManager {
    state: Mutex<ManagerState>,
}

impl CameraManager {
    /// Create an empty manager; call [`CameraManager::refresh`] to populate it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-probe every camera the operating system reports.
    ///
    /// Probing spawns short-lived child processes, so this must not run on an
    /// async runtime thread.
    pub fn refresh_blocking(&self) {
        let (discovered, issues) = match cameras::discover() {
            Ok(outcome) => {
                let mut issues = Vec::new();
                for node in outcome.skipped {
                    issues.push(format!("skipped unsupported video node {node}"));
                }
                for failure in outcome.failures {
                    issues.push(format!("camera unavailable: {failure}"));
                }
                (
                    outcome
                        .cameras
                        .into_iter()
                        .map(|camera| (camera.identifier(), camera))
                        .collect::<BTreeMap<_, _>>(),
                    issues,
                )
            }
            Err(error) => (
                BTreeMap::new(),
                vec![format!("camera discovery failed: {error}")],
            ),
        };

        self.apply_discovery(discovered, issues);
    }

    /// Adopt the result of a discovery pass.
    fn apply_discovery(&self, discovered: BTreeMap<String, DetectedCamera>, issues: Vec<String>) {
        // Streams for cameras that vanished must not keep holding a device.
        let stale = {
            let mut state = lock(&self.state);
            state.issues = issues;
            let stale = state
                .streams
                .keys()
                .filter(|id| !discovered.contains_key(*id))
                .cloned()
                .collect::<Vec<_>>();
            state.discovered = discovered;
            stale
        };
        for id in stale {
            self.close(&id);
        }
    }

    /// Cameras found by the last refresh, with their current stream state.
    pub fn statuses(&self) -> Vec<CameraStatus> {
        let state = lock(&self.state);
        state
            .discovered
            .values()
            .map(|camera| {
                let id = camera.identifier();
                let (streaming, frames, error) = match state.streams.get(&id) {
                    Some(stream) => {
                        let status = lock(&stream.status);
                        (status.running, status.frames, status.error.clone())
                    }
                    None => (false, 0, None),
                };
                CameraStatus {
                    id,
                    name: camera.name(),
                    width: camera.width(),
                    height: camera.height(),
                    fps: camera.fps(),
                    streaming,
                    frames,
                    error,
                }
            })
            .collect()
    }

    /// Record a problem that discovery cannot detect on its own.
    ///
    /// A denied permission prompt is the motivating case: cameras still
    /// enumerate and their streams still open, but no frame ever arrives, so
    /// without this the operator would see an empty preview and no reason.
    pub fn note_issue(&self, message: impl Into<String>) {
        lock(&self.state).persistent_issues.push(message.into());
    }

    /// Non-fatal problems worth showing the operator.
    pub fn issues(&self) -> Vec<String> {
        let state = lock(&self.state);
        let mut issues = state.persistent_issues.clone();
        issues.extend(state.issues.iter().cloned());
        issues
    }

    /// Subscribe to a camera's frames, starting its capture thread if needed.
    pub fn subscribe(&self, id: &str) -> Result<watch::Receiver<Option<Arc<Frame>>>, String> {
        let mut state = lock(&self.state);
        if let Some(stream) = state.streams.get_mut(id) {
            if stream.is_running() {
                return Ok(stream.frames.subscribe());
            }
            // The previous attempt ended; reap it so a retry starts clean.
            stream.shutdown();
            state.streams.remove(id);
        }

        let camera = state
            .discovered
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no camera with id {id:?}; refresh the camera list"))?;
        let stream = spawn_stream(camera);
        let frames = stream.frames.subscribe();
        state.streams.insert(id.to_owned(), stream);
        Ok(frames)
    }

    /// Stop a camera's capture thread and release the device.
    pub fn close(&self, id: &str) {
        let stream = lock(&self.state).streams.remove(id);
        if let Some(mut stream) = stream {
            stream.shutdown();
        }
    }

    /// Stop every capture thread.
    pub fn shutdown(&self) {
        let streams = std::mem::take(&mut lock(&self.state).streams);
        for (_, mut stream) in streams {
            stream.shutdown();
        }
    }
}

impl Drop for CameraManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_stream(camera: DetectedCamera) -> Stream {
    let stop = Arc::new(AtomicBool::new(false));
    // Drop the initial receiver so `receiver_count` only counts real viewers.
    let (frames, initial) = watch::channel(None);
    drop(initial);
    let frames = Arc::new(frames);
    let status = Arc::new(Mutex::new(StreamStatus {
        running: true,
        frames: 0,
        error: None,
    }));

    let thread = {
        let (stop, frames, status) = (stop.clone(), frames.clone(), status.clone());
        thread::Builder::new()
            .name(format!("rerobot-camera-{}", camera.identifier()))
            .spawn(move || {
                let mut idle_since: Option<Instant> = None;
                let result =
                    cameras::stream_jpeg_frames(&camera, &stop, DEFAULT_JPEG_QUALITY, |frame| {
                        let sequence = frame.sequence;
                        frames.send_replace(Some(Arc::new(Frame {
                            bytes: Bytes::from(frame.bytes),
                            sequence,
                        })));
                        lock(&status).frames = sequence;

                        if frames.receiver_count() == 0 {
                            let since = *idle_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= IDLE_TIMEOUT {
                                stop.store(true, Ordering::Relaxed);
                            }
                        } else {
                            idle_since = None;
                        }
                    });

                let mut status = lock(&status);
                status.running = false;
                status.error = result.err().map(|error| error.to_string());
            })
            .expect("a camera capture thread must start")
    };

    Stream {
        stop,
        frames,
        status,
        thread: Some(thread),
    }
}

/// Lock a mutex, treating poisoning as recoverable.
///
/// Every guarded value here is plain bookkeeping: a panic mid-update can leave
/// a frame count stale but cannot make it unsafe to read.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_manager_reports_nothing_and_refuses_unknown_cameras() {
        let manager = CameraManager::new();

        assert!(manager.statuses().is_empty());
        assert!(manager.issues().is_empty());
        let error = manager
            .subscribe("/dev/video0")
            .expect_err("an unknown camera cannot be subscribed to");
        assert!(error.contains("no camera with id"));
    }

    #[test]
    fn noted_issues_survive_a_discovery_pass() {
        let manager = CameraManager::new();
        manager.note_issue("camera access was denied");

        // Discovery replaces what it found last time, but a permission failure
        // it cannot re-detect has to stay visible.
        manager.apply_discovery(BTreeMap::new(), vec!["a node was skipped".to_owned()]);
        manager.apply_discovery(BTreeMap::new(), vec!["a different node".to_owned()]);

        let issues = manager.issues();
        assert_eq!(issues, ["camera access was denied", "a different node"]);
    }

    #[test]
    fn closing_an_unknown_camera_is_harmless() {
        let manager = CameraManager::new();
        manager.close("/dev/video9");
        manager.shutdown();
    }

    #[test]
    fn frames_replace_rather_than_queue() {
        // The preview contract: a viewer that falls behind sees the newest
        // frame, never a backlog of stale ones.
        let (sender, mut receiver) = watch::channel(None::<Arc<Frame>>);
        for sequence in 1..=5 {
            sender.send_replace(Some(Arc::new(Frame {
                bytes: Bytes::from_static(b"jpeg"),
                sequence,
            })));
        }

        let frame = receiver
            .borrow_and_update()
            .clone()
            .expect("a frame is available");
        assert_eq!(frame.sequence, 5);
    }

    #[test]
    fn a_stuck_camera_thread_cannot_block_shutdown_forever() {
        let stop = Arc::new(AtomicBool::new(false));
        let (frames, receiver) = watch::channel(None::<Arc<Frame>>);
        drop(receiver);
        let status = Arc::new(Mutex::new(StreamStatus::default()));
        let handle = thread::spawn(|| thread::sleep(STREAM_SHUTDOWN_GRACE * 4));
        let mut stream = Stream {
            stop: stop.clone(),
            frames: Arc::new(frames),
            status,
            thread: Some(handle),
        };

        let started = Instant::now();
        stream.shutdown();

        assert!(stop.load(Ordering::Relaxed));
        assert!(started.elapsed() < STREAM_SHUTDOWN_GRACE * 3);
    }
}
