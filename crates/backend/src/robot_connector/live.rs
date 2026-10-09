//! The live data a linked connector posts while someone watches it:
//! telemetry while a browser polls it, and each camera while an MJPEG
//! response shows it. The connector learns what is watched from its polls.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use spiral_robotics_common::robot_connector::LiveStream;
use tokio::sync::{Notify, watch};
use tokio::time::{Duration, Instant};

use crate::OperationError;
use crate::sync::MutexRecovery as _;

/// How long one telemetry read keeps telemetry watched. A page polls it
/// every 50 ms, so it stays watched while any page polls.
const TELEMETRY_LEASE: Duration = Duration::from_secs(3);
/// How long a read waits for the first sample when telemetry was unwatched.
const FIRST_SAMPLE_WAIT: Duration = Duration::from_secs(2);

/// The streams someone watches on one link, and the latest sample of each.
pub(super) struct LiveStreams {
    /// Wakes the link's waiting poll when the watched streams change.
    wakeup: Arc<Notify>,
    /// None once the link closed.
    state: Mutex<Option<LiveState>>,
}

struct LiveState {
    telemetry: TelemetryFeed,
    /// Each watched camera, by its ID.
    cameras: BTreeMap<String, CameraFeed>,
    /// The streams the connector last heard are watched.
    announced: Vec<LiveStream>,
}

/// Telemetry, watched until its lease runs out, and its latest sample.
struct TelemetryFeed {
    lease: Option<Instant>,
    latest: watch::Sender<Option<Bytes>>,
}

/// A camera's open MJPEG responses, and its latest frame.
struct CameraFeed {
    viewers: usize,
    latest: watch::Sender<Option<Bytes>>,
}

impl LiveStreams {
    pub(super) fn new(wakeup: Arc<Notify>) -> Arc<Self> {
        let state = LiveState {
            telemetry: TelemetryFeed {
                lease: None,
                latest: watch::Sender::new(None),
            },
            cameras: BTreeMap::new(),
            announced: Vec::new(),
        };
        Arc::new(Self {
            wakeup,
            state: Mutex::new(Some(state)),
        })
    }

    /// Every stream watched now, which the connector hears with this.
    pub(super) fn announce(&self) -> Vec<LiveStream> {
        let mut state = self.state.recover_lock();
        let Some(state) = state.as_mut() else {
            return Vec::new();
        };
        state.announced = state.watched();
        state.announced.clone()
    }

    /// Whether the watched streams changed since the connector last heard.
    pub(super) fn unannounced(&self) -> bool {
        self.state
            .recover_lock()
            .as_ref()
            .is_some_and(|state| state.watched() != state.announced)
    }

    /// The latest telemetry, watched for another lease. When nobody watched
    /// it, the first sample is a moment away.
    pub(super) async fn telemetry(self: &Arc<Self>) -> Result<Bytes, OperationError> {
        let mut samples = {
            let mut state = self.state.recover_lock();
            let state = state.as_mut().ok_or_else(Self::disconnected)?;
            let unwatched = state.telemetry.lease.is_none();
            state.telemetry.lease = Some(Instant::now() + TELEMETRY_LEASE);
            if unwatched {
                self.wakeup.notify_waiters();
                tokio::spawn(self.clone().end_telemetry_lease());
            }
            state.telemetry.latest.subscribe()
        };
        match tokio::time::timeout(FIRST_SAMPLE_WAIT, samples.wait_for(Option::is_some)).await {
            Ok(Ok(sample)) => Ok(sample.clone().expect("a sample arrived")),
            Ok(Err(_)) => Err(Self::disconnected()),
            Err(_) => Err(OperationError::Unavailable(
                "The robot connector sent no telemetry.".to_owned(),
            )),
        }
    }

