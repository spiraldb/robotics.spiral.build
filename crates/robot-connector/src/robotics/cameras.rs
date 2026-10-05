//! Live camera streams owned by the connector.
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
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use spiral_robotics_common::robots::{CameraStatus, CameraStreamFormat, CameraStreamState};
use spiral_robotics_hardware::{
    DEFAULT_JPEG_QUALITY, DetectedCamera, JpegStreamConfig, ProbeOutcome,
};
use tokio::sync::watch;

use crate::simulation::{SimulatedCamera, SimulatedWorld};
use crate::sync::MutexRecovery;

/// How long a stream runs with nobody watching before it releases the camera.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// A healthy capture loop observes `stop` between frames. A driver that blocks
/// forever inside `frame()` must not make the whole studio ignore Ctrl+C.
const STREAM_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// One encoded frame, ready to be written to an HTTP response.
#[derive(Clone, Debug)]
pub(crate) struct Frame {
    bytes: Bytes,
    sequence: u64,
    captured_at: Instant,
}

impl Frame {
    #[cfg(test)]
    pub(crate) fn new(bytes: Bytes, sequence: u64, captured_at: Instant) -> Self {
        Self {
            bytes,
            sequence,
            captured_at,
        }
    }

    pub(crate) fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) const fn captured_at(&self) -> Instant {
        self.captured_at
    }
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
    fn spawn(id: &str, source: CameraSource) -> Self {
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
                .name(format!("spiral-robotics-camera-{id}"))
                .spawn(move || {
                    let mut idle_since: Option<Instant> = None;
                    let result = source.stream(&stop, |bytes, sequence| {
                        frames.send_replace(Some(Arc::new(Frame {
                            bytes: Bytes::from(bytes),
                            sequence,
                            captured_at: Instant::now(),
                        })));
                        status.recover_lock().frames = sequence;

                        if frames.receiver_count() == 0 {
                            let since = *idle_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= IDLE_TIMEOUT {
                                stop.store(true, Ordering::Relaxed);
                            }
                        } else {
                            idle_since = None;
                        }
                    });

                    let mut status = status.recover_lock();
                    status.running = false;
                    status.error = result.err();
                })
                .expect("a camera capture thread must start")
        };

        Self {
            stop,
            frames,
            status,
            thread: Some(thread),
        }
    }

    fn is_running(&self) -> bool {
        self.status.recover_lock().running
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

/// Where a stream's frames come from: a device, or a drawn simulated camera.
enum CameraSource {
    Device(DetectedCamera),
    Simulated(SimulatedCamera),
}

impl CameraSource {
    /// Stream JPEG frames to `publish` until `stop` is set.
    fn stream(
        &self,
        stop: &AtomicBool,
        mut publish: impl FnMut(Vec<u8>, u64),
    ) -> Result<(), String> {
        match self {
            Self::Device(camera) => camera
                .stream_jpeg_frames(JpegStreamConfig::new(stop, DEFAULT_JPEG_QUALITY), |frame| {
                    let sequence = frame.sequence();
                    publish(frame.into_bytes(), sequence);
                })
                .map_err(|error| error.to_string()),
            Self::Simulated(camera) => camera.stream_jpeg_frames(stop, publish),
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

/// Discovers cameras, lists the simulated ones, and owns their capture threads.
#[derive(Default)]
pub(crate) struct CameraManager {
    state: Mutex<ManagerState>,
    simulation: Option<SimulatedWorld>,
}

impl CameraManager {
    pub(crate) async fn refresh(
        self: &Arc<Self>,
    ) -> Result<Vec<CameraStatus>, crate::OperationError> {
        let cameras = self.clone();
        tokio::task::spawn_blocking(move || cameras.refresh_blocking())
            .await
            .map_err(|error| crate::OperationError::Internal(error.to_string()))?;
        Ok(self.statuses())
    }

    /// A manager that films `simulation`'s mounted cameras beside the
    /// devices [`CameraManager::refresh_blocking`] finds.
    pub(crate) fn new(simulation: SimulatedWorld) -> Self {
        Self {
            state: Mutex::default(),
            simulation: Some(simulation),
        }
    }

    fn simulated_cameras(&self) -> Vec<SimulatedCamera> {
        self.simulation
            .as_ref()
            .map(SimulatedWorld::cameras)
            .unwrap_or_default()
    }

    /// Re-probe every camera the operating system reports.
    ///
    /// Probing spawns short-lived child processes, so this must not run on an
    /// async runtime thread.
    pub(crate) fn refresh_blocking(&self) {
        let (discovered, issues) = match ProbeOutcome::discover() {
            Ok(outcome) => {
                let (cameras, skipped, failures) = outcome.into_parts();
                let mut issues = Vec::new();
                for node in skipped {
                    issues.push(format!("skipped unsupported video node {node}"));
                }
                for failure in failures {
                    issues.push(format!("camera unavailable: {failure}"));
                }
                (
                    cameras
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
        let simulated = self.simulated_cameras();
        let stale = {
            let mut state = self.state.recover_lock();
            state.issues = issues;
            let stale = state
                .streams
                .keys()
                .filter(|id| {
                    !discovered.contains_key(*id)
                        && !simulated.iter().any(|camera| camera.id() == id.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            state.discovered = discovered;
            stale
        };
        for id in stale {
            self.close(&id);
        }
    }

    /// Cameras found by the last refresh and simulated ones, with their
    /// current stream state.
    pub(crate) fn statuses(&self) -> Vec<CameraStatus> {
        let simulated = self.simulated_cameras();
        let state = self.state.recover_lock();
        let devices = state.discovered.values().map(|camera| {
            CameraStatus::new(
                camera.identifier(),
                camera.name(),
                CameraStreamFormat::new(camera.width(), camera.height(), camera.fps()),
            )
        });
        devices
            .chain(simulated.iter().map(SimulatedCamera::status))
            .map(|status| {
                let stream_state = state.streams.get(status.id()).map_or_else(
                    CameraStreamState::default,
                    |stream| {
                        let stream = stream.status.recover_lock();
                        CameraStreamState::new(stream.running, stream.frames, stream.error.clone())
                    },
                );
                status.with_state(stream_state)
            })
            .collect()
    }

    /// Record a problem that discovery cannot detect on its own.
    ///
    /// A denied permission prompt is the motivating case: cameras still
    /// enumerate and their streams still open, but no frame ever arrives, so
    /// without this the operator would see an empty preview and no reason.
    pub(crate) fn note_issue(&self, message: impl Into<String>) {
        self.state
            .recover_lock()
            .persistent_issues
            .push(message.into());
    }

    /// Non-fatal problems worth showing the operator.
    pub(crate) fn issues(&self) -> Vec<String> {
        let state = self.state.recover_lock();
        let mut issues = state.persistent_issues.clone();
        issues.extend(state.issues.iter().cloned());
        issues
    }

    /// Subscribe to a camera's frames, starting its capture thread if needed.
    pub(crate) fn subscribe(
        &self,
        id: &str,
    ) -> Result<watch::Receiver<Option<Arc<Frame>>>, String> {
        let mut state = self.state.recover_lock();
        if let Some(stream) = state.streams.get_mut(id) {
            if stream.is_running() {
                return Ok(stream.frames.subscribe());
            }
            // The previous attempt ended; reap it so a retry starts clean.
            stream.shutdown();
            state.streams.remove(id);
        }

        let source = match state.discovered.get(id) {
            Some(camera) => CameraSource::Device(camera.clone()),
            None => self
                .simulated_cameras()
                .into_iter()
                .find(|camera| camera.id() == id)
                .map(CameraSource::Simulated)
                .ok_or_else(|| format!("no camera with id {id:?}; refresh the camera list"))?,
        };
        let stream = Stream::spawn(id, source);
        let frames = stream.frames.subscribe();
        state.streams.insert(id.to_owned(), stream);
        Ok(frames)
    }

    /// Stop a camera's capture thread and release the device.
    fn close(&self, id: &str) {
        let stream = self.state.recover_lock().streams.remove(id);
        if let Some(mut stream) = stream {
            stream.shutdown();
        }
    }

    /// Stop every capture thread.
    pub(crate) fn shutdown(&self) {
        let streams = std::mem::take(&mut self.state.recover_lock().streams);
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

/// Discover cameras once the HTTP listener is already accepting requests.
pub(crate) struct CameraDiscovery(Arc<CameraManager>);
impl CameraDiscovery {
    pub(crate) fn new(cameras: Arc<CameraManager>) -> Self {
        Self(cameras)
    }

    pub(crate) async fn run(self) {
        let cameras = self.0.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || cameras.refresh_blocking()).await {
            let message = format!("camera discovery task stopped: {error}");
            tracing::warn!("{message}");
            self.0.note_issue(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_manager_reports_nothing_and_refuses_unknown_cameras() {
        let manager = CameraManager::default();

        assert!(manager.statuses().is_empty());
        assert!(manager.issues().is_empty());
        let error = manager
            .subscribe("/dev/video0")
            .expect_err("an unknown camera cannot be subscribed to");
        assert!(error.contains("no camera with id"));
    }

    #[test]
    fn noted_issues_survive_a_discovery_pass() {
        let manager = CameraManager::default();
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
        let manager = CameraManager::default();
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
                captured_at: Instant::now(),
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
