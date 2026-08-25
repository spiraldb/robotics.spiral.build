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
use crate::client::http::EpisodeApiPath;
use crate::panels::timeline::panel::{
    TimelineSignalLane, TimelineSignalPoint, TimelineSignalSeries,
};
use crate::panels::timeline::signals::SignalNamePresentation;
use crate::shell::activity::{LoadState, RecordedSource};
use crate::shell::state::Studio;

const HISTORICAL_RENDER_HZ: f64 = 30.0;
const RECORDED_REQUEST_TIMEOUT_MS: u32 = 15_000;
const TRACE_BUCKETS: usize = 240;
const MAX_CACHED_EPISODES: usize = 4;

pub(crate) trait HistoricalReplayTime {
    fn replay_slot(self) -> u64;
}

impl HistoricalReplayTime for f64 {
    fn replay_slot(self) -> u64 {
        if !self.is_finite() || self <= 0.0 {
            return 0;
        }
        (self * HISTORICAL_RENDER_HZ).floor() as u64
    }
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
            episode: inspection.snapshot.episode().id().to_owned(),
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
        snapshot.validate()?;
        let timeline = Arc::new(api::EpisodeTimelineResponse::new(
            snapshot.episode().to_owned(),
            api::TimelineMetadata::new(
                snapshot.sample_times_seconds().len() as u64,
                snapshot.sample_times_seconds().first().copied(),
                snapshot.sample_times_seconds().last().copied(),
            ),
            snapshot.videos().to_owned(),
        ));
        let signal_lanes = Arc::new(snapshot.signal_lanes(TRACE_BUCKETS));
        Ok(Self {
            snapshot,
            timeline,
            signal_lanes,
        })
    }

    fn preview_at(&self, seconds: f64) -> Result<HistoricalPreview, String> {
        let sample = self.snapshot.sample_at(seconds)?;
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
                entry.snapshot.episode().id() == episode
                    && expected_frames
                        .is_none_or(|frames| entry.snapshot.episode().frames() == frames)
            })
            .cloned()
    }

    fn insert(&mut self, inspection: Arc<CachedInspection>) {
        self.entries
            .retain(|entry| entry.snapshot.episode().id() != inspection.snapshot.episode().id());
        self.entries.push_front(inspection);
        self.entries.truncate(MAX_CACHED_EPISODES);
    }

    pub(crate) fn invalidate(&mut self, episode: &str) {
        self.entries
            .retain(|entry| entry.snapshot.episode().id() != episode);
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

impl Studio {
    pub(crate) fn request_historical_preview(
        self,
        request: HistoricalPreviewRequest,
        signals: HistoricalPreviewSignals,
    ) {
        let studio = self;
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
            source.episode() == episode && matches!(source.state(), LoadState::Loading { .. })
        });
        if same_episode_loading && cache.peek().get(&episode, expected_frames).is_none() {
            if let Some(source) = state.write().as_mut() {
                source.set_seconds(seconds);
            }
            return;
        }

        let ticket = revision.peek().wrapping_add(1);
        revision.set(ticket);
        if let Some(inspection) = cache.peek().get(&episode, expected_frames) {
            match inspection.preview_at(seconds) {
                Ok(preview) => state.set(Some(RecordedSource::new(
                    episode,
                    seconds,
                    LoadState::Ready(preview),
                ))),
                Err(error) => {
                    state.set(Some(RecordedSource::new(
                        episode,
                        seconds,
                        LoadState::Failed {
                            reason: error.clone(),
                            previous: None,
                        },
                    )));
                    studio.report(error);
                }
            }
            return;
        }

        let previous = state
            .peek()
            .as_ref()
            .filter(|source| source.episode() == episode)
            .and_then(RecordedSource::renderable)
            .cloned();
        state.set(Some(RecordedSource::new(
            episode.clone(),
            seconds,
            LoadState::Loading {
                previous: previous.clone(),
            },
        )));
        spawn(async move {
            let path = episode.inspection_url();
            let loaded = HistoricalDeadline::run(
                "The episode inspection snapshot",
                RECORDED_REQUEST_TIMEOUT_MS,
                api::Http::get::<api::EpisodeInspectionSnapshot>(&path),
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
                        .is_some_and(|frames| inspection.snapshot.episode().frames() != frames)
                    {
                        let error = format!(
                            "The episode changed while it was loading (expected {expected} frames, received {actual}). Reload it to inspect the latest revision.",
                            expected = expected_frames.unwrap_or_default(),
                            actual = inspection.snapshot.episode().frames(),
                        );
                        state.set(Some(RecordedSource::new(
                            episode,
                            seconds,
                            LoadState::Failed {
                                reason: error.clone(),
                                previous,
                            },
                        )));
                        studio.report(error);
                        return;
                    }
                    let requested_seconds = state
                        .peek()
                        .as_ref()
                        .filter(|source| source.episode() == episode)
                        .map_or(seconds, |source| source.seconds());
                    match inspection.preview_at(requested_seconds) {
                        Ok(preview) => {
                            cache.write().insert(Arc::clone(&inspection));
                            state.set(Some(RecordedSource::new(
                                episode,
                                requested_seconds,
                                LoadState::Ready(preview),
                            )));
                        }
                        Err(error) => {
                            state.set(Some(RecordedSource::new(
                                episode,
                                requested_seconds,
                                LoadState::Failed {
                                    reason: error.clone(),
                                    previous,
                                },
                            )));
                            studio.report(error);
                        }
                    }
                }
                Err(error) => {
                    state.set(Some(RecordedSource::new(
                        episode,
                        seconds,
                        LoadState::Failed {
                            reason: error.clone(),
                            previous,
                        },
                    )));
                    studio.report(error);
                }
            }
        });
    }
}

