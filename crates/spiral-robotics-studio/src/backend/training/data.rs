//! Spiral ADBC to Burn ACT batch preparation.
//!
//! Video frame selection, decode, optional resize, CHW conversion, and
//! ImageNet normalization remain SPQL operations. This module also derives
//! the dataset state/action normalization contract before packing finite host
//! buffers for Burn.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use adbc_core::{Connection as _, Statement as _};
use adbc_driver_manager::ManagedConnection;
use arrow_array::{
    Array, ArrayRef, BooleanArray, FixedSizeListArray, Float16Array, Float32Array, Float64Array,
    Int8Array, Int16Array, Int32Array, Int64Array, LargeListArray, LargeStringArray, ListArray,
    RecordBatch, StringArray, StringViewArray, StructArray, UInt8Array, UInt16Array, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use spiral_robotics_contracts::api::StartTrainingRequest;
use spiral_robotics_contracts::spiral;
use spiral_robotics_inference::act::VisionActBatch;

use crate::backend::application::training::MAX_TRAINING_IMAGE_BATCH_BYTES;
use crate::backend::application::training_population::{
    self as population, Anchor, AnchorIndex, CameraSegment, NormalizationAccumulator,
    NormalizationStats, PopulationRow, SampleRange, StreamContract, StreamContractRow,
    TrainingPopulation, flatten_action_chunks, normalize_vectors,
};

const MAX_MANIFEST_ROWS: usize = 8_192;

#[derive(Clone, Debug)]
struct VideoSegment {
    segment_index: u32,
    timing: CameraSegment,
    width: usize,
    height: usize,
    media_relation: String,
}

#[derive(Clone, Debug)]
struct ImageCoordinate {
    batch_index: usize,
    camera_index: usize,
    episode_id: String,
    camera_stream_id: String,
    segment_index: u32,
    frame_index: u64,
    media_relation: String,
}

#[derive(Clone, Debug)]
struct FrameGroup {
    episode_id: String,
    camera_stream_id: String,
    segment_index: u32,
    media_relation: String,
    frame_indices: Vec<u64>,
    destinations: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LoaderMetrics {
    pub seconds: f64,
    pub source_bytes: u64,
    pub requested_frames: usize,
}

pub(crate) struct LoadedActBatch {
    pub batch: VisionActBatch,
    pub metrics: LoaderMetrics,
}

type ActionBatch = (Vec<f32>, Vec<f32>, Vec<bool>);

pub(crate) struct SpiralActLoader<'a> {
    connection: &'a mut ManagedConnection,
    base_sql: String,
    dataset: String,
    state_column: String,
    action_column: String,
    camera_streams: Vec<String>,
    chunk_size: usize,
    image_width: usize,
    image_height: usize,
    resize_images: bool,
    video_decoder: String,
    train_ranges: Vec<SampleRange>,
    validation_ranges: Vec<SampleRange>,
    train_anchors: AnchorIndex,
    validation_anchors: AnchorIndex,
    video_segments: BTreeMap<(String, String), Vec<VideoSegment>>,
    train_random: SplitMix64,
    validation_random: SplitMix64,
    pub stats: NormalizationStats,
    pub joint_names: Vec<String>,
    pub robot_type: String,
    pub sample_hz: f64,
}

impl<'a> SpiralActLoader<'a> {
    pub(crate) fn open(
        connection: &'a mut ManagedConnection,
        request: &StartTrainingRequest,
        dataset: &str,
    ) -> Result<Self, String> {
        let base_sql = clean_query(&request.sql)?;
        if !spiral::is_identifier(&request.state_column)
            || !spiral::is_identifier(&request.action_column)
            || !spiral::is_identifier(dataset)
        {
            return Err(
                "training query, columns, or dataset contain an invalid identifier".to_owned(),
            );
        }
        let mut loader = Self {
            connection,
            base_sql,
            dataset: dataset.to_owned(),
            state_column: request.state_column.clone(),
            action_column: request.action_column.clone(),
            camera_streams: request.act.camera_streams.clone(),
            chunk_size: request.chunk_size,
            image_width: request.act.image_width,
            image_height: request.act.image_height,
            resize_images: request.act.image_width != 0,
            video_decoder: request.act.video_decoder.clone(),
            train_ranges: Vec::new(),
            validation_ranges: Vec::new(),
            train_anchors: AnchorIndex::default(),
            validation_anchors: AnchorIndex::default(),
            video_segments: BTreeMap::new(),
            train_random: SplitMix64::new(request.seed),
            validation_random: SplitMix64::new(request.seed ^ 0xA17C_9E3D),
            stats: NormalizationStats::identity(1, 1),
            joint_names: Vec::new(),
            robot_type: String::new(),
            sample_hz: 0.0,
        };
        let population = loader.load_population()?;
        loader.sample_hz = population.sample_hz;
        let ranges = population.ranges;
        let contract = loader.load_stream_contracts(&ranges)?;
        loader.video_segments = loader.load_video_segments(&ranges)?;
        let camera_coverage = loader.camera_coverage();
        if loader.camera_streams.is_empty() {
            loader.camera_streams = population::common_camera_streams(&camera_coverage, &ranges);
            if loader.camera_streams.is_empty() {
                return Err(
                    "the selected dataset episodes do not share a recorded camera stream"
                        .to_owned(),
                );
            }
            if loader.camera_streams.len() > 8 {
                return Err(format!(
                    "ACT supports at most eight camera streams, but the selected dataset has {}",
                    loader.camera_streams.len()
                ));
            }
        }
        let ranges = population::ranges_with_camera_coverage(
            &ranges,
            &loader.camera_streams,
            &camera_coverage,
        )?;
        loader.resolve_image_dimensions(&ranges)?;
        validate_image_batch_size(
            request.batch_size,
            loader.camera_streams.len(),
            loader.image_width,
            loader.image_height,
        )?;
        loader.train_ranges = ranges;
        // LeRobot's dataset factory defaults eval_split to 0.0. Studio does
        // not expose an explicit split yet, so every covered range trains.
        loader.validation_ranges = Vec::new();
        if loader.train_ranges.is_empty() {
            return Err(
                "the query has no training samples with complete synchronized camera coverage"
                    .to_owned(),
            );
        }
        loader.train_anchors = AnchorIndex::new(&loader.train_ranges)?;
        loader.validation_anchors = AnchorIndex::new(&loader.validation_ranges)?;
        let (state_dim, action_dim) =
            loader.load_vector_contract(&loader.train_ranges[0].clone())?;
        population::validate_vector_contract(state_dim, action_dim, &contract)?;
        loader.stats = loader.load_normalization_stats(state_dim, action_dim)?;
        loader.joint_names = contract.joint_names.clone();
        loader.robot_type = contract.robot_type.clone();
        Ok(loader)
    }

    pub(crate) fn state_dimensions(&self) -> usize {
        self.stats.state_mean.len()
    }

    pub(crate) fn action_dimensions(&self) -> usize {
        self.stats.action_mean.len()
    }

    pub(crate) fn image_width(&self) -> usize {
        self.image_width
    }

    pub(crate) fn image_height(&self) -> usize {
        self.image_height
    }

    pub(crate) fn camera_streams(&self) -> &[String] {
        &self.camera_streams
    }

    pub(crate) fn has_validation(&self) -> bool {
        !self.validation_ranges.is_empty()
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "{} train anchors, {} validation anchors, {} camera(s)",
            self.train_anchors.total(),
            self.validation_anchors.total(),
            self.camera_streams.len()
        )
    }

    pub(crate) fn fetch(
        &mut self,
        batch_size: usize,
        validation: bool,
    ) -> Result<LoadedActBatch, String> {
        if batch_size
            .checked_mul(self.chunk_size)
            .is_none_or(|rows| rows > MAX_MANIFEST_ROWS)
        {
            return Err(format!(
                "ACT action manifest exceeds the supported maximum of {MAX_MANIFEST_ROWS} rows"
            ));
        }
        let started = Instant::now();
        let anchors = self.sample_anchors(validation, batch_size)?;
        let (states, actions, action_is_pad) = self.fetch_actions(&anchors)?;
        let (images, source_bytes) = self.fetch_images(&anchors)?;
        let requested_frames = batch_size
            .checked_mul(self.camera_streams.len())
            .ok_or_else(|| "requested camera frame count overflowed".to_owned())?;
        Ok(LoadedActBatch {
            batch: VisionActBatch {
                batch_size,
                states,
                images,
                actions,
                action_is_pad,
            },
            metrics: LoaderMetrics {
                seconds: started.elapsed().as_secs_f64(),
                source_bytes,
                requested_frames,
            },
        })
    }

    fn load_population(&mut self) -> Result<TrainingPopulation, String> {
        let query = format!(
            "WITH training_rows AS ({})\n\
             SELECT episode_id, stream_id, MIN(sample_index) AS first_index, \
             MAX(sample_index) AS last_index, COUNT(*) AS row_count, \
             MIN(sample_hz) AS sample_hz_min, MAX(sample_hz) AS sample_hz_max, \
             MIN(episode_frames) AS episode_frames_min, \
             MAX(episode_frames) AS episode_frames_max, \
             MIN(robot_type) AS robot_type_min, MAX(robot_type) AS robot_type_max \
             FROM training_rows GROUP BY episode_id, stream_id",
            self.base_sql
        );
        let batches = query_batches(self.connection, &query)?;
        let mut rows = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                rows.push(PopulationRow {
                    episode_id: string_value(required(batch, "episode_id")?, row)?,
                    stream_id: string_value(required(batch, "stream_id")?, row)?,
                    first_index: integer_value(required(batch, "first_index")?, row)?,
                    last_index: integer_value(required(batch, "last_index")?, row)?,
                    row_count: integer_value(required(batch, "row_count")?, row)?,
                    sample_hz_min: float_value(required(batch, "sample_hz_min")?, row)?,
                    sample_hz_max: float_value(required(batch, "sample_hz_max")?, row)?,
                    episode_frames_min: integer_value(required(batch, "episode_frames_min")?, row)?,
                    episode_frames_max: integer_value(required(batch, "episode_frames_max")?, row)?,
                    robot_type_min: optional_string(required(batch, "robot_type_min")?, row)?,
                    robot_type_max: optional_string(required(batch, "robot_type_max")?, row)?,
                });
            }
        }
        population::admit_population(rows)
    }

    fn load_stream_contracts(&mut self, ranges: &[SampleRange]) -> Result<StreamContract, String> {
        let query = format!(
            "SELECT episode_id, stream_id, robot_type, joint_names FROM {}.episode_streams WHERE modality = 'joint'",
            self.dataset
        );
        let batches = query_batches(self.connection, &query)?;
        let mut rows = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                rows.push(StreamContractRow {
                    episode_id: string_value(required(batch, "episode_id")?, row)?,
                    stream_id: string_value(required(batch, "stream_id")?, row)?,
                    robot_type: optional_string(required(batch, "robot_type")?, row)?,
                    joint_names: string_list(required(batch, "joint_names")?, row)?,
                });
            }
        }
        population::validate_stream_contracts(ranges, rows)
    }

    fn load_video_segments(
        &mut self,
        ranges: &[SampleRange],
    ) -> Result<BTreeMap<(String, String), Vec<VideoSegment>>, String> {
        let camera_filter = if self.camera_streams.is_empty() {
            String::new()
        } else {
            let cameras = self
                .camera_streams
                .iter()
                .map(|camera| sql_string(camera))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" WHERE stream_id IN ({cameras})")
        };
        let query = format!(
            "SELECT episode_id, stream_id AS camera_stream_id, segment_index, \
             timeline_offset_ns, fps, frames, width, height, media_relation FROM {}.video_assets \
             {camera_filter}",
            self.dataset,
        );
        let wanted = ranges
            .iter()
            .map(|range| range.episode_id.as_str())
            .collect::<BTreeSet<_>>();
        let batches = query_batches(self.connection, &query)?;
        let mut segments: BTreeMap<(String, String), Vec<VideoSegment>> = BTreeMap::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                let episode_id = string_value(required(batch, "episode_id")?, row)?;
                if !wanted.contains(episode_id.as_str()) {
                    continue;
                }
                let camera_stream_id = string_value(required(batch, "camera_stream_id")?, row)?;
                let media_relation = string_value(required(batch, "media_relation")?, row)?;
                validate_relation(&media_relation)?;
                let timeline_offset_ns =
                    integer_value(required(batch, "timeline_offset_ns")?, row)?;
                let fps = integer_value(required(batch, "fps")?, row)?
                    .try_into()
                    .map_err(|_| "camera fps must be positive".to_owned())?;
                let frames = integer_value(required(batch, "frames")?, row)?
                    .try_into()
                    .map_err(|_| "camera frame count must be positive".to_owned())?;
                let segment = VideoSegment {
                    segment_index: integer_value(required(batch, "segment_index")?, row)?
                        .try_into()
                        .map_err(|_| "camera segment index is outside u32".to_owned())?,
                    timing: CameraSegment::new(timeline_offset_ns, fps, frames)?,
                    width: integer_value(required(batch, "width")?, row)?
                        .try_into()
                        .map_err(|_| "camera width must be positive".to_owned())?,
                    height: integer_value(required(batch, "height")?, row)?
                        .try_into()
                        .map_err(|_| "camera height must be positive".to_owned())?,
                    media_relation,
                };
                if segment.width == 0 || segment.height == 0 {
                    return Err("camera segment has an invalid video contract".to_owned());
                }
                segments
                    .entry((episode_id, camera_stream_id))
                    .or_default()
                    .push(segment);
            }
        }
        for (key, values) in &mut segments {
            values
                .sort_by_key(|segment| (segment.timing.timeline_offset_ns, segment.segment_index));
            let timings = values
                .iter()
                .map(|segment| segment.timing.clone())
                .collect::<Vec<_>>();
            population::ensure_non_overlapping_camera_segments(key, &timings)?;
        }
        Ok(segments)
    }

    fn camera_coverage(&self) -> BTreeMap<(String, String), Vec<CameraSegment>> {
        self.video_segments
            .iter()
            .map(|(key, segments)| {
                (
                    key.clone(),
                    segments
                        .iter()
                        .map(|segment| segment.timing.clone())
                        .collect(),
                )
            })
            .collect()
    }

    fn resolve_image_dimensions(&mut self, ranges: &[SampleRange]) -> Result<(), String> {
        if self.resize_images {
            return Ok(());
        }
        let mut dimensions = BTreeSet::new();
        for range in ranges {
            for camera in &self.camera_streams {
                for segment in self
                    .video_segments
                    .get(&(range.episode_id.clone(), camera.clone()))
                    .into_iter()
                    .flatten()
                {
                    if segment
                        .timing
                        .sample_bounds(range.sample_hz)?
                        .is_some_and(|bounds| {
                            bounds.0 <= range.last_index && bounds.1 >= range.first_index
                        })
                    {
                        dimensions.insert((segment.width, segment.height));
                    }
                }
            }
        }
        let Some(&(width, height)) = dimensions.iter().next() else {
            return Err("training cameras have no recorded image dimensions".to_owned());
        };
        if dimensions.len() != 1 {
            return Err(format!(
                "native-resolution ACT requires one camera image size, found {dimensions:?}; set an explicit training width and height to resize"
            ));
        }
        self.image_width = width;
        self.image_height = height;
        Ok(())
    }

    fn load_vector_contract(&mut self, range: &SampleRange) -> Result<(usize, usize), String> {
        let query = format!(
            "WITH training_rows AS ({}) \
             SELECT {} AS state_value, {} AS action_value FROM training_rows \
             WHERE episode_id = {} AND stream_id = {} AND sample_index = {}",
            self.base_sql,
            self.state_column,
            self.action_column,
            sql_string(&range.episode_id),
            sql_string(&range.stream_id),
            range.first_index
        );
        let batches = query_batches(self.connection, &query)?;
        let mut rows = Vec::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                rows.push((
                    numeric_vector(required(batch, "state_value")?, row)?,
                    numeric_vector(required(batch, "action_value")?, row)?,
                ));
            }
        }
        if rows.len() != 1 || rows[0].0.is_empty() || rows[0].1.is_empty() {
            return Err("sampled stream did not resolve to one vector contract row".to_owned());
        }
        Ok((rows[0].0.len(), rows[0].1.len()))
    }

    fn load_normalization_stats(
        &mut self,
        state_dimensions: usize,
        action_dimensions: usize,
    ) -> Result<NormalizationStats, String> {
        let query = format!(
            "WITH training_rows AS ({}) \
             SELECT episode_id, stream_id, sample_index, {} AS state_value, \
             {} AS action_value FROM training_rows",
            self.base_sql, self.state_column, self.action_column
        );
        let batches = query_batches(self.connection, &query)?;
        let mut normalization = NormalizationAccumulator::new(state_dimensions, action_dimensions)?;
        for batch in &batches {
            for row in 0..batch.num_rows() {
                let episode_id = string_value(required(batch, "episode_id")?, row)?;
                let stream_id = string_value(required(batch, "stream_id")?, row)?;
                let sample_index = integer_value(required(batch, "sample_index")?, row)?;
                if !population::sample_is_eligible(
                    &self.train_ranges,
                    &episode_id,
                    &stream_id,
                    sample_index,
                ) && !population::sample_is_eligible(
                    &self.validation_ranges,
                    &episode_id,
                    &stream_id,
                    sample_index,
                ) {
                    continue;
                }
                let state = numeric_vector(required(batch, "state_value")?, row)?;
                let action = numeric_vector(required(batch, "action_value")?, row)?;
                normalization.observe(&state, &action)?;
            }
        }
        normalization.finish()
    }

    fn sample_anchors(
        &mut self,
        validation: bool,
        batch_size: usize,
    ) -> Result<Vec<Anchor>, String> {
        let (ranges, index, random, split) = if validation {
            (
                &self.validation_ranges,
                &self.validation_anchors,
                &mut self.validation_random,
                "validation",
            )
        } else {
            (
                &self.train_ranges,
                &self.train_anchors,
                &mut self.train_random,
                "training",
            )
        };
        let total = index.total();
        if total == 0 {
            return Err(format!("the episode split contains no {split} anchors"));
        }
        let mut result = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            let offset = random.range(total);
            result.push(index.select(ranges, offset)?);
        }
        Ok(result)
    }

    fn fetch_actions(&mut self, anchors: &[Anchor]) -> Result<ActionBatch, String> {
        let requests = anchors
            .iter()
            .enumerate()
            .flat_map(|(request, anchor)| {
                (0..self.chunk_size).map(move |offset| {
                    format!(
                        "({request}, {offset}, {}, {}, {})",
                        sql_string(&anchor.episode_id),
                        sql_string(&anchor.stream_id),
                        anchor.sample_index + offset as i64
                    )
                })
            })
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "WITH training_rows AS ({}), \
             requested(request_index, chunk_offset, episode_id, stream_id, sample_index) AS \
             (VALUES {requests}) \
             SELECT r.request_index, r.chunk_offset, j.sample_index IS NOT NULL AS present, \
             j.{} AS state_value, j.{} AS action_value FROM requested AS r \
             LEFT JOIN training_rows AS j ON j.episode_id = r.episode_id \
             AND j.stream_id = r.stream_id \
             AND CAST(j.sample_index AS INT64) = CAST(r.sample_index AS INT64) \
             ORDER BY r.request_index, r.chunk_offset",
            self.base_sql, self.state_column, self.action_column
        );
        let batches = query_batches(self.connection, &query)?;
        let batch_size = anchors.len();
        let mut states = vec![None; batch_size];
        let mut actions = vec![vec![None; self.chunk_size]; batch_size];
        let mut padding = vec![true; batch_size * self.chunk_size];
        let mut received = BTreeSet::new();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                let request: usize = integer_value(required(batch, "request_index")?, row)?
                    .try_into()
                    .map_err(|_| "invalid action request index".to_owned())?;
                let offset: usize = integer_value(required(batch, "chunk_offset")?, row)?
                    .try_into()
                    .map_err(|_| "invalid action chunk offset".to_owned())?;
                if request >= batch_size
                    || offset >= self.chunk_size
                    || !received.insert((request, offset))
                {
                    return Err(
                        "Spiral returned an invalid or duplicate action coordinate".to_owned()
                    );
                }
                let present = boolean_value(required(batch, "present")?, row)?;
                if offset == 0 {
                    if !present {
                        return Err("Spiral did not return the sampled anchor state".to_owned());
                    }
                    states[request] = Some(numeric_vector(required(batch, "state_value")?, row)?);
                }
                if present {
                    actions[request][offset] =
                        Some(numeric_vector(required(batch, "action_value")?, row)?);
                    padding[request * self.chunk_size + offset] = false;
                }
            }
        }
        if received.len() != batch_size * self.chunk_size || states.iter().any(Option::is_none) {
            return Err("Spiral returned an incomplete action manifest".to_owned());
        }
        let mut state_values = Vec::with_capacity(batch_size * self.state_dimensions());
        for state in states.into_iter().flatten() {
            if state.len() != self.state_dimensions() {
                return Err("training query mixed state vector widths".to_owned());
            }
            state_values.extend(state);
        }
        normalize_vectors(
            &mut state_values,
            &self.stats.state_mean,
            &self.stats.state_std,
        )?;
        let mut action_values = flatten_action_chunks(actions, self.action_dimensions())?;
        normalize_vectors(
            &mut action_values,
            &self.stats.action_mean,
            &self.stats.action_std,
        )?;
        Ok((state_values, action_values, padding))
    }

    fn fetch_images(&mut self, anchors: &[Anchor]) -> Result<(Vec<f32>, u64), String> {
        let coordinates = self.image_coordinates(anchors)?;
        let groups = frame_groups(&coordinates);
        let image_values = 3_usize
            .checked_mul(self.image_height)
            .and_then(|values| values.checked_mul(self.image_width))
            .ok_or_else(|| "ACT image element count overflowed".to_owned())?;
        let image_elements = validate_image_batch_size(
            anchors.len(),
            self.camera_streams.len(),
            self.image_width,
            self.image_height,
        )?;
        let mut images = Vec::new();
        images
            .try_reserve_exact(image_elements)
            .map_err(|error| format!("could not reserve ACT image batch: {error}"))?;
        images.resize(image_elements, 0.0);
        let mut seen = BTreeSet::new();
        for (group_index, group) in groups.iter().enumerate() {
            let indices = group
                .frame_indices
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let image_expression = if self.resize_images {
                format!(
                    "spql.image.to_tensor(spql.image.resize(spql.video.decode(f.frame, {}), $1, $2, 'bilinear'), 'chw', 'imagenet')",
                    sql_string(&self.video_decoder)
                )
            } else {
                format!(
                    "spql.image.to_tensor(spql.video.decode(f.frame, {}), 'chw', 'imagenet')",
                    sql_string(&self.video_decoder)
                )
            };
            let query = format!(
                "SELECT f.request_index, \
                 {image_expression} AS image \
                 FROM {} AS a CROSS JOIN LATERAL \
                 spql.video.frames(a.media, spql.video.frame_indices(ARRAY[{indices}])) AS f \
                 WHERE a.episode_id = {} AND a.stream_id = {} AND a.segment_index = {} \
                 ORDER BY f.request_index",
                group.media_relation,
                sql_string(&group.episode_id),
                sql_string(&group.camera_stream_id),
                group.segment_index,
            );
            let batches = if self.resize_images {
                query_image_batches(self.connection, &query, self.image_width, self.image_height)?
            } else {
                query_batches(self.connection, &query)?
            };
            for batch in &batches {
                for row in 0..batch.num_rows() {
                    let request: usize = integer_value(required(batch, "request_index")?, row)?
                        .try_into()
                        .map_err(|_| "invalid decoded-frame request index".to_owned())?;
                    let destination = group.destinations.get(request).ok_or_else(|| {
                        "Spiral returned an invalid decoded-frame coordinate".to_owned()
                    })?;
                    if !seen.insert((group_index, request)) {
                        return Err(
                            "Spiral returned a duplicate decoded-frame coordinate".to_owned()
                        );
                    }
                    let (values, shape) = tensor_value(required(batch, "image")?, row)?;
                    if shape != [3, self.image_height, self.image_width]
                        || values.len() != image_values
                    {
                        return Err(format!(
                            "Spiral returned image shape {shape:?}, expected [3, {}, {}]",
                            self.image_height, self.image_width
                        ));
                    }
                    let destination =
                        (destination.0 * self.camera_streams.len() + destination.1) * image_values;
                    images[destination..destination + image_values].copy_from_slice(&values);
                }
            }
        }
        if seen.len() != coordinates.len() {
            return Err("Spiral did not decode every requested image tensor".to_owned());
        }
        let bytes = images
            .len()
            .checked_mul(std::mem::size_of::<f32>())
            .and_then(|bytes| bytes.try_into().ok())
            .unwrap_or(u64::MAX);
        Ok((images, bytes))
    }

    fn image_coordinates(&self, anchors: &[Anchor]) -> Result<Vec<ImageCoordinate>, String> {
        let mut result = Vec::new();
        for (batch_index, anchor) in anchors.iter().enumerate() {
            for (camera_index, camera) in self.camera_streams.iter().enumerate() {
                let mut matches = Vec::new();
                for segment in self
                    .video_segments
                    .get(&(anchor.episode_id.clone(), camera.clone()))
                    .into_iter()
                    .flatten()
                {
                    if let Some(frame) = segment
                        .timing
                        .frame_for(anchor.sample_index, anchor.sample_hz)?
                    {
                        matches.push((segment, frame));
                    }
                }
                if matches.len() != 1 {
                    return Err(format!(
                        "fixed-FPS contract did not resolve one frame for episode={:?}, sample={}, camera={camera:?}",
                        anchor.episode_id, anchor.sample_index
                    ));
                }
                let (segment, frame_index) = matches[0];
                result.push(ImageCoordinate {
                    batch_index,
                    camera_index,
                    episode_id: anchor.episode_id.clone(),
                    camera_stream_id: camera.clone(),
                    segment_index: segment.segment_index,
                    frame_index,
                    media_relation: segment.media_relation.clone(),
                });
            }
        }
        Ok(result)
    }
}

