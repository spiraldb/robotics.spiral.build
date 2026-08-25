//! Load-once recorded episode inspection and browser-local frame selection.
//!
//! A snapshot contains the complete visual joint timeline in a compact
//! columnar form. Studio caches a bounded number of immutable snapshots and
//! resolves preview playback and scrubbing in memory; only physical motion
//! continues through backend-owned validation and control.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use dioxus::prelude::*;
use spiral_robotics_api::recording::{
    EpisodeInspectionArm, EpisodeInspectionSnapshot, EpisodeOptionalFrameMatrix,
};
use spiral_robotics_core::arm::SO101_JOINTS;

use crate::client::http as api;
use crate::panels::timeline::panel::{
    TimelineSignalLane, TimelineSignalPoint, TimelineSignalSeries,
};
use crate::panels::timeline::signals::signal_color;
use crate::shell::activity::{LoadState, RecordedSource};
use crate::shell::state::Studio;

const HISTORICAL_RENDER_HZ: f64 = 30.0;
const RECORDED_REQUEST_TIMEOUT_MS: u32 = 15_000;
const TRACE_BUCKETS: usize = 240;
const MAX_CACHED_EPISODES: usize = 4;

pub(crate) fn historical_replay_slot(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * HISTORICAL_RENDER_HZ).floor() as u64
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoricalPreview {
    episode: String,
    timeline: Arc<api::EpisodeTimelineResponse>,
    sample: api::TimelineSample,
    signal_lanes: Arc<[TimelineSignalLane; 6]>,
}

impl HistoricalPreview {
    fn from_inspection(inspection: &CachedInspection, sample: api::TimelineSample) -> Self {
        Self {
            episode: inspection.snapshot.episode.id.clone(),
            timeline: Arc::clone(&inspection.timeline),
            sample,
            signal_lanes: Arc::clone(&inspection.signal_lanes),
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        episode: String,
        timeline: Arc<api::EpisodeTimelineResponse>,
        render: (api::TimelineSample, Arc<[TimelineSignalLane; 6]>),
    ) -> Self {
        let (sample, signal_lanes) = render;
        Self {
            episode,
            timeline,
            sample,
            signal_lanes,
        }
    }

    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }

    pub(crate) fn timeline(&self) -> &api::EpisodeTimelineResponse {
        &self.timeline
    }

    #[cfg(test)]
    pub(crate) const fn sample(&self) -> &api::TimelineSample {
        &self.sample
    }

    pub(crate) fn signal_lanes(&self) -> &Arc<[TimelineSignalLane; 6]> {
        &self.signal_lanes
    }

    pub(crate) fn into_sample(self) -> api::TimelineSample {
        self.sample
    }
}

#[derive(Clone, Debug)]
struct CachedInspection {
    snapshot: EpisodeInspectionSnapshot,
    timeline: Arc<api::EpisodeTimelineResponse>,
    signal_lanes: Arc<[TimelineSignalLane; 6]>,
}

impl CachedInspection {
    fn try_new(snapshot: EpisodeInspectionSnapshot) -> Result<Self, String> {
        validate_snapshot(&snapshot)?;
        let timeline = Arc::new(api::EpisodeTimelineResponse {
            episode: snapshot.episode.clone(),
            samples: api::TimelineMetadata {
                samples: snapshot.sample_times_seconds.len() as u64,
                first_seconds: snapshot.sample_times_seconds.first().copied(),
                last_seconds: snapshot.sample_times_seconds.last().copied(),
            },
            videos: snapshot.videos.clone(),
        });
        let signal_lanes = Arc::new(signal_lanes(&snapshot, TRACE_BUCKETS));
        Ok(Self {
            snapshot,
            timeline,
            signal_lanes,
        })
    }

    fn preview_at(&self, seconds: f64) -> Result<HistoricalPreview, String> {
        let sample = sample_at(&self.snapshot, seconds)?;
        Ok(HistoricalPreview::from_inspection(self, sample))
    }
}

/// Small FIFO cache bounded independently of the total catalog size. A new
/// persisted frame count replaces an older snapshot with the same episode ID.
#[derive(Clone, Debug, Default)]
pub(crate) struct EpisodeInspectionCache {
    entries: VecDeque<Arc<CachedInspection>>,
}

