//! Load-once recorded episode inspection and browser-local frame selection.
//!
//! A snapshot contains the complete visual joint timeline in a compact
//! columnar form. Data attaches the immutable loaded object to its catalog
//! entry and resolves playback and scrubbing from that object in memory. The
//! small workspace cache below is only for paused recordings outside Data;
//! physical motion continues through backend-owned validation and control.

use super::RecordingClient;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use dioxus::prelude::*;
use spiral_robotics_common::recording::{
    EpisodeInspectionArm, EpisodeInspectionSnapshot, EpisodeOptionalFrameMatrix,
};
use spiral_robotics_viewport::SO101_JOINTS;

use crate::activities::live::signals::SignalNamePresentation;
use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalPoint, TimelineSignalSeries,
};
use crate::panels::recorded_source::{LoadState, RecordedSource};
use crate::shell::state::StudioProjectState;

const EPISODE_RENDER_HZ: f64 = 30.0;
const RECORDED_REQUEST_TIMEOUT_MS: u32 = 15_000;
const TRACE_BUCKETS: usize = 240;
const MAX_CACHED_EPISODES: usize = 4;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineArmSample {
    arm: String,
    normalized: BTreeMap<String, Option<f64>>,
    degrees: BTreeMap<String, Option<f64>>,
    velocity: Option<BTreeMap<String, Option<f64>>>,
    effort: Option<BTreeMap<String, Option<f64>>>,
    action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineOptionalArmChannels {
    velocity: Option<BTreeMap<String, Option<f64>>>,
    effort: Option<BTreeMap<String, Option<f64>>>,
    action: Option<BTreeMap<String, Option<f64>>>,
}

impl TimelineOptionalArmChannels {
    pub(crate) const fn new(
        velocity: Option<BTreeMap<String, Option<f64>>>,
        effort: Option<BTreeMap<String, Option<f64>>>,
        action: Option<BTreeMap<String, Option<f64>>>,
    ) -> Self {
        Self {
            velocity,
            effort,
            action,
        }
    }
}

impl TimelineArmSample {
    pub(crate) fn new(
        arm: impl Into<String>,
        normalized: BTreeMap<String, Option<f64>>,
        degrees: BTreeMap<String, Option<f64>>,
    ) -> Self {
        Self {
            arm: arm.into(),
            normalized,
            degrees,
            velocity: None,
            effort: None,
            action: None,
        }
    }

    pub(crate) fn with_optional_channels(mut self, channels: TimelineOptionalArmChannels) -> Self {
        self.velocity = channels.velocity;
        self.effort = channels.effort;
        self.action = channels.action;
        self
    }

    pub(crate) fn arm(&self) -> &str {
        &self.arm
    }

    pub(crate) const fn normalized(&self) -> &BTreeMap<String, Option<f64>> {
        &self.normalized
    }

    pub(crate) const fn degrees(&self) -> &BTreeMap<String, Option<f64>> {
        &self.degrees
    }
}

/// The nearest stored sample to a committed recorded playhead position.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineSample {
    frame: u64,
    seconds: f64,
    follow_error_degrees: Option<f64>,
    arms: Vec<TimelineArmSample>,
}

impl TimelineSample {
    pub(crate) const fn new(frame: u64, seconds: f64) -> Self {
        Self {
            frame,
            seconds,
            follow_error_degrees: None,
            arms: Vec::new(),
        }
    }

    pub(crate) fn with_measurements(
        mut self,
        follow_error_degrees: Option<f64>,
        arms: Vec<TimelineArmSample>,
    ) -> Self {
        self.follow_error_degrees = follow_error_degrees;
        self.arms = arms;
        self
    }

    #[cfg(test)]
    pub(crate) const fn frame(&self) -> u64 {
        self.frame
    }

    pub(crate) const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub(crate) fn arms(&self) -> &[TimelineArmSample] {
        &self.arms
    }
}

pub(crate) trait EpisodePlaybackTime {
    fn replay_slot(self) -> u64;
}

