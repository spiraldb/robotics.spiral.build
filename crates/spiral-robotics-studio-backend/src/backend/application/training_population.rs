//! Pure admission and sampling policy for one training population.
//!
//! Database adapters decode rows into the plain records in this module. The
//! policy then decides which samples belong to a run without knowing about
//! SQL, Arrow, video decoders, tensor frameworks, or task runtimes.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) type StreamKey = (String, String);
pub(crate) const MAX_TRAINING_EPISODE_FRAMES: i64 = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PopulationRow {
    pub episode_id: String,
    pub stream_id: String,
    pub first_index: i64,
    pub last_index: i64,
    pub row_count: i64,
    pub sample_hz_min: f64,
    pub sample_hz_max: f64,
    pub episode_frames_min: i64,
    pub episode_frames_max: i64,
    pub robot_type_min: Option<String>,
    pub robot_type_max: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SampleRange {
    pub episode_id: String,
    pub stream_id: String,
    pub first_index: i64,
    pub last_index: i64,
    pub robot_type: String,
    pub sample_hz: f64,
}

impl SampleRange {
    pub fn count(&self) -> Result<u64, String> {
        let count = self
            .last_index
            .checked_sub(self.first_index)
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| {
                format!(
                    "training range ({:?}, {:?}) is too large to count",
                    self.episode_id, self.stream_id
                )
            })?;
        u64::try_from(count).map_err(|_| {
            format!(
                "training range ({:?}, {:?}) has negative bounds",
                self.episode_id, self.stream_id
            )
        })
    }
}