impl EpisodeInspectionCache {
    fn get(&self, episode: &str, expected_frames: Option<u64>) -> Option<Arc<CachedInspection>> {
        self.entries
            .iter()
            .find(|entry| {
                entry.snapshot.episode.id == episode
                    && expected_frames.is_none_or(|frames| entry.snapshot.episode.frames == frames)
            })
            .cloned()
    }

    fn insert(&mut self, inspection: Arc<CachedInspection>) {
        self.entries
            .retain(|entry| entry.snapshot.episode.id != inspection.snapshot.episode.id);
        self.entries.push_front(inspection);
        self.entries.truncate(MAX_CACHED_EPISODES);
    }

    pub(crate) fn invalidate(&mut self, episode: &str) {
        self.entries
            .retain(|entry| entry.snapshot.episode.id != episode);
    }
}

pub(crate) struct HistoricalPreviewRequest {
    episode: String,
    expected_frames: Option<u64>,
    seconds: f64,
}

impl HistoricalPreviewRequest {
    pub(crate) fn new(episode: String, expected_frames: Option<u64>, seconds: f64) -> Self {
        Self {
            episode,
            expected_frames,
            seconds,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HistoricalPreviewSignals {
    state: Signal<Option<RecordedSource<HistoricalPreview>>>,
    revision: Signal<u64>,
    cache: Signal<EpisodeInspectionCache>,
}

impl HistoricalPreviewSignals {
    pub(crate) const fn new(
        state: Signal<Option<RecordedSource<HistoricalPreview>>>,
        revision: Signal<u64>,
        cache: Signal<EpisodeInspectionCache>,
    ) -> Self {
        Self {
            state,
            revision,
            cache,
        }
    }

    pub(crate) const fn into_signals(
        self,
    ) -> (
        Signal<Option<RecordedSource<HistoricalPreview>>>,
        Signal<u64>,
        Signal<EpisodeInspectionCache>,
    ) {
        (self.state, self.revision, self.cache)
    }
}

pub(crate) fn request_historical_preview(
    studio: Studio,
    request: HistoricalPreviewRequest,
    signals: HistoricalPreviewSignals,
) {
    let HistoricalPreviewRequest {
        episode,
        expected_frames,
        seconds,
    } = request;
    let HistoricalPreviewSignals {
        mut state,
        mut revision,
        mut cache,
    } = signals;
    // A controller clock may advance while the one snapshot request is in
    // flight. Record the newest requested time without starting another read;
    // the completion below samples that newest time from the loaded snapshot.
    let same_episode_loading = state.peek().as_ref().is_some_and(|source| {
        source.episode == episode && matches!(source.state, LoadState::Loading { .. })
    });
    if same_episode_loading && cache.peek().get(&episode, expected_frames).is_none() {
        if let Some(source) = state.write().as_mut() {
            source.seconds = seconds;
        }
        return;
    }

    let ticket = revision.peek().wrapping_add(1);
    revision.set(ticket);
    if let Some(inspection) = cache.peek().get(&episode, expected_frames) {
        match inspection.preview_at(seconds) {
            Ok(preview) => state.set(Some(RecordedSource {
                episode,
                seconds,
                state: LoadState::Ready(preview),
            })),
            Err(error) => {
                state.set(Some(RecordedSource {
                    episode,
                    seconds,
                    state: LoadState::Failed {
                        reason: error.clone(),
                        previous: None,
                    },
                }));
                studio.report(error);
            }
        }
        return;
    }

    let previous = state
        .peek()
        .as_ref()
        .filter(|source| source.episode == episode)
        .and_then(RecordedSource::renderable)
        .cloned();
    state.set(Some(RecordedSource {
        episode: episode.clone(),
        seconds,
        state: LoadState::Loading {
            previous: previous.clone(),
        },
    }));
    spawn(async move {
        let path = api::episode_inspection_url(&episode);
        let loaded = with_deadline(
            "The episode inspection snapshot",
            RECORDED_REQUEST_TIMEOUT_MS,
            api::get::<api::EpisodeInspectionSnapshot>(&path),
        )
        .await
        .and_then(CachedInspection::try_new)
        .map(Arc::new);
        if !revision.peek().eq(&ticket) {
            return;
        }
        match loaded {
            Ok(inspection) => {
                if expected_frames
                    .is_some_and(|frames| inspection.snapshot.episode.frames != frames)
                {
                    let error = format!(
                        "The episode changed while it was loading (expected {expected} frames, received {actual}). Reload it to inspect the latest revision.",
                        expected = expected_frames.unwrap_or_default(),
                        actual = inspection.snapshot.episode.frames,
                    );
                    state.set(Some(RecordedSource {
                        episode,
                        seconds,
                        state: LoadState::Failed {
                            reason: error.clone(),
                            previous,
                        },
                    }));
                    studio.report(error);
                    return;
                }
                let requested_seconds = state
                    .peek()
                    .as_ref()
                    .filter(|source| source.episode == episode)
                    .map_or(seconds, |source| source.seconds);
                match inspection.preview_at(requested_seconds) {
                    Ok(preview) => {
                        cache.write().insert(Arc::clone(&inspection));
                        state.set(Some(RecordedSource {
                            episode,
                            seconds: requested_seconds,
                            state: LoadState::Ready(preview),
                        }));
                    }
                    Err(error) => {
                        state.set(Some(RecordedSource {
                            episode,
                            seconds: requested_seconds,
                            state: LoadState::Failed {
                                reason: error.clone(),
                                previous,
                            },
                        }));
                        studio.report(error);
                    }
                }
            }
            Err(error) => {
                state.set(Some(RecordedSource {
                    episode,
                    seconds,
                    state: LoadState::Failed {
                        reason: error.clone(),
                        previous,
                    },
                }));
                studio.report(error);
            }
        }
    });
}

fn validate_snapshot(snapshot: &EpisodeInspectionSnapshot) -> Result<(), String> {
    let frames = snapshot.sample_times_seconds.len();
    if frames == 0 {
        return Err(format!(
            "Episode {:?} has no visual samples.",
            snapshot.episode.id
        ));
    }
    if snapshot.episode.frames != frames as u64 {
        return Err(format!(
            "Episode {:?} declares {} frames but delivered {frames} visual samples.",
            snapshot.episode.id, snapshot.episode.frames
        ));
    }
    if snapshot.follow_error_degrees.len() != frames {
        return Err(format!(
            "Episode {:?} delivered an inconsistent follow-error track.",
            snapshot.episode.id
        ));
    }
    let mut previous = None;
    for seconds in &snapshot.sample_times_seconds {
        if !seconds.is_finite() || *seconds < 0.0 || previous.is_some_and(|last| *seconds < last) {
            return Err(format!(
                "Episode {:?} delivered an invalid visual sample clock.",
                snapshot.episode.id
            ));
        }
        previous = Some(*seconds);
    }
    if snapshot.arms.len() != snapshot.episode.arms.len() {
        return Err(format!(
            "Episode {:?} delivered an inconsistent arm layout.",
            snapshot.episode.id
        ));
    }
    for arm in &snapshot.arms {
        let Some(expected) = snapshot
            .episode
            .arms
            .iter()
            .find(|expected| expected.id == arm.arm)
        else {
            return Err(format!(
                "Episode {:?} delivered an unknown arm {:?}.",
                snapshot.episode.id, arm.arm
            ));
        };
        if arm.joints != expected.joints {
            return Err(format!(
                "Episode {:?} delivered a different joint layout for {:?}.",
                snapshot.episode.id, arm.arm
            ));
        }
        let values = frames
            .checked_mul(arm.joints.len())
            .ok_or_else(|| "Episode inspection dimensions overflowed.".to_owned())?;
        if arm.normalized.len() != values || arm.raw.len() != values {
            return Err(format!(
                "Episode {:?} delivered an incomplete state track for {:?}.",
                snapshot.episode.id, arm.arm
            ));
        }
        validate_optional_matrix(snapshot, arm, "velocity", &arm.velocity, frames, values)?;
        validate_optional_matrix(snapshot, arm, "effort", &arm.effort, frames, values)?;
        validate_optional_matrix(snapshot, arm, "action", &arm.action, frames, values)?;
    }
    Ok(())
}

fn validate_optional_matrix(
    snapshot: &EpisodeInspectionSnapshot,
    arm: &EpisodeInspectionArm,
    label: &str,
    matrix: &EpisodeOptionalFrameMatrix,
    frames: usize,
    values: usize,
) -> Result<(), String> {
    if matrix.present.len() != frames || matrix.values.len() != values {
        return Err(format!(
            "Episode {:?} delivered an incomplete {label} track for {:?}.",
            snapshot.episode.id, arm.arm
        ));
    }
    Ok(())
}

fn sample_at(
    snapshot: &EpisodeInspectionSnapshot,
    seconds: f64,
) -> Result<api::TimelineSample, String> {
    if !seconds.is_finite() {
        return Err("Timeline time must be finite.".to_owned());
    }
    let frame = nearest_frame(&snapshot.sample_times_seconds, seconds)
        .ok_or_else(|| format!("Episode {:?} has no visual samples.", snapshot.episode.id))?;
    let arms = snapshot
        .arms
        .iter()
        .map(|arm| sample_arm(arm, frame))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(api::TimelineSample {
        frame: frame as u64,
        seconds: snapshot.sample_times_seconds[frame],
        follow_error_degrees: snapshot.follow_error_degrees[frame],
        arms,
    })
}

fn nearest_frame(times: &[f64], requested: f64) -> Option<usize> {
    if times.is_empty() {
        return None;
    }
    let after = times.partition_point(|candidate| *candidate <= requested);
    match after {
        0 => Some(0),
        index if index >= times.len() => Some(times.len() - 1),
        index => {
            let before = index - 1;
            if (requested - times[before]).abs() <= (times[index] - requested).abs() {
                Some(before)
            } else {
                Some(index)
            }
        }
    }
}

fn sample_arm(arm: &EpisodeInspectionArm, frame: usize) -> Result<api::TimelineArmSample, String> {
    let joint_count = arm.joints.len();
    let start = frame
        .checked_mul(joint_count)
        .ok_or_else(|| "Episode inspection frame offset overflowed.".to_owned())?;
    let end = start + joint_count;
    let optional = |matrix: &EpisodeOptionalFrameMatrix| {
        matrix.present[frame].then(|| {
            arm.joints
                .iter()
                .cloned()
                .zip(matrix.values[start..end].iter().copied())
                .collect::<BTreeMap<_, _>>()
        })
    };
    Ok(api::TimelineArmSample {
        arm: arm.arm.clone(),
        normalized: arm
            .joints
            .iter()
            .cloned()
            .zip(arm.normalized[start..end].iter().copied())
            .collect(),
        raw: arm
            .joints
            .iter()
            .cloned()
            .zip(arm.raw[start..end].iter().copied())
            .collect(),
        velocity: optional(&arm.velocity),
        effort: optional(&arm.effort),
        action: optional(&arm.action),
    })
}

fn signal_lanes(snapshot: &EpisodeInspectionSnapshot, buckets: usize) -> [TimelineSignalLane; 6] {
    std::array::from_fn(|lane_index| {
        let joint = SO101_JOINTS[lane_index];
        let series = snapshot
            .arms
            .iter()
            .filter_map(|arm| {
                let joint_index = arm.joints.iter().position(|name| name == joint.name())?;
                let joint_count = arm.joints.len();
                let values = arm
                    .normalized
                    .chunks_exact(joint_count)
                    .map(|frame| frame[joint_index]);
                Some(TimelineSignalSeries {
                    arm: arm.arm.clone(),
                    points: reduce_trace(&snapshot.sample_times_seconds, values, buckets),
                })
            })
            .collect();
        TimelineSignalLane {
            joint: joint.name().to_owned(),
            label: joint.label().to_owned(),
            color: signal_color(joint.name()).to_owned(),
            series,
        }
    })
}

fn reduce_trace(
    times: &[f64],
    values: impl Iterator<Item = Option<f64>>,
    buckets: usize,
) -> Vec<TimelineSignalPoint> {
    let bucket_count = buckets.clamp(1, 512).min(times.len());
    if bucket_count == 0 {
        return Vec::new();
    }
    let start = times[0];
    let end = times[times.len() - 1];
    let mut reduced = vec![TraceBucket::default(); bucket_count];
    for (seconds, value) in times.iter().copied().zip(values) {
        let bucket = if bucket_count <= 1 || end <= start {
            0
        } else {
            ((((seconds - start) / (end - start)).clamp(0.0, 1.0) * bucket_count as f64).floor()
                as usize)
                .min(bucket_count - 1)
        };
        reduced[bucket].observe(seconds, value);
    }
    reduced
        .into_iter()
        .flat_map(TraceBucket::into_points)
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
struct TraceBucket {
    minimum: Option<(f64, f64)>,
    maximum: Option<(f64, f64)>,
    first_missing: Option<f64>,
}

impl TraceBucket {
    fn observe(&mut self, seconds: f64, value: Option<f64>) {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            self.first_missing.get_or_insert(seconds);
            return;
        };
        if self.minimum.is_none_or(|(_, minimum)| value < minimum) {
            self.minimum = Some((seconds, value));
        }
        if self.maximum.is_none_or(|(_, maximum)| value > maximum) {
            self.maximum = Some((seconds, value));
        }
    }

    fn into_points(self) -> Vec<TimelineSignalPoint> {
        let mut points = Vec::with_capacity(3);
        if let Some((seconds, value)) = self.minimum {
            points.push(TimelineSignalPoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some((seconds, value)) = self.maximum
            && self.minimum != self.maximum
        {
            points.push(TimelineSignalPoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some(seconds) = self.first_missing {
            points.push(TimelineSignalPoint {
                seconds,
                value: None,
            });
        }
        points.sort_by(|left, right| left.seconds.total_cmp(&right.seconds));
        points
    }
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
async fn recorded_deadline(millis: u32) {
    gloo_timers::future::TimeoutFuture::new(millis).await;
}

#[cfg(not(all(feature = "web", target_arch = "wasm32")))]
async fn recorded_deadline(_millis: u32) {
    std::future::pending::<()>().await
}

async fn with_deadline<T>(
    label: &str,
    millis: u32,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    use futures_util::future::{Either, select};
    let work = std::pin::pin!(work);
    let timer = std::pin::pin!(recorded_deadline(millis));
    match select(work, timer).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(format!(
            "{label} did not answer within {}s.",
            millis / 1_000
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_api::recording::{EpisodeArm, EpisodeOptionalFrameMatrix, EpisodeSummary};

    fn snapshot() -> EpisodeInspectionSnapshot {
        let absent = EpisodeOptionalFrameMatrix {
            present: vec![false; 3],
            values: vec![None; 6],
        };
        EpisodeInspectionSnapshot {
            episode: EpisodeSummary {
                dataset_id: "default".to_owned(),
                id: "take-1".to_owned(),
                arms: vec![EpisodeArm {
                    id: "follower".to_owned(),
                    robot_type: None,
                    joints: vec!["shoulder_pan".to_owned(), "gripper".to_owned()],
                }],
                frames: 3,
                duration_seconds: 2.0,
                recording_gaps: Vec::new(),
                sample_hz: 1.0,
                started_ms: 0,
                source: "Teleoperation".to_owned(),
                max_follow_error_degrees: None,
                discarded: false,
                problem: None,
            },
            sample_times_seconds: vec![0.0, 1.0, 2.0],
            follow_error_degrees: vec![None, Some(0.5), None],
            arms: vec![EpisodeInspectionArm {
                arm: "follower".to_owned(),
                joints: vec!["shoulder_pan".to_owned(), "gripper".to_owned()],
                normalized: vec![Some(0.0), Some(0.1), Some(0.5), Some(0.6), Some(1.0), None],
                raw: vec![0, 1, 5, 6, 10, 11],
                velocity: absent.clone(),
                effort: absent.clone(),
                action: absent,
            }],
            videos: Vec::new(),
        }
    }

    #[test]
    fn a_cached_snapshot_resolves_nearest_frames_without_another_request() {
        let inspection = Arc::new(CachedInspection::try_new(snapshot()).expect("snapshot"));
        let mut cache = EpisodeInspectionCache::default();
        cache.insert(Arc::clone(&inspection));

        let cached = cache.get("take-1", Some(3)).expect("cached episode");
        let preview = cached.preview_at(0.6).expect("preview");

        assert_eq!(preview.sample().frame, 1);
        assert_eq!(preview.sample().seconds, 1.0);
        let follower = preview
            .sample
            .arms
            .iter()
            .find(|arm| arm.arm == "follower")
            .expect("follower sample");
        assert_eq!(follower.raw["shoulder_pan"], 5);
        assert_eq!(preview.signal_lanes()[0].series[0].points.len(), 3);
    }

    #[test]
    fn a_new_frame_count_invalidates_the_cached_revision() {
        let inspection = Arc::new(CachedInspection::try_new(snapshot()).expect("snapshot"));
        let mut cache = EpisodeInspectionCache::default();
        cache.insert(inspection);

        assert!(cache.get("take-1", Some(3)).is_some());
        assert!(cache.get("take-1", Some(4)).is_none());
    }

    #[test]
    fn malformed_frame_matrices_fail_before_becoming_renderable() {
        let mut malformed = snapshot();
        malformed.arms[0].normalized.pop();

        assert!(
            CachedInspection::try_new(malformed)
                .expect_err("invalid snapshot")
                .contains("incomplete state track")
        );
    }
}
