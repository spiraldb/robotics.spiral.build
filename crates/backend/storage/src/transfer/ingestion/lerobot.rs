//! A LeRobot v3 repository read as episodes: `meta/info.json` names its
//! features and where its files lie, `meta/tasks.parquet` its tasks,
//! `meta/episodes` where each episode's frames and video are, and the frame
//! tables hold one row per frame. Every feature becomes a stream with one
//! message per frame at the frame's time, and every camera an `h264` stream
//! FFmpeg encodes again: an episode's cameras at the same time, one episode
//! after another.
mod cameras;
mod features;

use super::downloads::HuggingFaceDownloads;
use crate::datasets::{
    DecodedEpisode, EpisodeOrigin, LeRobotEpisode, MessagePositions, RecordedStream, StreamMessages,
};
use arrow_array::cast::AsArray as _;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{
    Array, ArrayRef, BinaryViewArray, PrimitiveArray, RecordBatch, RecordBatchReader as _,
    StringViewArray, StructArray,
};
use bytes::Bytes;
use cameras::{CameraCut, CameraEncoder, FrameSpan};
use features::{FeatureInfo, LeRobotFeature};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;
use spiral_robotics_common::datasets::{H264AccessUnit, HuggingFaceDatasetFile};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What `meta/info.json` says of a repository.
#[derive(Deserialize)]
struct InfoFile {
    codebase_version: String,
    fps: f64,
    data_path: String,
    video_path: Option<String>,
    features: BTreeMap<String, FeatureInfo>,
}

/// A repository's layout and features.
struct LeRobotInfo {
    fps: f64,
    data_path: String,
    video_path: Option<String>,
    features: Vec<LeRobotFeature>,
}

impl LeRobotInfo {
    const CODEBASE_VERSION: &str = "v3.0";

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let info: InfoFile = serde_json::from_slice(bytes)
            .map_err(|error| format!("reading meta/info.json: {error}"))?;
        if info.codebase_version != Self::CODEBASE_VERSION {
            return Err(format!(
                "LeRobot codebase version `{}` is unsupported; Studio imports {}",
                info.codebase_version,
                Self::CODEBASE_VERSION
            ));
        }
        if !(info.fps.is_finite() && info.fps > 0.0) {
            return Err(format!("meta/info.json gives {} frames a second", info.fps));
        }
        let mut features = Vec::new();
        for (name, feature) in &info.features {
            features.extend(LeRobotFeature::of(name, feature)?);
        }
        if features.iter().any(LeRobotFeature::is_camera) && info.video_path.is_none() {
            return Err("meta/info.json names cameras but no video_path".into());
        }
        Ok(Self {
            fps: info.fps,
            data_path: info.data_path,
            video_path: info.video_path,
            features,
        })
    }

    /// The frame table at `place`.
    fn data_file(&self, place: FilePlace) -> Result<String, String> {
        PathTemplate(&self.data_path).fill(&[], place)
    }

    /// `camera`'s video file at `place`.
    fn video_file(&self, camera: &str, place: FilePlace) -> Result<String, String> {
        let template = self.video_path.as_deref().ok_or("No video_path")?;
        PathTemplate(template).fill(&[("video_key", camera)], place)
    }
}

/// A path of `meta/info.json`, its fields in Python's format syntax:
/// `data/chunk-{chunk_index:03d}/file-{file_index:03d}.parquet`.
struct PathTemplate<'a>(&'a str);