fn validate_image_batch_size(
    batch_size: usize,
    camera_count: usize,
    image_width: usize,
    image_height: usize,
) -> Result<usize, String> {
    let image_elements = batch_size
        .checked_mul(camera_count)
        .and_then(|values| values.checked_mul(3))
        .and_then(|values| values.checked_mul(image_width))
        .and_then(|values| values.checked_mul(image_height))
        .ok_or_else(|| "ACT image batch element count overflowed".to_owned())?;
    let image_bytes = image_elements
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| "ACT image batch byte size overflowed".to_owned())?;
    if image_bytes > MAX_TRAINING_IMAGE_BATCH_BYTES {
        return Err(format!(
            "ACT image batch requires {image_bytes} bytes, exceeding the {MAX_TRAINING_IMAGE_BATCH_BYTES}-byte safety limit"
        ));
    }
    Ok(image_elements)
}

fn query_batches(
    connection: &mut ManagedConnection,
    sql: &str,
) -> Result<Vec<RecordBatch>, String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral training query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral training query: {error}"))?;
    let reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral training query: {error}"))?;
    reader
        .map(|batch| batch.map_err(|error| format!("reading Spiral training query: {error}")))
        .collect()
}

pub(crate) fn query_image_batches(
    connection: &mut ManagedConnection,
    sql: &str,
    width: usize,
    height: usize,
) -> Result<Vec<RecordBatch>, String> {
    let width = u32::try_from(width)
        .map_err(|_| "training image width is outside Spiral's u32 range".to_owned())?;
    let height = u32::try_from(height)
        .map_err(|_| "training image height is outside Spiral's u32 range".to_owned())?;
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral image query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral image query: {error}"))?;
    statement
        .prepare()
        .map_err(|error| format!("preparing Spiral image query: {error}"))?;
    let parameters = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("width", DataType::UInt32, false),
            Field::new("height", DataType::UInt32, false),
        ])),
        vec![
            Arc::new(UInt32Array::from(vec![width])),
            Arc::new(UInt32Array::from(vec![height])),
        ],
    )
    .map_err(|error| format!("building Spiral image parameters: {error}"))?;
    statement
        .bind(parameters)
        .map_err(|error| format!("binding Spiral image parameters: {error}"))?;
    let reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral image query: {error}"))?;
    reader
        .map(|batch| batch.map_err(|error| format!("reading Spiral image query: {error}")))
        .collect()
}