    /// Stop watching telemetry once its lease runs out unrenewed, and forget
    /// its last sample, so the next read waits for a fresh one.
    async fn end_telemetry_lease(self: Arc<Self>) {
        loop {
            let lease = self
                .state
                .recover_lock()
                .as_ref()
                .and_then(|state| state.telemetry.lease);
            let Some(lease) = lease else {
                return;
            };
            tokio::time::sleep_until(lease).await;
            let mut state = self.state.recover_lock();
            let Some(state) = state.as_mut() else {
                return;
            };
            if state
                .telemetry
                .lease
                .is_some_and(|lease| lease <= Instant::now())
            {
                state.telemetry.lease = None;
                state.telemetry.latest.send_replace(None);
                self.wakeup.notify_waiters();
                return;
            }
        }
    }

    /// Keep `sample` as the latest telemetry, while someone watches it.
    pub(super) fn publish_telemetry(&self, sample: Bytes) {
        if let Some(state) = self.state.recover_lock().as_ref()
            && state.telemetry.lease.is_some()
        {
            state.telemetry.latest.send_replace(Some(sample));
        }
    }

    /// Keep `frame` as `camera`'s latest, while someone views it. A frame
    /// like the last one is no new frame.
    pub(super) fn publish_frame(&self, camera: &str, frame: Bytes) {
        if let Some(feed) = self
            .state
            .recover_lock()
            .as_ref()
            .and_then(|state| state.cameras.get(camera))
        {
            feed.latest.send_if_modified(|latest| {
                let fresh = latest.as_ref() != Some(&frame);
                if fresh {
                    *latest = Some(frame);
                }
                fresh
            });
        }
    }

    /// `camera`'s frames, watched while the returned viewer lasts.
    pub(super) fn view_camera(
        self: &Arc<Self>,
        camera: String,
    ) -> Result<CameraViewer, OperationError> {
        let mut state = self.state.recover_lock();
        let state = state.as_mut().ok_or_else(Self::disconnected)?;
        let feed = state
            .cameras
            .entry(camera.clone())
            .or_insert_with(|| CameraFeed {
                viewers: 0,
                latest: watch::Sender::new(None),
            });
        feed.viewers += 1;
        let mut frames = feed.latest.subscribe();
        // A frame another viewer already shows, this one shows at once.
        frames.mark_changed();
        if feed.viewers == 1 {
            self.wakeup.notify_waiters();
        }
        Ok(CameraViewer {
            live: self.clone(),
            camera,
            frames,
        })
    }

    fn leave_camera(&self, camera: &str) {
        let mut state = self.state.recover_lock();
        let Some(state) = state.as_mut() else {
            return;
        };
        let Some(feed) = state.cameras.get_mut(camera) else {
            return;
        };
        feed.viewers -= 1;
        if feed.viewers == 0 {
            state.cameras.remove(camera);
            self.wakeup.notify_waiters();
        }
    }

    /// Forget every stream: the link closed, so its viewers and readers stop.
    pub(super) fn close(&self) {
        self.state.recover_lock().take();
    }

    fn disconnected() -> OperationError {
        OperationError::Unavailable("The robot connector was disconnected.".to_owned())
    }
}

impl LiveState {
    fn watched(&self) -> Vec<LiveStream> {
        self.telemetry
            .lease
            .map(|_| LiveStream::Telemetry)
            .into_iter()
            .chain(
                self.cameras
                    .keys()
                    .map(|id| LiveStream::Camera { id: id.clone() }),
            )
            .collect()
    }
}

/// One open MJPEG response's view of a camera.
pub(super) struct CameraViewer {
    live: Arc<LiveStreams>,
    camera: String,
    frames: watch::Receiver<Option<Bytes>>,
}

impl CameraViewer {
    /// The next frame this viewer has not shown; none once the link closes.
    pub(super) async fn next_frame(&mut self) -> Option<Bytes> {
        loop {
            self.frames.changed().await.ok()?;
            if let Some(frame) = self.frames.borrow_and_update().clone() {
                return Some(frame);
            }
        }
    }
}

impl Drop for CameraViewer {
    fn drop(&mut self) {
        self.live.leave_camera(&self.camera);
    }
}