impl PathTemplate<'_> {
    /// The path for the file at `place`, `names` giving every other field.
    fn fill(&self, names: &[(&str, &str)], place: FilePlace) -> Result<String, String> {
        let mut path = String::new();
        let mut rest = self.0;
        while let Some(start) = rest.find('{') {
            path.push_str(&rest[..start]);
            let end = rest[start..]
                .find('}')
                .ok_or_else(|| format!("{:?} leaves a field open", self.0))?;
            let (field, spec) = rest[start + 1..start + end]
                .split_once(':')
                .unwrap_or((&rest[start + 1..start + end], ""));
            let value = match field {
                "chunk_index" => place.chunk.to_string(),
                "file_index" => place.file.to_string(),
                _ => names
                    .iter()
                    .find(|(name, _)| *name == field)
                    .map(|(_, value)| (*value).to_owned())
                    .ok_or_else(|| format!("{:?} names an unknown field {field}", self.0))?,
            };
            let width = match spec
                .strip_prefix('0')
                .and_then(|spec| spec.strip_suffix('d'))
            {
                Some(width) => width
                    .parse::<usize>()
                    .map_err(|_| format!("{:?} pads {field} as {spec:?}", self.0))?,
                None if spec.is_empty() => 0,
                None => return Err(format!("{:?} formats {field} as {spec:?}", self.0)),
            };
            path.push_str(&format!("{value:0>width$}"));
            rest = &rest[start + end + 1..];
        }
        path.push_str(rest);
        Ok(path)
    }
}

/// Where a file lies among a repository's chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FilePlace {
    chunk: i64,
    file: i64,
}

/// Where one episode's frames and videos lie.
struct EpisodeEntry {
    index: i64,
    frames: usize,
    data: FilePlace,
    /// Each camera's video file, and the second the episode starts in it.
    videos: BTreeMap<String, (FilePlace, f64)>,
}

/// One parquet table read whole.
struct ParquetTable {
    label: String,
    batch: RecordBatch,
}