impl EpisodePlaybackTime for f64 {
    fn replay_slot(self) -> u64 {
        if !self.is_finite() || self <= 0.0 {
            return 0;
        }
        (self * EPISODE_RENDER_HZ).floor() as u64
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodePreview {
    episode: String,
    sample: TimelineSample,
    signal_lanes: Arc<[TimelineSignalLane; 6]>,
}

impl EpisodePreview {
    fn from_inspection(inspection: &EpisodeData, sample: TimelineSample) -> Self {
        Self {
            episode: inspection.snapshot.episode().id().to_owned(),
            sample,
            signal_lanes: Arc::clone(&inspection.signal_lanes),
        }
    }

    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }

    #[cfg(test)]
    pub(crate) const fn sample(&self) -> &TimelineSample {
        &self.sample
    }

    pub(crate) fn signal_lanes(&self) -> &Arc<[TimelineSignalLane; 6]> {
        &self.signal_lanes
    }

    pub(crate) fn into_sample(self) -> TimelineSample {
        self.sample
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EpisodeData {
    snapshot: EpisodeInspectionSnapshot,
    signal_lanes: Arc<[TimelineSignalLane; 6]>,
}

impl EpisodeData {
    pub(crate) fn try_new(snapshot: EpisodeInspectionSnapshot) -> Result<Self, String> {
        snapshot.validate()?;
        let signal_lanes = Arc::new(snapshot.signal_lanes(TRACE_BUCKETS));
        Ok(Self {
            snapshot,
            signal_lanes,
        })
    }

    pub(crate) fn preview_at(&self, seconds: f64) -> Result<EpisodePreview, String> {
        let sample = self.snapshot.sample_at(seconds)?;
        Ok(EpisodePreview::from_inspection(self, sample))
    }

    pub(crate) async fn load(episode: &str) -> Result<Arc<Self>, String> {
        let request = RecordingClient::current().inspect_episode(episode.to_owned());
        EpisodeDataDeadline::run(
            "The episode data",
            RECORDED_REQUEST_TIMEOUT_MS,
            async move { request.await.map_err(|error| error.to_string()) },
        )
        .await
        .and_then(Self::try_new)
        .map(Arc::new)
    }
}

/// Small FIFO cache for recorded sources outside the Data catalog lifecycle.
/// A new persisted frame count replaces an older snapshot with the same ID.
#[derive(Clone, Debug, Default)]
pub(crate) struct EpisodeInspectionCache {
    entries: VecDeque<Arc<EpisodeData>>,
}

impl EpisodeInspectionCache {
    fn get(&self, episode: &str, expected_frames: Option<u64>) -> Option<Arc<EpisodeData>> {
        self.entries
            .iter()
            .find(|entry| {
                entry.snapshot.episode().id() == episode
                    && expected_frames
                        .is_none_or(|frames| entry.snapshot.episode().frames() == frames)
            })
            .cloned()
    }

    pub(crate) fn insert(&mut self, inspection: Arc<EpisodeData>) {
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

pub(crate) struct EpisodePreviewRequest {
    episode: String,
    expected_frames: Option<u64>,
    seconds: f64,
}

impl EpisodePreviewRequest {
    pub(crate) fn new(episode: String, expected_frames: Option<u64>, seconds: f64) -> Self {
        Self {
            episode,
            expected_frames,
            seconds,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EpisodePreviewSignals {
    state: Signal<Option<RecordedSource<EpisodePreview>>>,
    revision: Signal<u64>,
    cache: Signal<EpisodeInspectionCache>,
}

impl EpisodePreviewSignals {
    pub(crate) fn use_signals() -> Self {
        use_hook(|| Self {
            state: Signal::new(None),
            revision: Signal::new(0),
            cache: Signal::new(EpisodeInspectionCache::default()),
        })
    }

    pub(crate) const fn new(
        state: Signal<Option<RecordedSource<EpisodePreview>>>,
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
        Signal<Option<RecordedSource<EpisodePreview>>>,
        Signal<u64>,
        Signal<EpisodeInspectionCache>,
    ) {
        (self.state, self.revision, self.cache)
    }
}

impl EpisodePreviewSignals {
    pub(crate) fn request(self, request: EpisodePreviewRequest) {
        let studio = StudioProjectState::current();
        let EpisodePreviewRequest {
            episode,
            expected_frames,
            seconds,
        } = request;
        let EpisodePreviewSignals {
            mut state,
            mut revision,
            mut cache,
        } = self;
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
        dioxus::dioxus_core::Runtime::current().spawn(state.origin_scope(), async move {
            let loaded = EpisodeData::load(&episode).await;
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

trait EpisodeSnapshot {
    fn validate(&self) -> Result<(), String>;
    fn sample_at(&self, seconds: f64) -> Result<TimelineSample, String>;
    fn signal_lanes(&self, buckets: usize) -> [TimelineSignalLane; 6];
}

impl EpisodeSnapshot for EpisodeInspectionSnapshot {
    fn validate(&self) -> Result<(), String> {
        let frames = self.sample_times_seconds().len();
        if frames == 0 {
            return Err(format!(
                "Episode {:?} has no visual samples.",
                self.episode().id()
            ));
        }
        if self.episode().frames() != frames as u64 {
            return Err(format!(
                "Episode {:?} declares {} frames but delivered {frames} visual samples.",
                self.episode().id(),
                self.episode().frames()
            ));
        }
        if self.follow_error_degrees().len() != frames {
            return Err(format!(
                "Episode {:?} delivered an inconsistent follow-error track.",
                self.episode().id()
            ));
        }
        let mut previous = None;
        for seconds in self.sample_times_seconds() {
            if !seconds.is_finite()
                || *seconds < 0.0
                || previous.is_some_and(|last| *seconds < last)
            {
                return Err(format!(
                    "Episode {:?} delivered an invalid visual sample clock.",
                    self.episode().id()
                ));
            }
            previous = Some(*seconds);
        }
        if self.arms().len() != self.episode().arms().len() {
            return Err(format!(
                "Episode {:?} delivered an inconsistent arm layout.",
                self.episode().id()
            ));
        }
        for arm in self.arms() {
            let Some(expected) = self
                .episode()
                .arms()
                .iter()
                .find(|expected| expected.id() == arm.arm())
            else {
                return Err(format!(
                    "Episode {:?} delivered an unknown arm {:?}.",
                    self.episode().id(),
                    arm.arm()
                ));
            };
            if arm.joints() != expected.joints() {
                return Err(format!(
                    "Episode {:?} delivered a different joint layout for {:?}.",
                    self.episode().id(),
                    arm.arm()
                ));
            }
            let values = frames
                .checked_mul(arm.joints().len())
                .ok_or_else(|| "Episode inspection dimensions overflowed.".to_owned())?;
            if arm.normalized().len() != values
                || arm.raw().len() != values
                || arm.degrees().len() != values
            {
                return Err(format!(
                    "Episode {:?} delivered an incomplete state track for {:?}.",
                    self.episode().id(),
                    arm.arm()
                ));
            }
            let matrices = OptionalMatrixValidation {
                snapshot: self,
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

    fn sample_at(&self, seconds: f64) -> Result<TimelineSample, String> {
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
            TimelineSample::new(frame as u64, self.sample_times_seconds()[frame])
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

trait RecordedArm {
    fn sample_at(&self, frame: usize) -> Result<TimelineArmSample, String>;
}

impl RecordedArm for EpisodeInspectionArm {
    fn sample_at(&self, frame: usize) -> Result<TimelineArmSample, String> {
        let joint_count = self.joints().len();
        let start = frame
            .checked_mul(joint_count)
            .ok_or_else(|| "Episode inspection frame offset overflowed.".to_owned())?;
        let end = start + joint_count;
        let optional = |matrix: &EpisodeOptionalFrameMatrix| {
            matrix.present()[frame].then(|| {
                self.joints()
                    .iter()
                    .cloned()
                    .zip(matrix.values()[start..end].iter().copied())
                    .collect::<BTreeMap<_, _>>()
            })
        };
        Ok(TimelineArmSample::new(
            self.arm(),
            self.joints()
                .iter()
                .cloned()
                .zip(self.normalized()[start..end].iter().copied())
                .collect(),
            self.joints()
                .iter()
                .cloned()
                .zip(self.degrees()[start..end].iter().copied())
                .collect(),
        )
        .with_optional_channels(TimelineOptionalArmChannels::new(
            optional(self.velocity()),
            optional(self.effort()),
            optional(self.action()),
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

struct EpisodeDataDeadline;

impl EpisodeDataDeadline {
    async fn timer(millis: u32) {
        gloo_timers::future::TimeoutFuture::new(millis).await;
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
    use spiral_robotics_common::recording::{
        EpisodeArm, EpisodeArmScalarSamples, EpisodeInfo, EpisodeInspectionArm,
        EpisodeOptionalFrameMatrix,
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
        let episode = EpisodeInfo::new("take-1", "default")
            .with_arms(vec![EpisodeArm::new("follower", None, joints.clone())])
            .with_source("Teleoperation")
            .with_timeline(3, 2.0)
            .with_sample_hz(1.0);
        let arm = EpisodeInspectionArm::new(
            "follower",
            joints,
            EpisodeArmScalarSamples::new(normalized, vec![0, 1, 5, 6, 10, 11])
                .with_degrees(vec![Some(0.0); 6]),
        )
        .with_velocity(absent.clone())
        .with_effort(absent.clone())
        .with_action(absent);
        EpisodeInspectionSnapshot::new(episode, vec![0.0, 1.0, 2.0], vec![None, Some(0.5), None])
            .with_arms(vec![arm])
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_cached_snapshot_resolves_nearest_frames_without_another_request() {
        let inspection = Arc::new(EpisodeData::try_new(snapshot()).expect("snapshot"));
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
        assert_eq!(follower.degrees()["shoulder_pan"], Some(0.0));
        assert_eq!(preview.signal_lanes()[0].series()[0].points().len(), 3);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_new_frame_count_invalidates_the_cached_revision() {
        let inspection = Arc::new(EpisodeData::try_new(snapshot()).expect("snapshot"));
        let mut cache = EpisodeInspectionCache::default();
        cache.insert(inspection);

        assert!(cache.get("take-1", Some(3)).is_some());
        assert!(cache.get("take-1", Some(4)).is_none());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn malformed_frame_matrices_fail_before_becoming_renderable() {
        let malformed =
            snapshot_with_normalized(vec![Some(0.0), Some(0.1), Some(0.5), Some(0.6), Some(1.0)]);

        assert!(
            EpisodeData::try_new(malformed)
                .expect_err("invalid snapshot")
                .contains("incomplete state track")
        );
    }
}