trait HistoricalSnapshot {
    fn validate(&self) -> Result<(), String>;
    fn sample_at(&self, seconds: f64) -> Result<api::TimelineSample, String>;
    fn signal_lanes(&self, buckets: usize) -> [TimelineSignalLane; 6];
}

impl HistoricalSnapshot for EpisodeInspectionSnapshot {
    fn validate(&self) -> Result<(), String> {
        let snapshot = self;
        let frames = self.sample_times_seconds().len();
        if frames == 0 {
            return Err(format!(
                "Episode {:?} has no visual samples.",
                snapshot.episode().id()
            ));
        }
        if snapshot.episode().frames() != frames as u64 {
            return Err(format!(
                "Episode {:?} declares {} frames but delivered {frames} visual samples.",
                snapshot.episode().id(),
                snapshot.episode().frames()
            ));
        }
        if snapshot.follow_error_degrees().len() != frames {
            return Err(format!(
                "Episode {:?} delivered an inconsistent follow-error track.",
                snapshot.episode().id()
            ));
        }
        let mut previous = None;
        for seconds in snapshot.sample_times_seconds() {
            if !seconds.is_finite()
                || *seconds < 0.0
                || previous.is_some_and(|last| *seconds < last)
            {
                return Err(format!(
                    "Episode {:?} delivered an invalid visual sample clock.",
                    snapshot.episode().id()
                ));
            }
            previous = Some(*seconds);
        }
        if snapshot.arms().len() != snapshot.episode().arms().len() {
            return Err(format!(
                "Episode {:?} delivered an inconsistent arm layout.",
                snapshot.episode().id()
            ));
        }
        for arm in snapshot.arms() {
            let Some(expected) = snapshot
                .episode()
                .arms()
                .iter()
                .find(|expected| expected.id() == arm.arm())
            else {
                return Err(format!(
                    "Episode {:?} delivered an unknown arm {:?}.",
                    snapshot.episode().id(),
                    arm.arm()
                ));
            };
            if arm.joints() != expected.joints() {
                return Err(format!(
                    "Episode {:?} delivered a different joint layout for {:?}.",
                    snapshot.episode().id(),
                    arm.arm()
                ));
            }
            let values = frames
                .checked_mul(arm.joints().len())
                .ok_or_else(|| "Episode inspection dimensions overflowed.".to_owned())?;
            if arm.normalized().len() != values || arm.raw().len() != values {
                return Err(format!(
                    "Episode {:?} delivered an incomplete state track for {:?}.",
                    snapshot.episode().id(),
                    arm.arm()
                ));
            }
            let matrices = OptionalMatrixValidation {
                snapshot,
                arm,
                frames,
                values,
            };
            matrices.validate("velocity", arm.velocity())?;
            matrices.validate("effort", arm.effort())?;
            matrices.validate("action", arm.action())?;
        }
        Ok(())
    }