impl ParquetTable {
    /// `bytes`, the parquet file `label`, its columns `columns` names or
    /// all of them.
    fn read(label: &str, bytes: Bytes, columns: Option<&[&str]>) -> Result<Self, String> {
        let failed = |error: parquet::errors::ParquetError| format!("reading {label}: {error}");
        let mut builder = ParquetRecordBatchReaderBuilder::try_new(bytes).map_err(failed)?;
        if let Some(columns) = columns {
            let roots = columns
                .iter()
                .map(|column| {
                    builder
                        .schema()
                        .index_of(column)
                        .map_err(|_| format!("{label} has no column {column}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mask = ProjectionMask::roots(builder.parquet_schema(), roots);
            builder = builder.with_projection(mask);
        }
        let reader = builder.build().map_err(failed)?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("reading {label}: {error}"))?;
        let batch = arrow_select::concat::concat_batches(&schema, &batches)
            .map_err(|error| format!("reading {label}: {error}"))?;
        Ok(Self {
            label: label.to_owned(),
            batch,
        })
    }

    fn column(&self, name: &str) -> Result<&ArrayRef, String> {
        self.batch
            .column_by_name(name)
            .ok_or_else(|| format!("{} has no column {name}", self.label))
    }

    /// `name`'s values as `T`, none of them null.
    fn values<T: arrow_array::ArrowPrimitiveType>(
        &self,
        name: &str,
    ) -> Result<PrimitiveArray<T>, String> {
        let values = arrow_cast::cast(self.column(name)?, &T::DATA_TYPE)
            .map_err(|error| format!("{}'s {name}: {error}", self.label))?;
        let values = values.as_primitive::<T>().clone();
        if values.null_count() > 0 {
            return Err(format!("{}'s {name} has empty rows", self.label));
        }
        Ok(values)
    }
}

/// The frame table read last, which the next episodes most likely share.
struct FrameFile {
    place: FilePlace,
    table: ParquetTable,
}

/// Each camera's video file the episodes being read lie in, copied to local
/// disk once: FFmpeg cuts every episode of a file from the copy, and the
/// copy goes once the episodes move on to the camera's next file.
#[derive(Default)]
struct LocalVideos {
    copies: BTreeMap<String, LocalVideo>,
}

struct LocalVideo {
    place: FilePlace,
    file: tempfile::NamedTempFile,
}

impl LocalVideos {
    /// Copy every video file of `entry`'s cameras this does not hold yet,
    /// all at once, dropping the copies they replace.
    async fn hold(
        &mut self,
        downloads: &HuggingFaceDownloads,
        info: &LeRobotInfo,
        entry: &EpisodeEntry,
    ) -> Result<(), String> {
        let missing = entry
            .videos
            .iter()
            .filter(|(camera, (place, _))| {
                self.copies
                    .get(*camera)
                    .is_none_or(|copy| copy.place != *place)
            })
            .map(|(camera, (place, _))| {
                Ok((camera.clone(), *place, info.video_file(camera, *place)?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let copied = futures::future::try_join_all(missing.into_iter().map(
            |(camera, place, path)| async move {
                let file = tempfile::NamedTempFile::new()
                    .map_err(|error| format!("copying {path}: {error}"))?;
                let mut writer = tokio::fs::File::from_std(
                    file.reopen()
                        .map_err(|error| format!("copying {path}: {error}"))?,
                );
                downloads.save(&path, &mut writer).await?;
                Ok::<_, String>((camera, LocalVideo { place, file }))
            },
        ))
        .await?;
        self.copies.extend(copied);
        Ok(())
    }

    /// The copy of `camera`'s video file at `place`.
    fn path(&self, camera: &str, place: FilePlace) -> Result<&std::path::Path, String> {
        self.copies
            .get(camera)
            .filter(|copy| copy.place == place)
            .map(|copy| copy.file.path())
            .ok_or_else(|| format!("{camera}'s video is not copied yet"))
    }
}

pub(super) struct LeRobotRepository<'a> {
    downloads: &'a HuggingFaceDownloads,
    info: LeRobotInfo,
    tasks: BTreeMap<i64, String>,
    episodes: Vec<EpisodeEntry>,
    frames: Option<FrameFile>,
    cameras: CameraEncoder,
    videos: LocalVideos,
}

impl<'a> LeRobotRepository<'a> {
    /// The frame columns LeRobot positions every frame by.
    const TIMESTAMP: &str = "timestamp";
    const FRAME_INDEX: &str = "frame_index";
    const EPISODE_INDEX: &str = "episode_index";
    const TASK_INDEX: &str = "task_index";

    /// The repository `downloads` reads, whose files `files` lists.
    pub(super) async fn open(
        downloads: &'a HuggingFaceDownloads,
        files: &[HuggingFaceDatasetFile],
    ) -> Result<Self, String> {
        let info = LeRobotInfo::parse(&downloads.bytes("meta/info.json").await?)?;
        let tasks = Self::tasks(downloads.bytes("meta/tasks.parquet").await?)?;
        let mut episodes = Vec::new();
        for file in files.iter().filter(|file| {
            file.path().starts_with("meta/episodes/") && file.path().ends_with(".parquet")
        }) {
            let table = ParquetTable::read(file.path(), downloads.bytes(file.path()).await?, None)?;
            episodes.extend(Self::entries(&table, &info)?);
        }
        episodes.sort_by_key(|entry| entry.index);
        if episodes
            .windows(2)
            .any(|pair| pair[0].index == pair[1].index)
        {
            return Err("meta/episodes lists an episode twice".into());
        }
        Ok(Self {
            downloads,
            info,
            tasks,
            episodes,
            frames: None,
            cameras: CameraEncoder::from_environment(),
            videos: LocalVideos::default(),
        })
    }

    /// Each task's text by its index. Its text is the `task` column, or the
    /// table's pandas index where LeRobot wrote it as one.
    fn tasks(bytes: Bytes) -> Result<BTreeMap<i64, String>, String> {
        let table = ParquetTable::read("meta/tasks.parquet", bytes, None)?;
        let text = ["task", "__index_level_0__"]
            .into_iter()
            .find(|name| table.batch.column_by_name(name).is_some())
            .ok_or("meta/tasks.parquet has no task column")?;
        let texts = arrow_cast::cast(table.column(text)?, &arrow_schema::DataType::Utf8)
            .map_err(|error| format!("meta/tasks.parquet's {text}: {error}"))?;
        let texts = texts.as_string::<i32>();
        let indices = table.values::<Int64Type>(Self::TASK_INDEX)?;
        Ok(indices
            .values()
            .iter()
            .zip(texts.iter())
            .map(|(index, text)| (*index, text.unwrap_or_default().to_owned()))
            .collect())
    }

    /// The episodes one table of `meta/episodes` lists.
    fn entries(table: &ParquetTable, info: &LeRobotInfo) -> Result<Vec<EpisodeEntry>, String> {
        let integers = |name: &str| table.values::<Int64Type>(name);
        let places = |prefix: &str| -> Result<Vec<FilePlace>, String> {
            let chunks = integers(&format!("{prefix}/chunk_index"))?;
            let files = integers(&format!("{prefix}/file_index"))?;
            Ok(chunks
                .values()
                .iter()
                .zip(files.values())
                .map(|(&chunk, &file)| FilePlace { chunk, file })
                .collect())
        };
        let indices = integers(Self::EPISODE_INDEX)?;
        let lengths = integers("length")?;
        let data = places("data")?;
        let mut videos = BTreeMap::new();
        for camera in info.features.iter().filter(|feature| feature.is_camera()) {
            let prefix = format!("videos/{}", camera.name());
            let starts = table.values::<Float64Type>(&format!("{prefix}/from_timestamp"))?;
            videos.insert(camera.name(), (places(&prefix)?, starts));
        }
        (0..table.batch.num_rows())
            .map(|row| {
                Ok(EpisodeEntry {
                    index: indices.value(row),
                    frames: usize::try_from(lengths.value(row)).map_err(|_| {
                        format!("episode {} has a negative length", indices.value(row))
                    })?,
                    data: data[row],
                    videos: videos
                        .iter()
                        .map(|(camera, (places, starts))| {
                            ((*camera).to_owned(), (places[row], starts.value(row)))
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// How many episodes the repository holds.
    pub(super) fn len(&self) -> usize {
        self.episodes.len()
    }

    /// The episode at `position` in index order: every feature's values of
    /// each frame, and every camera's frames encoded again.
    pub(super) async fn episode(&mut self, position: usize) -> Result<DecodedEpisode, String> {
        let entry = &self.episodes[position];
        let (index, frames) = (entry.index, entry.frames);
        let rows = self.frames_of(position).await?;
        let frame_indices = rows.values::<Int64Type>(Self::FRAME_INDEX)?;
        if frame_indices.len() != frames
            || frame_indices
                .values()
                .iter()
                .zip(0..)
                .any(|(&frame, place)| frame != place)
        {
            return Err(format!(
                "episode {index}'s frames are not its {frames} frames in order"
            ));
        }
        let mut positions = MessagePositions::default();
        for (frame, timestamp) in rows
            .values::<Float64Type>(Self::TIMESTAMP)?
            .values()
            .iter()
            .enumerate()
        {
            let log_time = (timestamp * 1e9).round();
            if !(log_time >= 0.0 && log_time < u64::MAX as f64) {
                return Err(format!(
                    "episode {index}'s frame {frame} is at {timestamp} s"
                ));
            }
            positions.push(log_time as u64, log_time as u64, frame as u32);
        }
        let mut tasks = Vec::<String>::new();
        for task in rows.values::<Int64Type>(Self::TASK_INDEX)?.values() {
            let text = self.tasks.get(task).ok_or_else(|| {
                format!("episode {index} names task {task}, which meta/tasks.parquet does not")
            })?;
            if !tasks.contains(text) {
                tasks.push(text.clone());
            }
        }
        let mut streams = Vec::new();
        for feature in self
            .info
            .features
            .iter()
            .filter(|feature| !feature.is_camera())
        {
            let values = feature.values(rows.column(feature.name())?)?;
            let stream =
                RecordedStream::new(feature.name(), None, "parquet", values.data_type().clone())?;
            streams.push(StreamMessages::new(
                stream,
                positions.clone(),
                None,
                values,
            )?);
        }
        // Every camera of the episode is encoded at the same time, from its
        // video copied to disk.
        self.hold_videos(position).await?;
        let cameras = self
            .info
            .features
            .iter()
            .filter(|feature| feature.is_camera())
            .collect::<Vec<_>>();
        let encoded = futures::future::try_join_all(
            cameras
                .iter()
                .map(|camera| self.camera(position, camera.name())),
        )
        .await?;
        for (camera, units) in cameras.into_iter().zip(encoded) {
            let keyframes = units
                .iter()
                .map(|unit| H264AccessUnit::holds_idr(unit))
                .collect();
            let values = Self::camera_values(&units)?;
            let stream = RecordedStream::new(
                camera.name(),
                None,
                RecordedStream::H264,
                values.data_type().clone(),
            )?;
            streams.push(StreamMessages::new(
                stream,
                positions.clone(),
                Some(keyframes),
                values,
            )?);
        }
        Ok(DecodedEpisode::new(
            format!("episode_{index:06}"),
            EpisodeOrigin::LeRobot(LeRobotEpisode::new(
                self.downloads.root_uri()?,
                index,
                tasks,
            )),
            streams,
        ))
    }

    /// The frame rows of the episode at `position`, reading its frame table
    /// unless it was read last.
    async fn frames_of(&mut self, position: usize) -> Result<ParquetTable, String> {
        let entry = &self.episodes[position];
        if self
            .frames
            .as_ref()
            .is_none_or(|frames| frames.place != entry.data)
        {
            let path = self.info.data_file(entry.data)?;
            let bytes = self.downloads.bytes(&path).await?;
            let mut columns = vec![
                Self::TIMESTAMP,
                Self::FRAME_INDEX,
                Self::EPISODE_INDEX,
                Self::TASK_INDEX,
            ];
            columns.extend(
                self.info
                    .features
                    .iter()
                    .filter(|feature| !feature.is_camera())
                    .map(LeRobotFeature::name),
            );
            let columns = columns.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let table = tokio::task::spawn_blocking(move || {
                let columns = columns.iter().map(String::as_str).collect::<Vec<_>>();
                ParquetTable::read(&path, bytes, Some(&columns))
            })
            .await
            .map_err(|error| format!("reading a frame table failed: {error}"))??;
            self.frames = Some(FrameFile {
                place: entry.data,
                table,
            });
        }
        let table = &self.frames.as_ref().expect("read above").table;
        let index = entry.index;
        let episodes = table.values::<Int64Type>(Self::EPISODE_INDEX)?;
        let start = episodes
            .values()
            .iter()
            .position(|&episode| episode == index)
            .ok_or_else(|| format!("{} holds no frames of episode {index}", table.label))?;
        let length = episodes.values()[start..]
            .iter()
            .take_while(|&&episode| episode == index)
            .count();
        if episodes.values()[start + length..].contains(&index) {
            return Err(format!("{} splits episode {index}", table.label));
        }
        Ok(ParquetTable {
            label: format!("{}'s episode {index}", table.label),
            batch: table.batch.slice(start, length),
        })
    }

    /// Copy the video files the episode at `position` lies in to disk,
    /// unless they are already.
    async fn hold_videos(&mut self, position: usize) -> Result<(), String> {
        self.videos
            .hold(self.downloads, &self.info, &self.episodes[position])
            .await
    }

    /// `camera`'s frames of the episode at `position`, one access unit each,
    /// cut from the copy [`Self::hold_videos`] made.
    async fn camera(&self, position: usize, camera: &str) -> Result<Vec<Vec<u8>>, String> {
        let entry = &self.episodes[position];
        let (place, from_seconds) = entry
            .videos
            .get(camera)
            .ok_or_else(|| format!("meta/episodes places no {camera} video"))?;
        let span = FrameSpan::starting(*from_seconds, self.info.fps, entry.frames);
        self.cameras
            .encode(&CameraCut::new(self.videos.path(camera, *place)?, span))
            .await
            .map_err(|error| format!("episode {}'s {camera}: {error}", entry.index))
    }

    /// Access units as a camera stream's values, each `{data, format}`.
    fn camera_values(units: &[Vec<u8>]) -> Result<ArrayRef, String> {
        let arrow_schema::DataType::Struct(fields) = LeRobotFeature::camera_type() else {
            unreachable!("a camera is a struct");
        };
        StructArray::try_new(
            fields,
            vec![
                Arc::new(BinaryViewArray::from_iter_values(units)),
                Arc::new(StringViewArray::from_iter_values(
                    units.iter().map(|_| RecordedStream::H264),
                )),
            ],
            None,
        )
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests;