/// Partition synchronized ranges by whole episode so validation never observes
/// frames from an episode used for optimizer updates.
pub(crate) fn split_episode_ranges(
    ranges: Vec<SampleRange>,
    validation_split: f64,
    seed: u64,
) -> Result<(Vec<SampleRange>, Vec<SampleRange>), String> {
    if !validation_split.is_finite() || !(0.0..=0.5).contains(&validation_split) {
        return Err("validation split must be between 0 and 0.5".to_owned());
    }
    let mut episodes = ranges
        .iter()
        .map(|range| range.episode_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if validation_split == 0.0 || episodes.len() < 2 {
        return Ok((ranges, Vec::new()));
    }

    episodes.sort_by(|left, right| {
        episode_split_score(seed, left)
            .cmp(&episode_split_score(seed, right))
            .then_with(|| left.cmp(right))
    });
    let validation_count =
        ((episodes.len() as f64 * validation_split).round() as usize).clamp(1, episodes.len() - 1);
    let validation_episodes = episodes
        .into_iter()
        .take(validation_count)
        .collect::<BTreeSet<_>>();
    let (validation, training) = ranges
        .into_iter()
        .partition(|range| validation_episodes.contains(&range.episode_id));
    Ok((training, validation))
}

fn episode_split_score(seed: u64, episode_id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
    for byte in episode_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingPopulation {
    pub ranges: Vec<SampleRange>,
    pub sample_hz: f64,
}

pub(crate) fn admit_population(rows: Vec<PopulationRow>) -> Result<TrainingPopulation, String> {
    if rows.is_empty() {
        return Err("the Spiral training query returned no rows".to_owned());
    }

    let mut seen = BTreeSet::new();
    let mut ranges = Vec::with_capacity(rows.len());
    for row in rows {
        let key = (row.episode_id.clone(), row.stream_id.clone());
        if row.episode_id.is_empty() || row.stream_id.is_empty() || !seen.insert(key.clone()) {
            return Err(format!(
                "training population contains an empty or duplicate stream key {key:?}"
            ));
        }
        let expected_rows = row
            .last_index
            .checked_sub(row.first_index)
            .and_then(|count| count.checked_add(1));
        if row.first_index < 0
            || row.last_index < row.first_index
            || expected_rows != Some(row.row_count)
        {
            return Err(format!(
                "training rows for ({:?}, {:?}) are duplicated or non-contiguous",
                row.episode_id, row.stream_id
            ));
        }
        if row.episode_frames_min != row.episode_frames_max
            || row.episode_frames_min <= row.last_index
            || row.episode_frames_min > MAX_TRAINING_EPISODE_FRAMES
        {
            return Err(format!(
                "training rows for ({:?}, {:?}) have an inconsistent episode frame count; expected 1–{MAX_TRAINING_EPISODE_FRAMES}",
                row.episode_id, row.stream_id,
            ));
        }
        if !sample_rate_is_exact(row.sample_hz_min, row.sample_hz_max) {
            return Err(format!(
                "training rows for ({:?}, {:?}) have an inconsistent sample rate",
                row.episode_id, row.stream_id
            ));
        }
        let robot_min = resolve_robot_type(row.robot_type_min, &row.stream_id)?;
        let robot_max = resolve_robot_type(row.robot_type_max, &row.stream_id)?;
        if robot_min != robot_max {
            return Err(format!(
                "training rows for ({:?}, {:?}) mixed robot semantics",
                row.episode_id, row.stream_id
            ));
        }
        ranges.push(SampleRange {
            episode_id: row.episode_id,
            stream_id: row.stream_id,
            first_index: row.first_index,
            last_index: row.last_index,
            robot_type: robot_min,
            sample_hz: row.sample_hz_min,
        });
    }

    let sample_hz = ranges[0].sample_hz;
    if ranges
        .iter()
        .any(|range| range.sample_hz.to_bits() != sample_hz.to_bits())
    {
        return Err("one ACT run requires one fixed sample rate".to_owned());
    }
    Ok(TrainingPopulation { ranges, sample_hz })
}

pub(crate) fn sample_rate_is_exact(minimum: f64, maximum: f64) -> bool {
    minimum.is_finite()
        && maximum.is_finite()
        && minimum > 0.0
        && minimum.to_bits() == maximum.to_bits()
}

fn resolve_robot_type(value: Option<String>, stream_id: &str) -> Result<String, String> {
    if let Some(value) = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        return Ok(value);
    }
    if stream_id == "follower" {
        Ok("so101_follower".to_owned())
    } else {
        Err(format!("joint stream {stream_id:?} omitted its robot type"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StreamContractRow {
    pub episode_id: String,
    pub stream_id: String,
    pub robot_type: Option<String>,
    pub joint_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamContract {
    pub robot_type: String,
    pub joint_names: Vec<String>,
}

pub(crate) fn validate_stream_contracts(
    ranges: &[SampleRange],
    rows: Vec<StreamContractRow>,
) -> Result<StreamContract, String> {
    if ranges.is_empty() {
        return Err("the training population has no streams".to_owned());
    }
    let mut wanted = BTreeMap::new();
    for range in ranges {
        let key = (range.episode_id.clone(), range.stream_id.clone());
        if let Some(previous) = wanted.insert(key.clone(), range.robot_type.clone())
            && previous != range.robot_type
        {
            return Err(format!("training stream {key:?} mixed robot semantics"));
        }
    }

    let mut contracts = BTreeMap::new();
    for row in rows {
        let key = (row.episode_id, row.stream_id);
        let Some(expected_robot) = wanted.get(&key) else {
            continue;
        };
        let unique = row.joint_names.iter().collect::<BTreeSet<_>>();
        if row.joint_names.is_empty()
            || row.joint_names.iter().any(|joint| joint.trim().is_empty())
            || unique.len() != row.joint_names.len()
        {
            return Err(format!(
                "stream contract for {key:?} must contain unique, non-empty joint names"
            ));
        }
        let contract = StreamContract {
            robot_type: resolve_robot_type(row.robot_type, &key.1)?,
            joint_names: row.joint_names,
        };
        if &contract.robot_type != expected_robot {
            return Err(format!("robot type changed in stream contract for {key:?}"));
        }
        if contracts.insert(key.clone(), contract).is_some() {
            return Err(format!("duplicate stream contract for {key:?}"));
        }
    }
    if contracts.len() != wanted.len() {
        return Err("one or more training streams have no persisted stream contract".to_owned());
    }
    let canonical = contracts
        .values()
        .next()
        .cloned()
        .ok_or_else(|| "the training population has no stream contract".to_owned())?;
    if contracts.values().any(|contract| contract != &canonical) {
        return Err("one ACT run requires one robot type and one exact joint order".to_owned());
    }
    Ok(canonical)
}

pub(crate) fn validate_vector_contract(
    state_dimensions: usize,
    action_dimensions: usize,
    contract: &StreamContract,
) -> Result<(), String> {
    if state_dimensions == 0
        || action_dimensions == 0
        || state_dimensions != contract.joint_names.len()
        || action_dimensions != contract.joint_names.len()
    {
        return Err(
            "state/action vector widths do not match the persisted joint-name contract".to_owned(),
        );
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CameraSegment {
    pub timeline_offset_ns: i64,
    pub fps: u64,
    pub frames: u64,
}

impl CameraSegment {
    pub fn new(timeline_offset_ns: i64, fps: u64, frames: u64) -> Result<Self, String> {
        if fps == 0 || frames == 0 {
            return Err("camera segment has an invalid video contract".to_owned());
        }
        Ok(Self {
            timeline_offset_ns,
            fps,
            frames,
        })
    }

    fn start_seconds(&self) -> f64 {
        self.timeline_offset_ns as f64 / 1_000_000_000.0
    }

    fn end_seconds(&self) -> f64 {
        self.start_seconds() + self.frames as f64 / self.fps as f64
    }

    pub fn overlaps(&self, next: &Self) -> bool {
        next.start_seconds() < self.end_seconds()
    }

    pub fn sample_bounds(&self, sample_hz: f64) -> Result<Option<(i64, i64)>, String> {
        if !sample_rate_is_exact(sample_hz, sample_hz) {
            return Err("camera coverage requires a finite positive sample rate".to_owned());
        }
        // Start one candidate outside each ideal boundary, then trim with the
        // exact same predicate used when a sampled frame is selected.
        let mut first = finite_i64_bound(self.start_seconds() * sample_hz, f64::floor)?;
        let mut last = finite_i64_bound(self.end_seconds() * sample_hz, f64::ceil)?;
        while first <= last && self.frame_for(first, sample_hz)?.is_none() {
            first = first
                .checked_add(1)
                .ok_or_else(|| "camera sample interval overflowed".to_owned())?;
        }
        while first <= last && self.frame_for(last, sample_hz)?.is_none() {
            last = last
                .checked_sub(1)
                .ok_or_else(|| "camera sample interval overflowed".to_owned())?;
        }
        Ok((first <= last).then_some((first, last)))
    }

    pub fn frame_for(&self, sample_index: i64, sample_hz: f64) -> Result<Option<u64>, String> {
        if !sample_rate_is_exact(sample_hz, sample_hz) {
            return Err("camera frame selection requires a finite positive sample rate".to_owned());
        }
        let seconds = sample_index as f64 / sample_hz;
        if seconds < self.start_seconds() || seconds >= self.end_seconds() {
            return Ok(None);
        }
        let frame = ((seconds - self.start_seconds()) * self.fps as f64).floor();
        if !frame.is_finite() || frame < 0.0 || frame >= self.frames as f64 {
            return Ok(None);
        }
        Ok(Some(frame as u64))
    }
}

fn finite_i64_bound(value: f64, round: fn(f64) -> f64) -> Result<i64, String> {
    let value = round(value);
    // `i64::MAX as f64` rounds to 2^63, which is already outside i64.
    if !value.is_finite() || value < i64::MIN as f64 || value >= i64::MAX as f64 {
        return Err("camera sample interval is outside the supported index range".to_owned());
    }
    Ok(value as i64)
}

pub(crate) fn ensure_non_overlapping_camera_segments(
    key: &StreamKey,
    segments: &[CameraSegment],
) -> Result<(), String> {
    if segments.windows(2).any(|pair| pair[0].overlaps(&pair[1])) {
        return Err(format!("overlapping fixed-FPS camera segments for {key:?}"));
    }
    Ok(())
}

pub(crate) fn common_camera_streams(
    segments: &BTreeMap<StreamKey, Vec<CameraSegment>>,
    ranges: &[SampleRange],
) -> Vec<String> {
    let episodes = ranges
        .iter()
        .map(|range| range.episode_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut common: Option<BTreeSet<String>> = None;
    for episode in episodes {
        let available = segments
            .iter()
            .filter(|((candidate, _), values)| candidate == episode && !values.is_empty())
            .map(|((_, camera), _)| camera.clone())
            .collect::<BTreeSet<_>>();
        common = Some(match common {
            None => available,
            Some(previous) => previous.intersection(&available).cloned().collect(),
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

/// Resolve the one camera cadence that owns ACT's observation and action axis.
///
/// Joint telemetry may be sampled faster, but every selected camera in every
/// admitted episode must describe the same fixed-FPS policy clock.
pub(crate) fn camera_sample_hz(
    ranges: &[SampleRange],
    cameras: &[String],
    segments: &BTreeMap<StreamKey, Vec<CameraSegment>>,
) -> Result<f64, String> {
    if cameras.is_empty() {
        return Err("ACT requires at least one camera stream".to_owned());
    }
    let episodes = ranges
        .iter()
        .map(|range| range.episode_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut expected = None;
    for episode in episodes {
        for camera in cameras {
            let key = (episode.to_owned(), camera.clone());
            let values = segments
                .get(&key)
                .filter(|values| !values.is_empty())
                .ok_or_else(|| format!("camera stream {key:?} has no recorded video segment"))?;
            for segment in values {
                if expected.is_some_and(|fps| fps != segment.fps) {
                    return Err(format!(
                        "one ACT run requires one fixed camera frame rate; {key:?} uses {} FPS",
                        segment.fps
                    ));
                }
                expected.get_or_insert(segment.fps);
            }
        }
    }
    expected
        .map(|fps| fps as f64)
        .ok_or_else(|| "the selected cameras have no fixed frame rate".to_owned())
}

/// Convert joint-sample population ranges onto the shared camera-frame axis.
///
/// The returned `SampleRange::first_index`/`last_index` values are camera
/// frame indices. `sample_hz` is consequently the camera rate, not the motor
/// polling rate stored on the source range.
pub(crate) fn ranges_with_camera_frame_coverage(
    ranges: &[SampleRange],
    cameras: &[String],
    segments: &BTreeMap<StreamKey, Vec<CameraSegment>>,
    camera_hz: f64,
) -> Result<Vec<SampleRange>, String> {
    if !sample_rate_is_exact(camera_hz, camera_hz) {
        return Err("camera frame coverage requires a finite positive rate".to_owned());
    }
    let mut accepted = Vec::new();
    for range in ranges {
        if range.sample_hz < camera_hz {
            return Err(format!(
                "joint stream ({:?}, {:?}) runs at {} Hz, below the {camera_hz} Hz camera rate",
                range.episode_id, range.stream_id, range.sample_hz
            ));
        }
        let Some(joint_bounds) = camera_bounds_for_joint_range(range, camera_hz)? else {
            continue;
        };
        let mut intervals = vec![joint_bounds];
        for camera in cameras {
            let mut camera_intervals = segments
                .get(&(range.episode_id.clone(), camera.clone()))
                .into_iter()
                .flatten()
                .map(|segment| segment.sample_bounds(camera_hz))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            camera_intervals.sort_unstable();
            intervals = intersect_intervals(&intervals, &camera_intervals);
            if intervals.is_empty() {
                break;
            }
        }
        accepted.extend(intervals.into_iter().map(|(first, last)| SampleRange {
            episode_id: range.episode_id.clone(),
            stream_id: range.stream_id.clone(),
            first_index: first,
            last_index: last,
            robot_type: range.robot_type.clone(),
            sample_hz: camera_hz,
        }));
    }
    Ok(accepted)
}

fn camera_bounds_for_joint_range(
    range: &SampleRange,
    camera_hz: f64,
) -> Result<Option<(i64, i64)>, String> {
    let mut first = finite_i64_bound(
        range.first_index as f64 * camera_hz / range.sample_hz,
        f64::floor,
    )?;
    let last_exclusive = range
        .last_index
        .checked_add(1)
        .ok_or_else(|| "joint sample range overflowed while aligning camera frames".to_owned())?;
    let mut last = finite_i64_bound(
        last_exclusive as f64 * camera_hz / range.sample_hz,
        f64::ceil,
    )?;
    while first <= last
        && joint_sample_for_frame(first, camera_hz, range.sample_hz)? < range.first_index
    {
        first = first
            .checked_add(1)
            .ok_or_else(|| "camera frame range overflowed".to_owned())?;
    }
    while first <= last
        && joint_sample_for_frame(last, camera_hz, range.sample_hz)? > range.last_index
    {
        last = last
            .checked_sub(1)
            .ok_or_else(|| "camera frame range overflowed".to_owned())?;
    }
    Ok((first <= last).then_some((first, last)))
}

/// Select the latest non-future joint sample for one camera-frame timestamp.
pub(crate) fn joint_sample_for_frame(
    frame_index: i64,
    camera_hz: f64,
    joint_hz: f64,
) -> Result<i64, String> {
    if frame_index < 0
        || !sample_rate_is_exact(camera_hz, camera_hz)
        || !sample_rate_is_exact(joint_hz, joint_hz)
    {
        return Err(
            "camera-to-joint alignment requires valid non-negative indices and rates".to_owned(),
        );
    }
    finite_i64_bound(frame_index as f64 * joint_hz / camera_hz, f64::floor)
}

/// Whether a raw joint row is selected by one admitted camera-frame anchor.
pub(crate) fn joint_sample_is_frame_aligned(
    ranges: &[SampleRange],
    episode_id: &str,
    stream_id: &str,
    sample_index: i64,
    joint_hz: f64,
) -> Result<bool, String> {
    for range in ranges
        .iter()
        .filter(|range| range.episode_id == episode_id && range.stream_id == stream_id)
    {
        let estimate =
            finite_i64_bound(sample_index as f64 * range.sample_hz / joint_hz, f64::floor)?;
        for candidate in estimate.saturating_sub(1)..=estimate.saturating_add(2) {
            if (range.first_index..=range.last_index).contains(&candidate)
                && joint_sample_for_frame(candidate, range.sample_hz, joint_hz)? == sample_index
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn intersect_intervals(left: &[(i64, i64)], right: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut result = Vec::new();
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        let first = left[left_index].0.max(right[right_index].0);
        let last = left[left_index].1.min(right[right_index].1);
        if first <= last {
            result.push((first, last));
        }
        if left[left_index].1 < right[right_index].1 {
            left_index += 1;
        } else {
            right_index += 1;
        }
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub episode_id: String,
    pub stream_id: String,
    pub frame_index: i64,
    pub sample_hz: f64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AnchorIndex {
    cumulative: Vec<u64>,
    total: u64,
}

impl AnchorIndex {
    pub fn new(ranges: &[SampleRange]) -> Result<Self, String> {
        let mut total = 0_u64;
        let mut cumulative = Vec::with_capacity(ranges.len());
        for range in ranges {
            total = total
                .checked_add(range.count()?)
                .ok_or_else(|| "training population contains too many anchors".to_owned())?;
            cumulative.push(total);
        }
        Ok(Self { cumulative, total })
    }

    pub const fn total(&self) -> u64 {
        self.total
    }

    pub fn select(&self, ranges: &[SampleRange], offset: u64) -> Result<Anchor, String> {
        if ranges.len() != self.cumulative.len() || offset >= self.total {
            return Err("training anchor offset is outside the admitted population".to_owned());
        }
        let range_index = self.cumulative.partition_point(|value| *value <= offset);
        let previous = range_index
            .checked_sub(1)
            .map_or(0, |index| self.cumulative[index]);
        let range = ranges
            .get(range_index)
            .ok_or_else(|| "training anchor index is inconsistent".to_owned())?;
        let within: i64 = (offset - previous)
            .try_into()
            .map_err(|_| "training anchor offset exceeds the sample index range".to_owned())?;
        let frame_index = range
            .first_index
            .checked_add(within)
            .filter(|sample| *sample <= range.last_index)
            .ok_or_else(|| "training anchor index overflowed its admitted range".to_owned())?;
        Ok(Anchor {
            episode_id: range.episode_id.clone(),
            stream_id: range.stream_id.clone(),
            frame_index,
            sample_hz: range.sample_hz,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizationStats {
    pub state_mean: Vec<f32>,
    pub state_std: Vec<f32>,
    pub action_mean: Vec<f32>,
    pub action_std: Vec<f32>,
}

impl NormalizationStats {
    pub fn identity(state: usize, action: usize) -> Self {
        Self {
            state_mean: vec![0.0; state],
            state_std: vec![1.0; state],
            action_mean: vec![0.0; action],
            action_std: vec![1.0; action],
        }
    }
}

#[derive(Clone, Debug)]
struct RunningMoments {
    count: u64,
    mean: Vec<f64>,
    squared_deviation: Vec<f64>,
}

impl RunningMoments {
    fn new(dimensions: usize) -> Result<Self, String> {
        if dimensions == 0 {
            return Err("normalization vectors must have at least one dimension".to_owned());
        }
        Ok(Self {
            count: 0,
            mean: vec![0.0; dimensions],
            squared_deviation: vec![0.0; dimensions],
        })
    }

    fn update(&mut self, values: &[f32]) -> Result<(), String> {
        if values.len() != self.mean.len() {
            return Err("training query mixed vector widths while computing statistics".to_owned());
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err("normalization vector contains a non-finite value".to_owned());
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| "normalization sample count overflowed".to_owned())?;
        let count = self.count as f64;
        for ((mean, squared_deviation), value) in self
            .mean
            .iter_mut()
            .zip(&mut self.squared_deviation)
            .zip(values)
        {
            let value = f64::from(*value);
            let delta = value - *mean;
            *mean += delta / count;
            *squared_deviation += delta * (value - *mean);
            if !mean.is_finite() || !squared_deviation.is_finite() {
                return Err("normalization moments became non-finite".to_owned());
            }
        }
        Ok(())
    }

    fn finish(self, label: &str) -> Result<(Vec<f32>, Vec<f32>), String> {
        if self.count == 0 {
            return Err(format!(
                "the accepted training population has no {label} vectors"
            ));
        }
        let count = self.count as f64;
        let means = self
            .mean
            .into_iter()
            .map(|value| value as f32)
            .collect::<Vec<_>>();
        let standard_deviations = self
            .squared_deviation
            .into_iter()
            .map(|value| (value / count).max(0.0).sqrt() as f32)
            .collect::<Vec<_>>();
        if means
            .iter()
            .chain(&standard_deviations)
            .any(|value| !value.is_finite())
        {
            return Err(format!("the {label} normalization contract is non-finite"));
        }
        Ok((means, standard_deviations))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizationAccumulator {
    state: RunningMoments,
    action: RunningMoments,
}

impl NormalizationAccumulator {
    pub fn new(state_dimensions: usize, action_dimensions: usize) -> Result<Self, String> {
        Ok(Self {
            state: RunningMoments::new(state_dimensions)?,
            action: RunningMoments::new(action_dimensions)?,
        })
    }

    pub fn observe(&mut self, state: &[f32], action: &[f32]) -> Result<(), String> {
        self.state.update(state)?;
        self.action.update(action)
    }

    pub fn finish(self) -> Result<NormalizationStats, String> {
        let (state_mean, state_std) = self.state.finish("state")?;
        let (action_mean, action_std) = self.action.finish("action")?;
        Ok(NormalizationStats {
            state_mean,
            state_std,
            action_mean,
            action_std,
        })
    }
}

pub(crate) fn normalize_vectors(
    values: &mut [f32],
    means: &[f32],
    standard_deviations: &[f32],
) -> Result<(), String> {
    const EPSILON: f32 = 1.0e-8;
    if means.is_empty()
        || means.len() != standard_deviations.len()
        || !values.len().is_multiple_of(means.len())
        || means.iter().any(|value| !value.is_finite())
        || standard_deviations
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("normalization contract has invalid dimensions or values".to_owned());
    }
    for vector in values.chunks_exact_mut(means.len()) {
        for ((value, mean), standard_deviation) in
            vector.iter_mut().zip(means).zip(standard_deviations)
        {
            if !value.is_finite() {
                return Err("normalization input contains a non-finite value".to_owned());
            }
            *value = (*value - mean) / (*standard_deviation + EPSILON);
            if !value.is_finite() {
                return Err("normalization output became non-finite".to_owned());
            }
        }
    }
    Ok(())
}

pub(crate) fn flatten_action_chunks(
    actions: Vec<Vec<Option<Vec<f32>>>>,
    dimensions: usize,
) -> Result<Vec<f32>, String> {
    if dimensions == 0 {
        return Err("action vectors must have at least one dimension".to_owned());
    }
    let slots = actions.iter().try_fold(0_usize, |total, row| {
        total
            .checked_add(row.len())
            .ok_or_else(|| "action chunk size overflowed".to_owned())
    })?;
    let capacity = slots
        .checked_mul(dimensions)
        .ok_or_else(|| "action chunk capacity overflowed".to_owned())?;
    let mut result = Vec::with_capacity(capacity);
    for row in actions {
        let mut last = None;
        for value in row {
            match value {
                Some(value)
                    if value.len() == dimensions && value.iter().all(|value| value.is_finite()) =>
                {
                    result.extend_from_slice(&value);
                    last = Some(value);
                }
                Some(value) if value.len() != dimensions => {
                    return Err("training query mixed action vector widths".to_owned());
                }
                Some(_) => return Err("action vector contains a non-finite value".to_owned()),
                None => result.extend_from_slice(last.as_deref().ok_or_else(|| {
                    "sampled action chunk is missing its anchor action".to_owned()
                })?),
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: &str, first: i64, last: i64, hz: f64) -> PopulationRow {
        PopulationRow {
            episode_id: episode.to_owned(),
            stream_id: "follower".to_owned(),
            first_index: first,
            last_index: last,
            row_count: last - first + 1,
            sample_hz_min: hz,
            sample_hz_max: hz,
            episode_frames_min: last + 1,
            episode_frames_max: last + 1,
            robot_type_min: None,
            robot_type_max: None,
        }
    }

    fn range(episode: &str, first: i64, last: i64) -> SampleRange {
        SampleRange {
            episode_id: episode.to_owned(),
            stream_id: "follower".to_owned(),
            first_index: first,
            last_index: last,
            robot_type: "so101_follower".to_owned(),
            sample_hz: 60.0,
        }
    }

    #[test]
    fn population_admission_derives_one_exact_rate_and_legacy_robot_contract() {
        let population =
            admit_population(vec![row("one", 0, 2, 60.0), row("two", 4, 8, 60.0)]).unwrap();
        assert_eq!(population.sample_hz, 60.0);
        assert_eq!(population.ranges[0].robot_type, "so101_follower");
        assert_eq!(population.ranges[1].count().unwrap(), 5);
    }

    #[test]
    fn validation_split_is_deterministic_disjoint_and_episode_level() {
        let mut ranges = (0..10)
            .map(|index| range(&format!("episode-{index}"), 0, 9))
            .collect::<Vec<_>>();
        ranges.push(range("episode-3", 20, 29));

        let (training, validation) = split_episode_ranges(ranges.clone(), 0.2, 42).unwrap();
        let (training_again, validation_again) = split_episode_ranges(ranges, 0.2, 42).unwrap();
        assert_eq!(training, training_again);
        assert_eq!(validation, validation_again);

        let training_episodes = training
            .iter()
            .map(|range| range.episode_id.as_str())
            .collect::<BTreeSet<_>>();
        let validation_episodes = validation
            .iter()
            .map(|range| range.episode_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(validation_episodes.len(), 2);
        assert!(training_episodes.is_disjoint(&validation_episodes));
        assert_eq!(training.len() + validation.len(), 11);
    }

    #[test]
    fn validation_split_keeps_small_or_disabled_populations_trainable() {
        let only = vec![range("only", 0, 9)];
        assert_eq!(
            split_episode_ranges(only.clone(), 0.1, 7).unwrap(),
            (only.clone(), Vec::new())
        );
        assert_eq!(
            split_episode_ranges(only.clone(), 0.0, 7).unwrap(),
            (only, Vec::new())
        );
        assert!(split_episode_ranges(Vec::new(), f64::NAN, 7).is_err());
        assert!(split_episode_ranges(Vec::new(), 0.51, 7).is_err());
    }

    #[test]
    fn population_admission_fails_closed_on_rates_gaps_duplicates_and_overflow() {
        let mut invalid_rate = row("one", 0, 2, 60.0);
        invalid_rate.sample_hz_max = f64::NAN;
        assert!(admit_population(vec![invalid_rate]).is_err());
        assert!(admit_population(vec![row("one", 0, 2, 60.0), row("two", 0, 2, 30.0)]).is_err());

        let mut gap = row("one", 0, 2, 60.0);
        gap.row_count = 2;
        assert!(admit_population(vec![gap]).is_err());
        assert!(admit_population(vec![row("one", 0, 2, 60.0), row("one", 0, 2, 60.0)]).is_err());

        let mut overflow = row("one", 0, 0, 60.0);
        overflow.last_index = i64::MAX;
        overflow.row_count = i64::MAX;
        overflow.episode_frames_min = i64::MAX;
        overflow.episode_frames_max = i64::MAX;
        assert!(admit_population(vec![overflow]).is_err());
    }

    #[test]
    fn population_admits_the_episode_frame_ceiling_and_refuses_one_more() {
        let at_ceiling = row("one", 0, MAX_TRAINING_EPISODE_FRAMES - 1, 60.0);
        let population = admit_population(vec![at_ceiling]).unwrap();
        assert_eq!(
            population.ranges[0].count().unwrap(),
            MAX_TRAINING_EPISODE_FRAMES as u64
        );

        let beyond = row("one", 0, MAX_TRAINING_EPISODE_FRAMES, 60.0);
        let error = admit_population(vec![beyond]).unwrap_err();
        assert!(error.contains("1–1000000"));
    }

    #[test]
    fn stream_contracts_are_complete_unique_and_identical() {
        let ranges = vec![range("one", 0, 2), range("two", 0, 2)];
        let contract = |episode: &str, joints: &[&str]| StreamContractRow {
            episode_id: episode.to_owned(),
            stream_id: "follower".to_owned(),
            robot_type: Some("so101_follower".to_owned()),
            joint_names: joints.iter().map(|joint| (*joint).to_owned()).collect(),
        };
        let canonical = validate_stream_contracts(
            &ranges,
            vec![
                contract("one", &["shoulder", "elbow"]),
                contract("two", &["shoulder", "elbow"]),
            ],
        )
        .unwrap();
        assert_eq!(canonical.joint_names, ["shoulder", "elbow"]);
        assert!(validate_stream_contracts(&ranges, vec![contract("one", &["shoulder"])]).is_err());
        assert!(
            validate_stream_contracts(
                &ranges,
                vec![contract("one", &["shoulder"]), contract("two", &["elbow"])]
            )
            .is_err()
        );
    }

    #[test]
    fn camera_frames_own_act_anchors_and_stride_joint_targets() {
        let ranges = vec![range("one", 0, 19)];
        let mut cameras = BTreeMap::new();
        for camera in ["top", "wrist"] {
            cameras.insert(
                ("one".to_owned(), camera.to_owned()),
                vec![CameraSegment::new(0, 30, 10).unwrap()],
            );
        }
        let selected = ["top".to_owned(), "wrist".to_owned()];
        let camera_hz = camera_sample_hz(&ranges, &selected, &cameras).unwrap();
        let accepted =
            ranges_with_camera_frame_coverage(&ranges, &selected, &cameras, camera_hz).unwrap();

        assert_eq!(camera_hz, 30.0);
        assert_eq!(accepted.len(), 1);
        assert_eq!((accepted[0].first_index, accepted[0].last_index), (0, 9));
        assert_eq!(accepted[0].sample_hz, 30.0);
        assert_eq!(joint_sample_for_frame(0, camera_hz, 60.0).unwrap(), 0);
        assert_eq!(joint_sample_for_frame(1, camera_hz, 60.0).unwrap(), 2);
        assert_eq!(joint_sample_for_frame(9, camera_hz, 60.0).unwrap(), 18);
        assert!(joint_sample_is_frame_aligned(&accepted, "one", "follower", 18, 60.0).unwrap());
        assert!(!joint_sample_is_frame_aligned(&accepted, "one", "follower", 19, 60.0).unwrap());
    }

    #[test]
    fn camera_policy_clock_rejects_mixed_fps_and_slower_joint_streams() {
        let ranges = vec![range("one", 0, 19)];
        let selected = ["top".to_owned(), "wrist".to_owned()];
        let mut cameras = BTreeMap::new();
        cameras.insert(
            ("one".to_owned(), "top".to_owned()),
            vec![CameraSegment::new(0, 30, 10).unwrap()],
        );
        cameras.insert(
            ("one".to_owned(), "wrist".to_owned()),
            vec![CameraSegment::new(0, 24, 10).unwrap()],
        );
        assert!(camera_sample_hz(&ranges, &selected, &cameras).is_err());

        cameras.insert(
            ("one".to_owned(), "wrist".to_owned()),
            vec![CameraSegment::new(0, 30, 10).unwrap()],
        );
        let mut slow = range("one", 0, 19);
        slow.sample_hz = 20.0;
        assert!(ranges_with_camera_frame_coverage(&[slow], &selected, &cameras, 30.0).is_err());
    }

    #[test]
    fn common_camera_selection_is_dataset_wide_and_deterministic() {
        let ranges = vec![range("one", 0, 2), range("two", 0, 2)];
        let segment = || vec![CameraSegment::new(0, 30, 10).unwrap()];
        let mut cameras = BTreeMap::new();
        for episode in ["one", "two"] {
            cameras.insert((episode.to_owned(), "top".to_owned()), segment());
            cameras.insert((episode.to_owned(), "wrist".to_owned()), segment());
        }
        cameras.insert(("one".to_owned(), "side".to_owned()), segment());
        assert_eq!(common_camera_streams(&cameras, &ranges), ["top", "wrist"]);
    }

    #[test]
    fn camera_alignment_rejects_invalid_rates_and_index_overflow() {
        let segment = CameraSegment::new(i64::MAX, 30, 10).unwrap();
        assert!(segment.sample_bounds(f64::MAX).is_err());
        assert!(segment.frame_for(0, f64::NAN).is_err());
        assert!(CameraSegment::new(0, 0, 10).is_err());
    }

    #[test]
    fn weighted_anchor_index_selects_every_boundary_without_bias() {
        let ranges = vec![range("one", 10, 11), range("two", 20, 22)];
        let index = AnchorIndex::new(&ranges).unwrap();
        assert_eq!(index.total(), 5);
        assert_eq!(index.select(&ranges, 0).unwrap().frame_index, 10);
        assert_eq!(index.select(&ranges, 1).unwrap().frame_index, 11);
        assert_eq!(index.select(&ranges, 2).unwrap().frame_index, 20);
        assert_eq!(index.select(&ranges, 4).unwrap().frame_index, 22);
        assert!(index.select(&ranges, 5).is_err());
    }

    #[test]
    fn normalization_uses_population_statistics_and_refuses_bad_vectors() {
        let mut accumulator = NormalizationAccumulator::new(2, 2).unwrap();
        accumulator.observe(&[1.0, 10.0], &[2.0, 20.0]).unwrap();
        accumulator.observe(&[3.0, 14.0], &[4.0, 24.0]).unwrap();
        let stats = accumulator.finish().unwrap();
        assert_eq!(stats.state_mean, [2.0, 12.0]);
        assert_eq!(stats.state_std, [1.0, 2.0]);

        let mut invalid = NormalizationAccumulator::new(1, 1).unwrap();
        assert!(invalid.observe(&[f32::NAN], &[1.0]).is_err());
        assert!(invalid.observe(&[1.0, 2.0], &[1.0]).is_err());
    }

    #[test]
    fn normalization_and_action_padding_validate_their_complete_layout() {
        let mut values = vec![2.0, 4.0, 2.0, 8.0];
        normalize_vectors(&mut values, &[2.0, 6.0], &[0.0, 2.0]).unwrap();
        assert_eq!(values, [0.0, -1.0, 0.0, 1.0]);
        assert!(normalize_vectors(&mut [1.0, 2.0, 3.0], &[0.0, 0.0], &[1.0, 1.0]).is_err());

        let values = flatten_action_chunks(
            vec![vec![Some(vec![1.0, 2.0]), Some(vec![3.0, 4.0]), None]],
            2,
        )
        .unwrap();
        assert_eq!(values, [1.0, 2.0, 3.0, 4.0, 3.0, 4.0]);
        assert!(flatten_action_chunks(vec![vec![None]], 2).is_err());
    }
}