fn required<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef, String> {
    batch
        .column_by_name(name)
        .ok_or_else(|| format!("Spiral training result is missing column {name:?}"))
}

macro_rules! integer_array {
    ($array:expr, $row:expr, $($ty:ty),+ $(,)?) => {{
        let array = $array;
        let row = $row;
        let mut value = None;
        $(
            if let Some(values) = array.as_any().downcast_ref::<$ty>() {
                value = Some(values.value(row) as i64);
            }
        )+
        value
    }};
}

fn integer_value(array: &ArrayRef, row: usize) -> Result<i64, String> {
    if array.is_null(row) {
        return Err("required integer value is null".to_owned());
    }
    integer_array!(
        array,
        row,
        Int8Array,
        Int16Array,
        Int32Array,
        Int64Array,
        UInt8Array,
        UInt16Array,
        UInt32Array
    )
    .or_else(|| {
        array
            .as_any()
            .downcast_ref::<UInt64Array>()
            .and_then(|values| values.value(row).try_into().ok())
    })
    .ok_or_else(|| format!("expected integer Arrow value, found {}", array.data_type()))
}

fn float_value(array: &ArrayRef, row: usize) -> Result<f64, String> {
    if array.is_null(row) {
        return Err("required floating-point value is null".to_owned());
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(values.value(row));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float32Array>() {
        return Ok(f64::from(values.value(row)));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float16Array>() {
        return Ok(f64::from(f32::from(values.value(row))));
    }
    integer_value(array, row).map(|value| value as f64)
}

fn boolean_value(array: &ArrayRef, row: usize) -> Result<bool, String> {
    if array.is_null(row) {
        return Err("required boolean value is null".to_owned());
    }
    array
        .as_any()
        .downcast_ref::<BooleanArray>()
        .map(|values| values.value(row))
        .ok_or_else(|| format!("expected Boolean Arrow value, found {}", array.data_type()))
}

fn optional_string(array: &ArrayRef, row: usize) -> Result<Option<String>, String> {
    if array.is_null(row) {
        Ok(None)
    } else {
        string_value(array, row).map(Some)
    }
}

fn string_value(array: &ArrayRef, row: usize) -> Result<String, String> {
    if array.is_null(row) {
        return Err("required string value is null".to_owned());
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(values.value(row).to_owned());
    }
    if let Some(values) = array.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(values.value(row).to_owned());
    }
    if let Some(values) = array.as_any().downcast_ref::<StringViewArray>() {
        return Ok(values.value(row).to_owned());
    }
    Err(format!(
        "expected UTF-8 Arrow value, found {}",
        array.data_type()
    ))
}

fn list_value(array: &ArrayRef, row: usize) -> Result<ArrayRef, String> {
    if array.is_null(row) {
        return Err("required list value is null".to_owned());
    }
    if let Some(values) = array.as_any().downcast_ref::<ListArray>() {
        return Ok(values.value(row));
    }
    if let Some(values) = array.as_any().downcast_ref::<LargeListArray>() {
        return Ok(values.value(row));
    }
    if let Some(values) = array.as_any().downcast_ref::<FixedSizeListArray>() {
        return Ok(values.value(row));
    }
    Err(format!(
        "expected list Arrow value, found {}",
        array.data_type()
    ))
}

fn string_list(array: &ArrayRef, row: usize) -> Result<Vec<String>, String> {
    let values = list_value(array, row)?;
    (0..values.len())
        .map(|row| string_value(&values, row))
        .collect::<Result<Vec<_>, _>>()
}

fn numeric_vector(array: &ArrayRef, row: usize) -> Result<Vec<f32>, String> {
    if let Some(values) = array.as_any().downcast_ref::<StructArray>() {
        let data = values
            .column_by_name("data")
            .ok_or_else(|| "Arrow tensor struct has no data field".to_owned())?;
        return numeric_vector(data, row);
    }
    let values = list_value(array, row)?;
    // This is the hot path for Spiral image tensors and numeric vectors. Arrow
    // already guarantees one contiguous child slice, so copy the entire F32
    // buffer once instead of boxing or converting individual scalar values.
    if let Some(values) = values.as_any().downcast_ref::<Float32Array>()
        && values.null_count() == 0
    {
        let result = values.values().to_vec();
        if result.iter().any(|value| !value.is_finite()) {
            return Err("numeric vector contains a non-finite value".to_owned());
        }
        return Ok(result);
    }
    let mut result = Vec::with_capacity(values.len());
    for index in 0..values.len() {
        let value = float_value(&values, index)? as f32;
        if !value.is_finite() {
            return Err("numeric vector contains a non-finite value".to_owned());
        }
        result.push(value);
    }
    Ok(result)
}

pub(crate) fn tensor_value(array: &ArrayRef, row: usize) -> Result<(Vec<f32>, Vec<usize>), String> {
    let values = array
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| format!("expected Arrow tensor struct, found {}", array.data_type()))?;
    let data = values
        .column_by_name("data")
        .ok_or_else(|| "Arrow tensor struct has no data field".to_owned())?;
    let shape = values
        .column_by_name("shape")
        .ok_or_else(|| "Arrow tensor struct has no shape field".to_owned())?;
    let data = numeric_vector(data, row)?;
    let shape = list_value(shape, row)?;
    let shape = (0..shape.len())
        .map(|index| {
            integer_value(&shape, index).and_then(|value| {
                value
                    .try_into()
                    .map_err(|_| "Arrow tensor shape contains a negative dimension".to_owned())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((data, shape))
}

fn clean_query(sql: &str) -> Result<String, String> {
    let query = sql.trim().trim_end_matches(';').trim();
    if query.is_empty() {
        Err("the Spiral training query cannot be empty".to_owned())
    } else {
        Ok(query.to_owned())
    }
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_relation(value: &str) -> Result<(), String> {
    if value.split('.').all(spiral::is_identifier) {
        Ok(())
    } else {
        Err(format!("invalid media relation {value:?}"))
    }
}

fn frame_groups(coordinates: &[ImageCoordinate]) -> Vec<FrameGroup> {
    let mut groups: BTreeMap<(String, String, u32, String), Vec<&ImageCoordinate>> =
        BTreeMap::new();
    for coordinate in coordinates {
        groups
            .entry((
                coordinate.episode_id.clone(),
                coordinate.camera_stream_id.clone(),
                coordinate.segment_index,
                coordinate.media_relation.clone(),
            ))
            .or_default()
            .push(coordinate);
    }
    groups
        .into_iter()
        .map(|(key, values)| FrameGroup {
            episode_id: key.0,
            camera_stream_id: key.1,
            segment_index: key.2,
            media_relation: key.3,
            frame_indices: values.iter().map(|value| value.frame_index).collect(),
            destinations: values
                .iter()
                .map(|value| (value.batch_index, value.camera_index))
                .collect(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    fn range(&mut self, end: u64) -> u64 {
        self.next() % end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_batch_budget_accepts_largest_whole_batch_and_rejects_next() {
        assert_eq!(
            validate_image_batch_size(85, 1, 512, 512),
            Ok(85 * 3 * 512 * 512)
        );
        let error = validate_image_batch_size(86, 1, 512, 512).unwrap_err();
        assert!(error.contains("safety limit"));
        assert!(
            validate_image_batch_size(usize::MAX, 8, 1024, 1024)
                .unwrap_err()
                .contains("overflowed")
        );
    }

    #[test]
    fn relation_names_are_never_interpolated_without_validation() {
        assert!(validate_relation("robotics.video_assets").is_ok());
        assert!(validate_relation("robotics.assets; DELETE").is_err());
    }
}