    fn sample_at(&self, seconds: f64) -> Result<api::TimelineSample, String> {
        if !seconds.is_finite() {
            return Err("Timeline time must be finite.".to_owned());
        }
        let frame = self
            .sample_times_seconds()
            .nearest_frame(seconds)
            .ok_or_else(|| format!("Episode {:?} has no visual samples.", self.episode().id()))?;
        let arms = self
            .arms()
            .iter()
            .map(|arm| arm.sample_at(frame))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(
            api::TimelineSample::new(frame as u64, self.sample_times_seconds()[frame])
                .with_measurements(self.follow_error_degrees()[frame], arms),
        )
    }

    fn signal_lanes(&self, buckets: usize) -> [TimelineSignalLane; 6] {
        std::array::from_fn(|lane_index| {
            let joint = SO101_JOINTS[lane_index];
            let series = self
                .arms()
                .iter()
                .filter_map(|arm| {
                    let joint_index = arm.joints().iter().position(|name| name == joint.name())?;
                    let joint_count = arm.joints().len();
                    let values = arm
                        .normalized()
                        .chunks_exact(joint_count)
                        .map(|frame| frame[joint_index]);
                    Some(TimelineSignalSeries::new(
                        arm.arm(),
                        TraceReduction::new(self.sample_times_seconds(), buckets).apply(values),
                    ))
                })
                .collect();
            TimelineSignalLane::new(joint.name(), joint.label(), joint.name().signal_color())
                .with_series(series)
        })
    }
}

struct OptionalMatrixValidation<'a> {
    snapshot: &'a EpisodeInspectionSnapshot,
    arm: &'a EpisodeInspectionArm,
    frames: usize,
    values: usize,
}

impl OptionalMatrixValidation<'_> {
    fn validate(&self, label: &str, matrix: &EpisodeOptionalFrameMatrix) -> Result<(), String> {
        if matrix.present().len() != self.frames || matrix.values().len() != self.values {
            return Err(format!(
                "Episode {:?} delivered an incomplete {label} track for {:?}.",
                self.snapshot.episode().id(),
                self.arm.arm()
            ));
        }
        Ok(())
    }
}

trait SampleClock {
    fn nearest_frame(&self, requested: f64) -> Option<usize>;
}

impl SampleClock for [f64] {
    fn nearest_frame(&self, requested: f64) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        let after = self.partition_point(|candidate| *candidate <= requested);
        match after {
            0 => Some(0),
            index if index >= self.len() => Some(self.len() - 1),
            index => {
                let before = index - 1;
                if (requested - self[before]).abs() <= (self[index] - requested).abs() {
                    Some(before)
                } else {
                    Some(index)
                }
            }
        }
    }
}

trait HistoricalArm {
    fn sample_at(&self, frame: usize) -> Result<api::TimelineArmSample, String>;
}

impl HistoricalArm for EpisodeInspectionArm {
    fn sample_at(&self, frame: usize) -> Result<api::TimelineArmSample, String> {
        let arm = self;
        let joint_count = self.joints().len();
        let start = frame
            .checked_mul(joint_count)
            .ok_or_else(|| "Episode inspection frame offset overflowed.".to_owned())?;
        let end = start + joint_count;
        let optional = |matrix: &EpisodeOptionalFrameMatrix| {
            matrix.present()[frame].then(|| {
                arm.joints()
                    .iter()
                    .cloned()
                    .zip(matrix.values()[start..end].iter().copied())
                    .collect::<BTreeMap<_, _>>()
            })
        };
        Ok(api::TimelineArmSample::new(
            arm.arm(),
            arm.joints()
                .iter()
                .cloned()
                .zip(arm.normalized()[start..end].iter().copied())
                .collect(),
            arm.joints()
                .iter()
                .cloned()
                .zip(arm.raw()[start..end].iter().copied())
                .collect(),
        )
        .with_optional_channels(api::TimelineOptionalArmChannels::new(
            optional(arm.velocity()),
            optional(arm.effort()),
            optional(arm.action()),
        )))
    }
}

struct TraceReduction<'a> {
    times: &'a [f64],
    buckets: usize,
}

impl<'a> TraceReduction<'a> {
    const fn new(times: &'a [f64], buckets: usize) -> Self {
        Self { times, buckets }
    }

    fn apply(&self, values: impl Iterator<Item = Option<f64>>) -> Vec<TimelineSignalPoint> {
        let bucket_count = self.buckets.clamp(1, 512).min(self.times.len());
        if bucket_count == 0 {
            return Vec::new();
        }
        let start = self.times[0];
        let end = self.times[self.times.len() - 1];
        let mut reduced = vec![TraceBucket::default(); bucket_count];
        for (seconds, value) in self.times.iter().copied().zip(values) {
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
            points.push(TimelineSignalPoint::new(seconds, Some(value)));
        }
        if let Some((seconds, value)) = self.maximum
            && self.minimum != self.maximum
        {
            points.push(TimelineSignalPoint::new(seconds, Some(value)));
        }
        if let Some(seconds) = self.first_missing {
            points.push(TimelineSignalPoint::new(seconds, None));
        }
        points.sort_by(|left, right| left.seconds().total_cmp(&right.seconds()));
        points
    }
}

struct HistoricalDeadline;

impl HistoricalDeadline {
    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    async fn timer(millis: u32) {
        gloo_timers::future::TimeoutFuture::new(millis).await;
    }

    #[cfg(not(all(feature = "web", target_arch = "wasm32")))]
    async fn timer(_millis: u32) {
        std::future::pending::<()>().await
    }

    async fn run<T>(
        label: &str,
        millis: u32,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        use futures_util::future::{Either, select};
        let work = std::pin::pin!(work);
        let timer = std::pin::pin!(Self::timer(millis));
        match select(work, timer).await {
            Either::Left((result, _)) => result,
            Either::Right(((), _)) => Err(format!(
                "{label} did not answer within {}s.",
                millis / 1_000
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_api::recording::{
        EpisodeArm, EpisodeArmOptionalChannels, EpisodeArmScalarSamples, EpisodeAssessment,
        EpisodeIdentity, EpisodeInspectionArm, EpisodeOptionalFrameMatrix, EpisodeSummary,
        EpisodeTimeline,
    };

    fn snapshot() -> EpisodeInspectionSnapshot {
        snapshot_with_normalized(vec![
            Some(0.0),
            Some(0.1),
            Some(0.5),
            Some(0.6),
            Some(1.0),
            None,
        ])
    }

    fn snapshot_with_normalized(normalized: Vec<Option<f64>>) -> EpisodeInspectionSnapshot {
        let joints = vec!["shoulder_pan".to_owned(), "gripper".to_owned()];
        let absent = EpisodeOptionalFrameMatrix::new(vec![false; 3], vec![None; 6]);
        let episode = EpisodeSummary::new(
            EpisodeIdentity::new(
                "take-1",
                "default",
                vec![EpisodeArm::new("follower", None, joints.clone())],
            )
            .with_source("Teleoperation"),
            EpisodeTimeline::new(3, 2.0, 1.0),
            EpisodeAssessment::new(false, None, None),
        );
        let arm = EpisodeInspectionArm::new(
            "follower",
            joints,
            EpisodeArmScalarSamples::new(normalized, vec![0, 1, 5, 6, 10, 11]),
        )
        .with_optional_channels(EpisodeArmOptionalChannels::new(
            absent.clone(),
            absent.clone(),
            absent,
        ));
        EpisodeInspectionSnapshot::new(episode, vec![0.0, 1.0, 2.0], vec![None, Some(0.5), None])
            .with_arms(vec![arm])
    }

    #[test]
    fn a_cached_snapshot_resolves_nearest_frames_without_another_request() {
        let inspection = Arc::new(CachedInspection::try_new(snapshot()).expect("snapshot"));
        let mut cache = EpisodeInspectionCache::default();
        cache.insert(Arc::clone(&inspection));

        let cached = cache.get("take-1", Some(3)).expect("cached episode");
        let preview = cached.preview_at(0.6).expect("preview");

        assert_eq!(preview.sample().frame(), 1);
        assert_eq!(preview.sample().seconds(), 1.0);
        let follower = preview
            .sample()
            .arms()
            .iter()
            .find(|arm| arm.arm() == "follower")
            .expect("follower sample");
        assert_eq!(follower.raw()["shoulder_pan"], 5);
        assert_eq!(preview.signal_lanes()[0].series()[0].points().len(), 3);
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
        let malformed =
            snapshot_with_normalized(vec![Some(0.0), Some(0.1), Some(0.5), Some(0.6), Some(1.0)]);

        assert!(
            CachedInspection::try_new(malformed)
                .expect_err("invalid snapshot")
                .contains("incomplete state track")
        );
    }
}
