//! LeRobot dataset interchange.
//!
//! The studio keeps its live recording format deliberately simple and
//! self-describing. Interchange is a boundary concern: completed episodes are
//! projected into the current LeRobot v3 tabular layout and packaged as one
//! ZIP, while imported v2/v3 Parquet archives are converted back into local
//! episodes before replay. Camera video is not invented here—the recorder does
//! not capture it yet—so exports contain low-dimensional data only: joint
//! state and action always, and the motors' own velocity and effort when every
//! exported take carries them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
#[cfg(test)]
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, Int32Array, Int64Array,
    LargeListArray, ListArray, RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use bytes::Bytes;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde_json::{Value, json};
use spiral_robotics_contracts::api::EpisodeArm;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::backend::recording::episodes::{
    EpisodeError, EpisodeStore, MAX_EPISODE_FRAMES, Sample, frame_capacity,
};

const TASK: &str = "Recorded with Spiral Robotics";
const MAX_ARCHIVE_ENTRIES: usize = 4_096;
pub(crate) const MAX_ARCHIVE_COMPRESSED_BYTES: usize = 256 * 1024 * 1024;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_COMPRESSION_RATIO: u64 = 200;
const COMPRESSION_RATIO_CHECK_BYTES: u64 = 1024 * 1024;
const MAX_DATASET_EPISODES: usize = 10_000;
const SO101_JOINTS: [&str; 6] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
    "gripper",
];

/// A complete downloadable dataset package.
#[cfg(test)]
pub(crate) struct DatasetArchive {
    pub bytes: Vec<u8>,
    pub summary: DatasetArchiveSummary,
}

#[cfg(test)]
impl std::ops::Deref for DatasetArchive {
    type Target = DatasetArchiveSummary;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DatasetArchiveSummary {
    pub episodes: u64,
    pub frames: u64,
    pub skipped: u64,
    /// Exported takes that carried no velocity or effort.
    ///
    /// LeRobot wants one feature set for a whole dataset, so a single take
    /// recorded on a bus that does not publish these costs every take the
    /// columns. Counted rather than papered over: an export that quietly filled
    /// the gap with zeroes would put fabricated effort into a training set.
    pub without_proprioception: u64,
}

#[derive(Debug)]
struct ExportEpisode {
    id: String,
    frames: u64,
    joints: Vec<String>,
    positions: Vec<Vec<f32>>,
    actions: Vec<Vec<f32>>,
    /// Per accepted frame, in `joints` order. `None` when any frame of this
    /// take lacked it, because a take is exported whole or not at all.
    velocity: Option<Vec<Vec<f32>>>,
    effort: Option<Vec<Vec<f32>>>,
    sample_hz: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DatasetExportInspection {
    pub id: String,
    pub frames: u64,
    pub joints: Vec<String>,
    pub sample_hz: f64,
    pub complete_observations: bool,
    pub complete_actions: bool,
    pub complete_velocity: bool,
    pub complete_effort: bool,
}

#[derive(Debug)]
pub(crate) struct PreparedDatasetExport {
    episodes: Vec<ExportEpisode>,
    inspections: Vec<DatasetExportInspection>,
}

impl PreparedDatasetExport {
    pub(crate) fn inspections(&self) -> &[DatasetExportInspection] {
        &self.inspections
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DatasetImportSummary {
    pub episodes: u64,
    pub frames: u64,
    pub first_episode: Option<String>,
}

#[derive(Debug)]
pub(crate) struct StagedDatasetImport {
    staging: tempfile::TempDir,
    staged_files: Vec<String>,
    summary: DatasetImportSummary,
}

impl StagedDatasetImport {
    pub(crate) fn summary(&self) -> &DatasetImportSummary {
        &self.summary
    }
}

#[derive(Debug)]
struct ImportedFrame {
    frame: i64,
    timestamp: f64,
    observation: Vec<f64>,
    action: Vec<f64>,
}

/// Export every complete, non-cancelled episode as a LeRobot v3 ZIP.
#[cfg(test)]
pub(crate) fn export_lerobot_archive(store: &EpisodeStore) -> Result<DatasetArchive, EpisodeError> {
    export_lerobot_archive_for_dataset(store, None)
}

/// Export complete episodes assigned to one dataset. `None` preserves the
/// legacy all-episodes boundary for CLI/tests that have not selected a set.
#[cfg(test)]
pub(crate) fn export_lerobot_archive_for_dataset(
    store: &EpisodeStore,
    dataset_id: Option<&str>,
) -> Result<DatasetArchive, EpisodeError> {
    let (cursor, summary) = export_lerobot_archive_to(store, dataset_id, Cursor::new(Vec::new()))?;
    Ok(DatasetArchive {
        bytes: cursor.into_inner(),
        summary,
    })
}

/// Write a LeRobot ZIP directly to a file so the HTTP layer can stream it
/// without retaining a second archive-sized allocation in server memory.
#[cfg(test)]
pub(crate) fn export_lerobot_archive_to_file_for_dataset(
    store: &EpisodeStore,
    dataset_id: Option<&str>,
    file: File,
) -> Result<DatasetArchiveSummary, EpisodeError> {
    let (file, summary) = export_lerobot_archive_to(store, dataset_id, file)?;
    file.sync_all().map_err(|source| EpisodeError::Io {
        path: Path::new("temporary LeRobot export").to_owned(),
        source,
    })?;
    Ok(summary)
}

/// Read exactly the application-selected takes into an opaque export staging
/// value and report the facts needed for compatibility admission.
pub(crate) fn prepare_lerobot_export(
    store: &EpisodeStore,
    episode_ids: &[String],
) -> Result<PreparedDatasetExport, EpisodeError> {
    let mut episodes = Vec::with_capacity(episode_ids.len());
    let mut inspections = Vec::with_capacity(episode_ids.len());
    for id in episode_ids {
        let summary = store.get(id)?;
        let episode = read_export_episode(store, &summary)?;
        inspections.push(DatasetExportInspection {
            id: episode.id.clone(),
            frames: episode.frames,
            joints: episode.joints.clone(),
            sample_hz: episode.sample_hz,
            complete_observations: episode.positions.len() as u64 == episode.frames
                && episode.positions.iter().all(|frame| !frame.is_empty()),
            complete_actions: episode.actions.len() as u64 == episode.frames
                && episode.actions.iter().all(|frame| !frame.is_empty()),
            complete_velocity: episode
                .velocity
                .as_ref()
                .is_some_and(|frames| frames.len() as u64 == episode.frames),
            complete_effort: episode
                .effort
                .as_ref()
                .is_some_and(|frames| frames.len() as u64 == episode.frames),
        });
        episodes.push(episode);
    }
    Ok(PreparedDatasetExport {
        episodes,
        inspections,
    })
}

#[cfg(test)]
fn export_lerobot_archive_to<W: Write + std::io::Seek>(
    store: &EpisodeStore,
    dataset_id: Option<&str>,
    destination: W,
) -> Result<(W, DatasetArchiveSummary), EpisodeError> {
    let listed = store.list();
    let candidates = listed
        .iter()
        .filter(|episode| {
            episode.usable()
                && !episode.discarded
                && dataset_id.is_none_or(|dataset_id| episode.dataset_id == dataset_id)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(invalid("there are no recorded episodes to download"));
    }

    let mut skipped = listed.len().saturating_sub(candidates.len()) as u64;
    let mut episodes = Vec::new();
    for summary in candidates {
        let episode = read_export_episode(store, summary)?;
        let inspection = DatasetExportInspection {
            id: episode.id.clone(),
            frames: episode.frames,
            joints: episode.joints.clone(),
            sample_hz: episode.sample_hz,
            complete_observations: episode.positions.len() as u64 == episode.frames
                && episode.positions.iter().all(|frame| !frame.is_empty()),
            complete_actions: episode.actions.len() as u64 == episode.frames
                && episode.actions.iter().all(|frame| !frame.is_empty()),
            complete_velocity: episode.velocity.is_some(),
            complete_effort: episode.effort.is_some(),
        };
        if !inspection.complete_observations {
            return Err(invalid(format!(
                "{} has no complete follower observation; refusing to shorten the exported take",
                inspection.id
            )));
        }
        if !inspection.complete_actions {
            return Err(invalid(format!(
                "{} has no recorded follower action; refusing to fabricate a training label",
                inspection.id
            )));
        }
        if episode.positions.is_empty() {
            skipped += 1;
        } else {
            episodes.push(episode);
        }
    }
    if episodes.is_empty() {
        return Err(invalid(
            "the recorded episodes contain no calibrated follower positions to export",
        ));
    }

    let joints = episodes[0].joints.clone();
    if episodes.iter().any(|episode| episode.joints != joints) {
        return Err(invalid(
            "the session contains different follower joint layouts; export matching episodes together",
        ));
    }
    let fps = episodes[0].sample_hz.round().max(1.0) as i64;
    if episodes
        .iter()
        .any(|episode| (episode.sample_hz - fps as f64).abs() > 0.5)
    {
        return Err(invalid(
            "the session contains different sample rates; LeRobot requires one dataset FPS",
        ));
    }

    let without_proprioception = episodes
        .iter()
        .filter(|episode| episode.velocity.is_none() || episode.effort.is_none())
        .count() as u64;
    write_lerobot_archive_to(
        episodes,
        destination,
        fps,
        skipped,
        without_proprioception == 0,
        without_proprioception,
    )
}

/// Encode a previously inspected selection after application policy has
/// authorized its exact feature plan.
pub(crate) fn export_prepared_lerobot_to_file(
    prepared: PreparedDatasetExport,
    file: File,
    fps: u32,
    include_proprioception: bool,
) -> Result<(), EpisodeError> {
    if prepared.episodes.is_empty()
        || prepared
            .inspections
            .iter()
            .any(|episode| !episode.complete_observations || !episode.complete_actions)
    {
        return Err(invalid(
            "the prepared export is empty or requires fabricated training values",
        ));
    }
    let without_proprioception = prepared
        .episodes
        .iter()
        .filter(|episode| episode.velocity.is_none() || episode.effort.is_none())
        .count() as u64;
    if include_proprioception != (without_proprioception == 0) {
        return Err(invalid(
            "the authorized feature plan does not match the prepared episodes",
        ));
    }
    let (file, _) = write_lerobot_archive_to(
        prepared.episodes,
        file,
        i64::from(fps),
        0,
        include_proprioception,
        without_proprioception,
    )?;
    file.sync_all().map_err(|source| EpisodeError::Io {
        path: Path::new("temporary LeRobot export").to_owned(),
        source,
    })
}

fn write_lerobot_archive_to<W: Write + std::io::Seek>(
    episodes: Vec<ExportEpisode>,
    destination: W,
    fps: i64,
    skipped: u64,
    proprioception: bool,
    without_proprioception: u64,
) -> Result<(W, DatasetArchiveSummary), EpisodeError> {
    let episode_count = episodes.len();
    if episode_count > MAX_DATASET_EPISODES {
        return Err(invalid(format!(
            "the export contains more than the supported {MAX_DATASET_EPISODES} episodes"
        )));
    }
    let total_frames = episodes.iter().try_fold(0_usize, |total, episode| {
        total
            .checked_add(episode.positions.len())
            .filter(|frames| *frames as u64 <= MAX_EPISODE_FRAMES)
            .ok_or_else(|| {
                invalid(format!(
                    "the export contains more than the supported {MAX_EPISODE_FRAMES} frames"
                ))
            })
    })?;
    let joints = episodes[0].joints.clone();
    let dimension = joints.len();
    // Every take or none: LeRobot declares one feature set for the whole
    // dataset, so one take recorded on a bus that publishes neither velocity
    // nor effort costs every take the columns. The alternative — filling the
    // gap with zeroes — would put fabricated effort into a training set, which
    // is the one outcome here worth refusing outright.
    let mut velocities = Vec::with_capacity(total_frames * dimension);
    let mut efforts = Vec::with_capacity(total_frames * dimension);
    let mut states = Vec::with_capacity(total_frames * dimension);
    let mut actions = Vec::with_capacity(total_frames * dimension);
    let mut timestamps = Vec::with_capacity(total_frames);
    let mut frame_indices = Vec::with_capacity(total_frames);
    let mut episode_indices = Vec::with_capacity(total_frames);
    let mut indices = Vec::with_capacity(total_frames);
    let mut episode_offsets = Vec::with_capacity(episode_count);
    let mut cursor = 0_i64;

    for (episode_index, episode) in episodes.iter().enumerate() {
        let from = cursor;
        for (frame_index, pose) in episode.positions.iter().enumerate() {
            states.extend(
                pose.iter()
                    .zip(&joints)
                    .map(|(value, joint)| to_lerobot_position(*value, joint)),
            );
            actions.extend(
                episode.actions[frame_index]
                    .iter()
                    .zip(&joints)
                    .map(|(value, joint)| to_lerobot_position(*value, joint)),
            );
            if proprioception {
                // Physical units, unlike the position columns: degrees per
                // second and a signed share of rated torque are what the motor
                // measured, and nothing about LeRobot asks them to be rescaled.
                velocities.extend(
                    episode.velocity.as_ref().expect("checked above")[frame_index]
                        .iter()
                        .copied(),
                );
                efforts.extend(
                    episode.effort.as_ref().expect("checked above")[frame_index]
                        .iter()
                        .copied(),
                );
            }
            timestamps.push(frame_index as f32 / fps as f32);
            frame_indices.push(frame_index as i64);
            episode_indices.push(episode_index as i64);
            indices.push(cursor);
            cursor += 1;
        }
        episode_offsets.push((from, cursor, episode.positions.len() as i64));
    }

    let stats = stats_document(
        dimension,
        &states,
        &actions,
        proprioception.then_some(velocities.as_slice()),
        proprioception.then_some(efforts.as_slice()),
        &timestamps,
        &frame_indices,
        &episode_indices,
        &indices,
    );
    let data = data_parquet(
        dimension,
        states,
        actions.clone(),
        proprioception.then(|| velocities.clone()),
        proprioception.then(|| efforts.clone()),
        timestamps.clone(),
        frame_indices.clone(),
        episode_indices.clone(),
        indices.clone(),
    )?;
    let episode_meta = episodes_parquet(&episode_offsets)?;
    let tasks = tasks_parquet()?;

    let mut features = json!({
        "observation.state": vector_feature(&joints),
        "action": vector_feature(&joints),
        "timestamp": scalar_feature("float32"),
        "frame_index": scalar_feature("int64"),
        "episode_index": scalar_feature("int64"),
        "index": scalar_feature("int64"),
        "task_index": scalar_feature("int64"),
    });
    if proprioception {
        let declared = features
            .as_object_mut()
            .expect("the feature map is a JSON object");
        declared.insert(
            "observation.velocity".to_owned(),
            vector_feature_named(&joints, "vel"),
        );
        declared.insert(
            "observation.effort".to_owned(),
            vector_feature_named(&joints, "effort"),
        );
    }
    let info = json!({
        "codebase_version": "v3.0",
        "robot_type": "so101_follower",
        "total_episodes": episode_count,
        "total_frames": total_frames,
        "total_tasks": 1,
        "chunks_size": 1000,
        "fps": fps,
        "splits": { "train": format!("0:{episode_count}") },
        "data_path": "data/chunk-{chunk_index:03d}/file-{file_index:03d}.parquet",
        "features": features,
        "data_files_size_in_mb": 100,
    });
    let mut archive = ZipWriter::new(destination);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    write_zip_entry(
        &mut archive,
        options,
        "meta/info.json",
        pretty_json(&info)?.as_bytes(),
    )?;
    write_zip_entry(
        &mut archive,
        options,
        "meta/stats.json",
        pretty_json(&stats)?.as_bytes(),
    )?;
    write_zip_entry(&mut archive, options, "meta/tasks.parquet", &tasks)?;
    write_zip_entry(
        &mut archive,
        options,
        "meta/episodes/chunk-000/file-000.parquet",
        &episode_meta,
    )?;
    write_zip_entry(
        &mut archive,
        options,
        "data/chunk-000/file-000.parquet",
        &data,
    )?;
    write_zip_entry(
        &mut archive,
        options,
        "README.md",
        dataset_card(proprioception, without_proprioception).as_bytes(),
    )?;
    let destination = archive
        .finish()
        .map_err(|error| invalid(error.to_string()))?;

    Ok((
        destination,
        DatasetArchiveSummary {
            episodes: episode_count as u64,
            frames: total_frames as u64,
            skipped,
            without_proprioception,
        },
    ))
}

/// Import a zipped LeRobot v2/v3 dataset into the local episode workspace.
#[cfg(test)]
pub(crate) fn import_lerobot_archive(
    store: &EpisodeStore,
    bytes: &[u8],
) -> Result<DatasetImportSummary, EpisodeError> {
    import_lerobot_archive_into_dataset(store, bytes, "default")
}

#[cfg(test)]
pub(crate) fn import_lerobot_archive_into_dataset(
    store: &EpisodeStore,
    bytes: &[u8],
    dataset_id: &str,
) -> Result<DatasetImportSummary, EpisodeError> {
    let staged = stage_lerobot_archive_reader_into_dataset(
        store,
        Cursor::new(bytes),
        bytes.len() as u64,
        dataset_id,
    )?;
    commit_staged_lerobot_import(store, staged)
}

/// Import from a spool file so request bytes never need to coexist in the
/// Axum body buffer, a ZIP allocation, and browser WASM memory.
#[cfg(test)]
pub(crate) fn import_lerobot_archive_file_into_dataset(
    store: &EpisodeStore,
    file: File,
    dataset_id: &str,
) -> Result<DatasetImportSummary, EpisodeError> {
    let staged = stage_lerobot_archive_file_into_dataset(store, file, dataset_id)?;
    commit_staged_lerobot_import(store, staged)
}

/// Decode and validate an uploaded archive without making any imported take
/// visible in the destination catalog.
pub(crate) fn stage_lerobot_archive_file_into_dataset(
    store: &EpisodeStore,
    file: File,
    dataset_id: &str,
) -> Result<StagedDatasetImport, EpisodeError> {
    let length = file
        .metadata()
        .map_err(|source| EpisodeError::Io {
            path: Path::new("temporary LeRobot import").to_owned(),
            source,
        })?
        .len();
    stage_lerobot_archive_reader_into_dataset(store, file, length, dataset_id)
}

fn stage_lerobot_archive_reader_into_dataset<R: Read + std::io::Seek>(
    store: &EpisodeStore,
    source: R,
    compressed_bytes: u64,
    dataset_id: &str,
) -> Result<StagedDatasetImport, EpisodeError> {
    if !spiral_robotics_contracts::spiral::is_dataset_id(dataset_id) {
        return Err(invalid(format!(
            "{dataset_id:?} is not a valid destination dataset id"
        )));
    }
    if compressed_bytes > MAX_ARCHIVE_COMPRESSED_BYTES as u64 {
        return Err(invalid(format!(
            "the selected ZIP is larger than the supported {} MiB compressed limit",
            MAX_ARCHIVE_COMPRESSED_BYTES / (1024 * 1024)
        )));
    }
    let mut archive = ZipArchive::new(source)
        .map_err(|error| invalid(format!("the selected file is not a readable ZIP: {error}")))?;
    let names = validated_archive_names(&mut archive)?;
    let info_name = names
        .iter()
        .find(|name| name.ends_with("meta/info.json"))
        .cloned()
        .ok_or_else(|| invalid("the ZIP does not contain a LeRobot meta/info.json"))?;
    let info: Value = serde_json::from_slice(&read_zip_entry(&mut archive, &info_name)?)
        .map_err(|error| invalid(format!("could not read LeRobot metadata: {error}")))?;
    let fps = info
        .get("fps")
        .and_then(Value::as_f64)
        .filter(|fps| fps.is_finite() && *fps > 0.0 && *fps <= 1_000.0)
        .ok_or_else(|| invalid("LeRobot metadata does not declare a usable fps"))?;
    let video_features = declared_video_features(&info);
    if !video_features.is_empty() {
        return Err(invalid(format!(
            "this archive contains camera features ({}) that this low-dimensional importer cannot preserve; import was stopped instead of silently dropping video",
            video_features.join(", ")
        )));
    }
    let declared_names = action_names(&info);
    let robot_type = info
        .get("robot_type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let data_names = names
        .into_iter()
        .filter(|name| {
            name.ends_with(".parquet")
                && (name.starts_with("data/") || name.contains("/data/"))
                && !name.contains("/meta/")
        })
        .collect::<Vec<_>>();
    if data_names.is_empty() {
        return Err(invalid("the ZIP contains no LeRobot data Parquet files"));
    }

    let mut grouped: BTreeMap<i64, Vec<ImportedFrame>> = BTreeMap::new();
    let mut dimension = None;
    let mut total_frames = 0_u64;
    for (file_number, name) in data_names.iter().enumerate() {
        let parquet = read_zip_entry(&mut archive, name)?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(parquet))
            .map_err(|error| invalid(format!("could not open {name}: {error}")))?
            .build()
            .map_err(|error| invalid(format!("could not read {name}: {error}")))?;
        for batch in reader {
            let batch =
                batch.map_err(|error| invalid(format!("could not read {name}: {error}")))?;
            let actions = batch
                .column_by_name("action")
                .ok_or_else(|| invalid(format!("{name} has no action column")))?;
            let observations = batch
                .column_by_name("observation.state")
                .ok_or_else(|| {
                    invalid(format!(
                        "{name} has no observation.state column; importing it would require fabricating robot state from actions"
                    ))
                })?;
            let episode_index = batch.column_by_name("episode_index");
            let frame_index = batch.column_by_name("frame_index");
            let timestamp = batch.column_by_name("timestamp");
            for row in 0..batch.num_rows() {
                let action = vector_at(actions.as_ref(), row)?;
                let observation = vector_at(observations.as_ref(), row)?;
                if observation.len() != action.len() {
                    return Err(invalid(format!(
                        "{name} row {row} has {} observation values but {} action values",
                        observation.len(),
                        action.len()
                    )));
                }
                if let Some(expected) = dimension {
                    if action.len() != expected {
                        return Err(invalid(
                            "LeRobot action dimensions change inside the dataset",
                        ));
                    }
                } else {
                    dimension = Some(action.len());
                }
                let episode = episode_index
                    .map(|array| integer_at(array.as_ref(), row))
                    .transpose()?
                    .unwrap_or(file_number as i64);
                if episode < 0 {
                    return Err(invalid("LeRobot episode indexes must be non-negative"));
                }
                let frame = frame_index
                    .map(|array| integer_at(array.as_ref(), row))
                    .transpose()?
                    .unwrap_or_else(|| {
                        grouped
                            .get(&episode)
                            .map_or(0, |frames| frames.len() as i64)
                    });
                if frame < 0 {
                    return Err(invalid("LeRobot frame indexes must be non-negative"));
                }
                let time = timestamp
                    .map(|array| number_at(array.as_ref(), row))
                    .transpose()?
                    .unwrap_or(frame as f64 / fps);
                if !time.is_finite() || time < 0.0 {
                    return Err(invalid(
                        "LeRobot timestamps must be finite, non-negative numbers",
                    ));
                }
                total_frames += 1;
                if total_frames > MAX_EPISODE_FRAMES {
                    return Err(invalid(format!(
                        "the archive contains more than the supported {MAX_EPISODE_FRAMES} frames"
                    )));
                }
                grouped.entry(episode).or_default().push(ImportedFrame {
                    frame,
                    timestamp: time,
                    observation,
                    action,
                });
                if grouped.len() > MAX_DATASET_EPISODES {
                    return Err(invalid(format!(
                        "the archive contains more than the supported {MAX_DATASET_EPISODES} episodes"
                    )));
                }
            }
        }
    }

    let dimension = dimension
        .filter(|dimension| *dimension > 0)
        .ok_or_else(|| invalid("the LeRobot dataset has no action samples to import"))?;
    let joints = normalized_joint_names(declared_names, dimension, robot_type)?;

    // Decode and write every take in an isolated same-filesystem store first.
    // Invalid values, writer failures, or a later bad episode therefore cannot
    // leave earlier episodes visible in the destination after an error.
    let staging = tempfile::Builder::new()
        .prefix(".lerobot-import-")
        .tempdir_in(store.directory())
        .map_err(|source| EpisodeError::Io {
            path: store.directory().to_owned(),
            source,
        })?;
    for entry in fs::read_dir(store.directory()).map_err(|source| EpisodeError::Io {
        path: store.directory().to_owned(),
        source,
    })? {
        let entry = entry.map_err(|source| EpisodeError::Io {
            path: store.directory().to_owned(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file()
            || !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("jsonl" | "part")
            )
        {
            continue;
        }
        fs::hard_link(&path, staging.path().join(entry.file_name())).map_err(|source| {
            EpisodeError::Io {
                path: path.clone(),
                source,
            }
        })?;
    }
    let staging_store = EpisodeStore::open(staging.path().to_path_buf())?;

    let mut first_episode = None;
    let mut imported_episodes = 0_u64;
    let mut imported_frames = 0_u64;
    let mut staged_files = Vec::new();
    for frames in grouped.values_mut() {
        frames.sort_by_key(|frame| frame.frame);
        if frames.is_empty() {
            continue;
        }
        if frames.windows(2).any(|pair| pair[0].frame == pair[1].frame) {
            return Err(invalid(
                "a LeRobot episode contains duplicate frame indexes",
            ));
        }
        if frames
            .windows(2)
            .any(|pair| pair[0].timestamp > pair[1].timestamp)
        {
            return Err(invalid(
                "a LeRobot episode's timestamps run backward after frame ordering",
            ));
        }
        let writer = staging_store.begin_in_dataset(
            dataset_id.to_owned(),
            "Imported — LeRobot dataset".to_owned(),
            Some("Imported from a LeRobot ZIP".to_owned()),
            fps,
            vec![EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                joints: joints.clone(),
            }],
            // An imported dataset carries no motor condition: it was recorded
            // on hardware this studio never saw, and inventing a temperature
            // for it would put a fabricated reading in a take file.
            Vec::new(),
        )?;
        let id = writer.id().to_owned();
        let mut last_time = 0.0_f64;
        for frame in frames.iter() {
            let normalized = frame
                .observation
                .iter()
                .zip(&joints)
                .map(|(value, joint)| from_lerobot_position(*value, joint))
                .collect::<Result<Vec<_>, _>>()?;
            let action = frame
                .action
                .iter()
                .zip(&joints)
                .map(|(value, joint)| from_lerobot_position(*value, joint).map(Some))
                .collect::<Result<Vec<_>, _>>()?;
            last_time = frame.timestamp.max(last_time);
            if !writer.push(Sample {
                seconds: frame.timestamp.max(0.0),
                normalized: vec![normalized.into_iter().map(Some).collect()],
                // Encoder counts are not part of the LeRobot contract. An
                // empty channel preserves their absence instead of inventing
                // zero readings that look like real hardware evidence.
                raw: Vec::new(),
                follow_error_degrees: None,
                velocity: None,
                effort: None,
                action: vec![Some(action)],
            }) {
                return Err(invalid(format!("could not write imported episode {id}")));
            }
        }
        let summary = writer.finish(
            last_time.max(frames.len() as f64 / fps),
            Vec::new(),
            None,
            false,
            Vec::new(),
        )?;
        first_episode.get_or_insert(summary.id);
        imported_episodes += 1;
        imported_frames += summary.frames;
        staged_files.push(summary.file);
    }

    Ok(StagedDatasetImport {
        staging,
        staged_files,
        summary: DatasetImportSummary {
            episodes: imported_episodes,
            frames: imported_frames,
            first_episode,
        },
    })
}

/// Publish every staged take as one catalog transaction.
pub(crate) fn commit_staged_lerobot_import(
    store: &EpisodeStore,
    staged: StagedDatasetImport,
) -> Result<DatasetImportSummary, EpisodeError> {
    let StagedDatasetImport {
        staging,
        staged_files,
        summary,
    } = staged;
    // Hard links publish without overwriting any externally-created file. If
    // any link or durability step fails, remove only links created by this
    // transaction; the staged originals remain available until cleanup.
    let mut published = Vec::with_capacity(staged_files.len());
    for file in &staged_files {
        let source = staging.path().join(file);
        let destination = store.directory().join(file);
        if let Err(source_error) = fs::hard_link(&source, &destination) {
            for published in &published {
                let _ = fs::remove_file(published);
            }
            return Err(EpisodeError::Io {
                path: destination,
                source: source_error,
            });
        }
        published.push(destination);
    }
    let durability = File::open(store.directory()).and_then(|directory| directory.sync_all());
    if let Err(source) = durability {
        for published in &published {
            let _ = fs::remove_file(published);
        }
        return Err(EpisodeError::Io {
            path: store.directory().to_owned(),
            source,
        });
    }
    if let Err(source) = staging.close() {
        for published in &published {
            let _ = fs::remove_file(published);
        }
        return Err(EpisodeError::Io {
            path: store.directory().to_owned(),
            source,
        });
    }
    if let Err(error) = store.reload() {
        for published in &published {
            let _ = fs::remove_file(published);
        }
        let _ = store.reload();
        return Err(error);
    }

    Ok(summary)
}

fn read_export_episode(
    store: &EpisodeStore,
    summary: &spiral_robotics_contracts::api::EpisodeSummary,
) -> Result<ExportEpisode, EpisodeError> {
    let completed = store.completed(&summary.id)?;
    let arms = &completed.start.arms;
    let arm_index = arms
        .iter()
        .position(|arm| arm.id.to_ascii_lowercase().contains("follower"))
        .or_else(|| (arms.len() == 1).then_some(0))
        .ok_or_else(|| invalid(format!("{} has no unambiguous follower arm", summary.id)))?;
    let joints = arms[arm_index].joints.clone();
    let capacity = frame_capacity(summary.frames);
    let mut positions = Vec::with_capacity(capacity);
    let mut actions = Vec::with_capacity(capacity);
    let mut velocity = Vec::with_capacity(capacity);
    let mut effort = Vec::with_capacity(capacity);
    // One missing frame disqualifies the whole take rather than the frame: a
    // column that is present for the first half of an episode and absent for
    // the second is worse than no column at all.
    let mut complete_velocity = true;
    let mut complete_effort = true;
    for sample in &completed.samples {
        let arm_values = |values: &Vec<Vec<Option<f64>>>| -> Option<Vec<f32>> {
            values
                .get(arm_index)?
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .map(|values| values.into_iter().map(|value| value as f32).collect())
        };
        if let Some(pose) = arm_values(&sample.normalized) {
            positions.push(pose);
        }
        // An action is the command the policy should learn, not an observation.
        // Legacy takes that did not record commands remain viewable/replayable,
        // but cannot truthfully become supervised training data.
        if let Some(action) = sample
            .action
            .get(arm_index)
            .and_then(Option::as_ref)
            .and_then(|values| values.iter().copied().collect::<Option<Vec<_>>>())
            .map(|values| values.into_iter().map(|value| value as f32).collect())
        {
            actions.push(action);
        }
        if let Some(frame_velocity) = sample.velocity.as_ref().and_then(arm_values) {
            velocity.push(frame_velocity);
        } else {
            complete_velocity = false;
        }
        if let Some(frame_effort) = sample.effort.as_ref().and_then(arm_values) {
            effort.push(frame_effort);
        } else {
            complete_effort = false;
        }
    }
    Ok(ExportEpisode {
        id: summary.id.clone(),
        frames: completed.samples.len() as u64,
        joints,
        velocity: complete_velocity.then_some(velocity),
        effort: complete_effort.then_some(effort),
        positions,
        actions,
        sample_hz: summary.sample_hz,
    })
}

#[allow(clippy::too_many_arguments)]
fn data_parquet(
    dimension: usize,
    state: Vec<f32>,
    action: Vec<f32>,
    velocity: Option<Vec<f32>>,
    effort: Option<Vec<f32>>,
    timestamp: Vec<f32>,
    frame_index: Vec<i64>,
    episode_index: Vec<i64>,
    index: Vec<i64>,
) -> Result<Vec<u8>, EpisodeError> {
    let item = Arc::new(Field::new("item", DataType::Float32, false));
    let vector = |values: Vec<f32>| -> Result<ArrayRef, EpisodeError> {
        Ok(Arc::new(
            FixedSizeListArray::try_new(
                item.clone(),
                dimension as i32,
                Arc::new(Float32Array::from(values)),
                None,
            )
            .map_err(|error| invalid(error.to_string()))?,
        ) as ArrayRef)
    };
    let vector_field = |name: &str| {
        Field::new(
            name,
            DataType::FixedSizeList(item.clone(), dimension as i32),
            false,
        )
    };

    let rows = timestamp.len();
    let mut fields = vec![vector_field("observation.state"), vector_field("action")];
    let mut columns = vec![vector(state)?, vector(action)?];
    // Declared together or not at all, in the order `features` names them, so
    // the schema and the metadata cannot drift apart.
    if let (Some(velocity), Some(effort)) = (velocity, effort) {
        fields.push(vector_field("observation.velocity"));
        fields.push(vector_field("observation.effort"));
        columns.push(vector(velocity)?);
        columns.push(vector(effort)?);
    }
    fields.extend([
        Field::new("timestamp", DataType::Float32, false),
        Field::new("frame_index", DataType::Int64, false),
        Field::new("episode_index", DataType::Int64, false),
        Field::new("index", DataType::Int64, false),
        Field::new("task_index", DataType::Int64, false),
    ]);
    columns.extend([
        Arc::new(Float32Array::from(timestamp)) as ArrayRef,
        Arc::new(Int64Array::from(frame_index)),
        Arc::new(Int64Array::from(episode_index)),
        Arc::new(Int64Array::from(index)),
        Arc::new(Int64Array::from(vec![0; rows])),
    ]);

    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|error| invalid(error.to_string()))?;
    parquet_bytes(schema, batch)
}

/// The dataset card, which has to describe the archive that was actually made.
///
/// A reader who finds no effort columns deserves to know whether this studio
/// cannot record them or whether one take in the session could not, so the card
/// says which, and how many takes were the reason.
fn dataset_card(proprioception: bool, without_proprioception: u64) -> String {
    let mut card = String::from(
        "---\ntags:\n- LeRobot\n- robotics\n---\n\n# Spiral Robotics dataset\n\n         LeRobot v3 low-dimensional SO-ARM101 data exported by Spiral Robotics Studio.          Camera video is not included because Spiral Robotics does not record camera          streams yet.\n\n",
    );
    if proprioception {
        card.push_str(
            "Every exported take carried the motors' own readings, so this              dataset declares `observation.velocity` in degrees per second and              `observation.effort` as a signed share of each motor's rated              torque, alongside `observation.state` and `action`.\n",
        );
    } else {
        card.push_str(&format!(
            "This dataset declares `observation.state` and `action` only.              {without_proprioception} of the exported takes carried no velocity              or effort — recorded before those were captured, or on a motor bus              that does not publish them — and LeRobot requires one feature set              for a whole dataset, so the columns were dropped rather than              filled in for the takes that had them.\n",
        ));
    }
    card
}

fn episodes_parquet(offsets: &[(i64, i64, i64)]) -> Result<Vec<u8>, EpisodeError> {
    let mut tasks = ListBuilder::new(StringBuilder::new());
    for _ in offsets {
        tasks.values().append_value(TASK);
        tasks.append(true);
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("episode_index", DataType::Int64, false),
        Field::new(
            "tasks",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("length", DataType::Int64, false),
        Field::new("dataset_from_index", DataType::Int64, false),
        Field::new("dataset_to_index", DataType::Int64, false),
        Field::new("data/chunk_index", DataType::Int64, false),
        Field::new("data/file_index", DataType::Int64, false),
        Field::new("meta/episodes/chunk_index", DataType::Int64, false),
        Field::new("meta/episodes/file_index", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(0..offsets.len() as i64)),
            Arc::new(tasks.finish()),
            Arc::new(Int64Array::from_iter_values(
                offsets.iter().map(|(_, _, length)| *length),
            )),
            Arc::new(Int64Array::from_iter_values(
                offsets.iter().map(|(from, _, _)| *from),
            )),
            Arc::new(Int64Array::from_iter_values(
                offsets.iter().map(|(_, to, _)| *to),
            )),
            Arc::new(Int64Array::from(vec![0; offsets.len()])),
            Arc::new(Int64Array::from(vec![0; offsets.len()])),
            Arc::new(Int64Array::from(vec![0; offsets.len()])),
            Arc::new(Int64Array::from(vec![0; offsets.len()])),
        ],
    )
    .map_err(|error| invalid(error.to_string()))?;
    parquet_bytes(schema, batch)
}

fn tasks_parquet() -> Result<Vec<u8>, EpisodeError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("task", DataType::Utf8, false),
        Field::new("task_index", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![TASK])),
            Arc::new(Int64Array::from(vec![0])),
        ],
    )
    .map_err(|error| invalid(error.to_string()))?;
    parquet_bytes(schema, batch)
}

fn parquet_bytes(schema: Arc<Schema>, batch: RecordBatch) -> Result<Vec<u8>, EpisodeError> {
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(Vec::new(), schema, Some(properties))
        .map_err(|error| invalid(error.to_string()))?;
    writer
        .write(&batch)
        .map_err(|error| invalid(error.to_string()))?;
    writer
        .into_inner()
        .map_err(|error| invalid(error.to_string()))
}

fn vector_feature(joints: &[String]) -> Value {
    let names = joints
        .iter()
        .map(|joint| format!("{joint}.pos"))
        .collect::<Vec<_>>();
    json!({ "dtype": "float32", "shape": [joints.len()], "names": names })
}

/// A per-joint vector feature whose names carry the quantity they measure.
///
/// LeRobot names position columns `<joint>.pos`; velocity and effort follow the
/// same shape so a reader can tell three same-width vectors apart by name.
fn vector_feature_named(joints: &[String], suffix: &str) -> Value {
    let names = joints
        .iter()
        .map(|joint| format!("{joint}.{suffix}"))
        .collect::<Vec<_>>();
    json!({ "dtype": "float32", "shape": [joints.len()], "names": names })
}

fn scalar_feature(dtype: &str) -> Value {
    json!({ "dtype": dtype, "shape": [1], "names": Value::Null })
}

#[allow(clippy::too_many_arguments)]
fn stats_document(
    dimension: usize,
    states: &[f32],
    actions: &[f32],
    velocities: Option<&[f32]>,
    efforts: Option<&[f32]>,
    timestamps: &[f32],
    frames: &[i64],
    episodes: &[i64],
    indices: &[i64],
) -> Value {
    let mut stats = json!({
        "observation.state": vector_stats(states, dimension),
        "action": vector_stats(actions, dimension),
        "timestamp": scalar_stats(timestamps.iter().map(|value| *value as f64)),
        "frame_index": scalar_stats(frames.iter().map(|value| *value as f64)),
        "episode_index": scalar_stats(episodes.iter().map(|value| *value as f64)),
        "index": scalar_stats(indices.iter().map(|value| *value as f64)),
        "task_index": scalar_stats(std::iter::repeat_n(0.0, timestamps.len())),
    });
    if let (Some(velocities), Some(efforts)) = (velocities, efforts) {
        let stats = stats.as_object_mut().expect("the stats map is an object");
        stats.insert(
            "observation.velocity".to_owned(),
            vector_stats(velocities, dimension),
        );
        stats.insert(
            "observation.effort".to_owned(),
            vector_stats(efforts, dimension),
        );
    }
    stats
}

fn vector_stats(values: &[f32], dimension: usize) -> Value {
    let mut min = vec![f64::INFINITY; dimension];
    let mut max = vec![f64::NEG_INFINITY; dimension];
    let mut sum = vec![0.0; dimension];
    for row in values.chunks_exact(dimension) {
        for (index, value) in row.iter().enumerate() {
            let value = *value as f64;
            min[index] = min[index].min(value);
            max[index] = max[index].max(value);
            sum[index] += value;
        }
    }
    let count = values.len() / dimension;
    let mean = sum
        .into_iter()
        .map(|sum| sum / count.max(1) as f64)
        .collect::<Vec<_>>();
    let mut variance = vec![0.0; dimension];
    for row in values.chunks_exact(dimension) {
        for (index, value) in row.iter().enumerate() {
            variance[index] += (*value as f64 - mean[index]).powi(2);
        }
    }
    let std = variance
        .into_iter()
        .map(|sum| (sum / count.max(1) as f64).sqrt())
        .collect::<Vec<_>>();
    json!({ "min": min, "max": max, "mean": mean, "std": std, "count": [count] })
}

fn scalar_stats(values: impl Iterator<Item = f64>) -> Value {
    let values = values.collect::<Vec<_>>();
    let count = values.len();
    let min = values.iter().copied().reduce(f64::min).unwrap_or(0.0);
    let max = values.iter().copied().reduce(f64::max).unwrap_or(0.0);
    let mean = values.iter().sum::<f64>() / count.max(1) as f64;
    let std = (values
        .iter()
        .map(|value| (*value - mean).powi(2))
        .sum::<f64>()
        / count.max(1) as f64)
        .sqrt();
    json!({ "min": [min], "max": [max], "mean": [mean], "std": [std], "count": [count] })
}

fn pretty_json(value: &Value) -> Result<String, EpisodeError> {
    serde_json::to_string_pretty(value)
        .map(|text| format!("{text}\n"))
        .map_err(|error| invalid(error.to_string()))
}

fn write_zip_entry<W: Write + std::io::Seek>(
    archive: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    name: &str,
    bytes: &[u8],
) -> Result<(), EpisodeError> {
    archive
        .start_file(name, options)
        .map_err(|error| invalid(error.to_string()))?;
    archive
        .write_all(bytes)
        .map_err(|error| invalid(error.to_string()))
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, EpisodeError> {
    let mut file = archive
        .by_name(name)
        .map_err(|error| invalid(format!("could not read {name}: {error}")))?;
    if file.is_dir() {
        return Err(invalid(format!("{name} is a directory, not a file")));
    }
    validate_archive_entry_metadata(name, file.size(), file.compressed_size())?;
    let mut bytes = Vec::with_capacity(file.size().min(16 * 1024 * 1024) as usize);
    (&mut file)
        .take(MAX_ARCHIVE_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("could not read {name}: {error}")))?;
    if bytes.len() as u64 > MAX_ARCHIVE_ENTRY_BYTES {
        return Err(invalid(format!(
            "{name} expands beyond the supported {} MiB entry limit",
            MAX_ARCHIVE_ENTRY_BYTES / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

fn validated_archive_names<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Vec<String>, EpisodeError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid(format!(
            "the ZIP contains {} entries; at most {MAX_ARCHIVE_ENTRIES} are supported",
            archive.len()
        )));
    }
    let mut names = Vec::with_capacity(archive.len());
    let mut seen = BTreeSet::new();
    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|error| invalid(format!("could not inspect ZIP entry {index}: {error}")))?;
        let name = file.name().to_owned();
        validate_archive_entry_metadata(&name, file.size(), file.compressed_size())?;
        total_uncompressed = total_uncompressed.checked_add(file.size()).ok_or_else(|| {
            invalid("the ZIP's declared uncompressed size is too large to represent")
        })?;
        if total_uncompressed > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(invalid(format!(
                "the ZIP expands beyond the supported {} MiB archive limit",
                MAX_ARCHIVE_UNCOMPRESSED_BYTES / (1024 * 1024)
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(invalid(format!(
                "the ZIP contains duplicate entry {name:?}; import is ambiguous"
            )));
        }
        names.push(name);
    }
    Ok(names)
}

fn validate_archive_entry_metadata(
    name: &str,
    uncompressed: u64,
    compressed: u64,
) -> Result<(), EpisodeError> {
    let path = Path::new(name);
    if name.is_empty()
        || name.chars().count() > 512
        || name.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err(invalid(format!(
            "the ZIP contains unsafe entry name {name:?}"
        )));
    }
    if uncompressed > MAX_ARCHIVE_ENTRY_BYTES {
        return Err(invalid(format!(
            "{name} declares {} MiB uncompressed; the per-entry limit is {} MiB",
            uncompressed.div_ceil(1024 * 1024),
            MAX_ARCHIVE_ENTRY_BYTES / (1024 * 1024)
        )));
    }
    if uncompressed >= COMPRESSION_RATIO_CHECK_BYTES
        && (compressed == 0 || uncompressed / compressed.max(1) > MAX_ARCHIVE_COMPRESSION_RATIO)
    {
        return Err(invalid(format!(
            "{name} has an unsafe compression ratio above {MAX_ARCHIVE_COMPRESSION_RATIO}:1"
        )));
    }
    Ok(())
}

fn declared_video_features(info: &Value) -> Vec<String> {
    info.get("features")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|features| features.iter())
        .filter_map(|(name, feature)| {
            let dtype = feature
                .get("dtype")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            (name.starts_with("observation.images")
                || name.starts_with("observation.image")
                || dtype == "video"
                || dtype == "image")
                .then(|| name.clone())
        })
        .collect()
}

fn action_names(info: &Value) -> Vec<String> {
    let Some(names) = info
        .get("features")
        .and_then(|features| features.get("action"))
        .and_then(|action| action.get("names"))
    else {
        return Vec::new();
    };
    let mut flattened = Vec::new();
    flatten_strings(names, &mut flattened);
    flattened
}

fn flatten_strings(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(value) => output.push(value.clone()),
        Value::Array(values) => {
            for value in values {
                flatten_strings(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                flatten_strings(value, output);
            }
        }
        _ => {}
    }
}

fn normalized_joint_names(
    names: Vec<String>,
    dimension: usize,
    robot_type: &str,
) -> Result<Vec<String>, EpisodeError> {
    if dimension != SO101_JOINTS.len() {
        return Err(invalid(format!(
            "this studio can replay six-axis SO-101 datasets, but the imported action has {dimension} values"
        )));
    }

    let canonical = SO101_JOINTS
        .iter()
        .map(|joint| (*joint).to_owned())
        .collect::<Vec<_>>();
    let declared_so101 = {
        let robot_type = robot_type.to_ascii_lowercase();
        robot_type.contains("so101") || robot_type.contains("so100")
    };
    if names.is_empty() {
        return declared_so101.then_some(canonical).ok_or_else(|| {
            invalid(
                "the dataset does not identify an SO-101 robot or declare recognizable SO-101 action names",
            )
        });
    }
    if names.len() != dimension {
        return Err(invalid(format!(
            "the dataset declares {} action names for {dimension} values",
            names.len()
        )));
    }

    let cleaned = names
        .iter()
        .map(|name| name.strip_suffix(".pos").unwrap_or(name).to_owned())
        .collect::<Vec<_>>();
    if cleaned == canonical {
        Ok(canonical)
    } else {
        Err(invalid(format!(
            "the dataset's action names ({}) do not match the SO-101 joint layout",
            names.join(", ")
        )))
    }
}

fn vector_at(array: &dyn Array, row: usize) -> Result<Vec<f64>, EpisodeError> {
    if let Some(array) = array.as_any().downcast_ref::<FixedSizeListArray>() {
        return primitive_vector(array.value(row).as_ref());
    }
    if let Some(array) = array.as_any().downcast_ref::<ListArray>() {
        return primitive_vector(array.value(row).as_ref());
    }
    if let Some(array) = array.as_any().downcast_ref::<LargeListArray>() {
        return primitive_vector(array.value(row).as_ref());
    }
    Ok(vec![number_at(array, row)?])
}

fn primitive_vector(array: &dyn Array) -> Result<Vec<f64>, EpisodeError> {
    (0..array.len()).map(|row| number_at(array, row)).collect()
}

fn number_at(array: &dyn Array, row: usize) -> Result<f64, EpisodeError> {
    if array.is_null(row) {
        return Err(invalid("LeRobot action data contains a null value"));
    }
    if let Some(array) = array.as_any().downcast_ref::<Float32Array>() {
        return Ok(array.value(row) as f64);
    }
    if let Some(array) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(array.value(row));
    }
    if let Some(array) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(array.value(row) as f64);
    }
    if let Some(array) = array.as_any().downcast_ref::<Int32Array>() {
        return Ok(array.value(row) as f64);
    }
    Err(invalid(format!(
        "unsupported LeRobot numeric column type {:?}",
        array.data_type()
    )))
}

fn integer_at(array: &dyn Array, row: usize) -> Result<i64, EpisodeError> {
    if array.is_null(row) {
        return Err(invalid("LeRobot index data contains a null value"));
    }
    if let Some(array) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(array.value(row));
    }
    if let Some(array) = array.as_any().downcast_ref::<Int32Array>() {
        return Ok(array.value(row) as i64);
    }
    if let Some(array) = array.as_any().downcast_ref::<UInt64Array>() {
        return i64::try_from(array.value(row)).map_err(|_| invalid("LeRobot index is too large"));
    }
    if let Some(array) = array.as_any().downcast_ref::<UInt32Array>() {
        return Ok(array.value(row) as i64);
    }
    Err(invalid(format!(
        "unsupported LeRobot index column type {:?}",
        array.data_type()
    )))
}

fn to_lerobot_position(value: f32, joint: &str) -> f32 {
    if is_gripper(joint) {
        (value.clamp(-1.0, 1.0) + 1.0) * 50.0
    } else {
        value.clamp(-1.0, 1.0) * 100.0
    }
}

fn from_lerobot_position(value: f64, joint: &str) -> Result<f64, EpisodeError> {
    if !value.is_finite() {
        return Err(invalid("LeRobot action data contains a non-finite value"));
    }
    let normalized = if is_gripper(joint) {
        if !(-2.0..=102.0).contains(&value) {
            return Err(invalid(format!(
                "gripper value {value:.2} is outside LeRobot's 0–100 SO-101 range"
            )));
        }
        value / 50.0 - 1.0
    } else {
        if !(-102.0..=102.0).contains(&value) {
            return Err(invalid(format!(
                "joint value {value:.2} is outside LeRobot's -100–100 SO-101 range"
            )));
        }
        value / 100.0
    };
    Ok(normalized.clamp(-1.0, 1.0))
}

fn is_gripper(joint: &str) -> bool {
    joint.to_ascii_lowercase().contains("gripper")
}

fn invalid(message: impl Into<String>) -> EpisodeError {
    EpisodeError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn recorded_store() -> (TempDir, EpisodeStore) {
        let directory = TempDir::new().unwrap();
        let store = EpisodeStore::open(directory.path().join("episodes")).unwrap();
        record_take(&store, true);
        store.reload().unwrap();
        (directory, store)
    }

    /// Write one four-frame take, with or without the motors' own readings.
    fn record_take(store: &EpisodeStore, proprioception: bool) {
        record_take_with_actions(store, proprioception, true);
    }

    fn record_take_with_actions(store: &EpisodeStore, proprioception: bool, actions: bool) {
        let writer = store
            .begin(
                "Teleoperation".to_owned(),
                None,
                30.0,
                vec![EpisodeArm {
                    id: "follower".to_owned(),
                    robot_type: Some("so101_follower".to_owned()),
                    joints: SO101_JOINTS
                        .iter()
                        .map(|joint| (*joint).to_owned())
                        .collect(),
                }],
                Vec::new(),
            )
            .unwrap();
        for frame in 0..4 {
            writer.push(Sample {
                seconds: frame as f64 / 30.0,
                normalized: vec![vec![Some(frame as f64 / 10.0); SO101_JOINTS.len()]],
                raw: vec![vec![0; SO101_JOINTS.len()]],
                follow_error_degrees: None,
                velocity: proprioception
                    .then(|| vec![vec![Some(frame as f64); SO101_JOINTS.len()]]),
                effort: proprioception.then(|| vec![vec![Some(0.25); SO101_JOINTS.len()]]),
                // Keep the commanded action visibly different from measured
                // state so this fixture catches accidental state-as-action
                // exports.
                action: if actions {
                    vec![Some(vec![
                        Some(frame as f64 / 10.0 + 0.5);
                        SO101_JOINTS.len()
                    ])]
                } else {
                    Vec::new()
                },
            });
        }
        writer
            .finish(0.1, Vec::new(), None, false, Vec::new())
            .unwrap();
    }

    /// Read `meta/info.json` and `README.md` out of an archive.
    fn archive_metadata(bytes: &[u8]) -> (Value, String) {
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let info: Value = {
            let mut file = zip.by_name("meta/info.json").unwrap();
            serde_json::from_reader(&mut file).unwrap()
        };
        let mut card = String::new();
        zip.by_name("README.md")
            .unwrap()
            .read_to_string(&mut card)
            .unwrap();
        (info, card)
    }

    /// Column names actually present in the archive's Parquet data.
    fn archive_columns(bytes: &[u8]) -> Vec<String> {
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut parquet = Vec::new();
        zip.by_name("data/chunk-000/file-000.parquet")
            .unwrap()
            .read_to_end(&mut parquet)
            .unwrap();
        ParquetRecordBatchReaderBuilder::try_new(Bytes::from(parquet))
            .unwrap()
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    }

    fn action_only_archive() -> Vec<u8> {
        let item = Arc::new(Field::new("item", DataType::Float32, false));
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "action",
                DataType::FixedSizeList(item.clone(), SO101_JOINTS.len() as i32),
                false,
            ),
            Field::new("timestamp", DataType::Float32, false),
            Field::new("frame_index", DataType::Int64, false),
            Field::new("episode_index", DataType::Int64, false),
        ]));
        let action = FixedSizeListArray::try_new(
            item,
            SO101_JOINTS.len() as i32,
            Arc::new(Float32Array::from(vec![0.0; SO101_JOINTS.len()])),
            None,
        )
        .unwrap();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(action),
                Arc::new(Float32Array::from(vec![0.0])),
                Arc::new(Int64Array::from(vec![0])),
                Arc::new(Int64Array::from(vec![0])),
            ],
        )
        .unwrap();
        let parquet = parquet_bytes(schema, batch).unwrap();
        let info = json!({
            "codebase_version": "v3.0",
            "robot_type": "so101_follower",
            "fps": 30,
            "features": {
                "action": vector_feature(&SO101_JOINTS.iter().map(|joint| (*joint).to_owned()).collect::<Vec<_>>())
            }
        });
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        write_zip_entry(
            &mut archive,
            options,
            "meta/info.json",
            pretty_json(&info).unwrap().as_bytes(),
        )
        .unwrap();
        write_zip_entry(
            &mut archive,
            options,
            "data/chunk-000/file-000.parquet",
            &parquet,
        )
        .unwrap();
        archive.finish().unwrap().into_inner()
    }

    fn two_episode_archive_with_invalid_second() -> Vec<u8> {
        let item = Arc::new(Field::new("item", DataType::Float32, false));
        let vector = || DataType::FixedSizeList(item.clone(), SO101_JOINTS.len() as i32);
        let schema = Arc::new(Schema::new(vec![
            Field::new("observation.state", vector(), false),
            Field::new("action", vector(), false),
            Field::new("timestamp", DataType::Float32, false),
            Field::new("frame_index", DataType::Int64, false),
            Field::new("episode_index", DataType::Int64, false),
        ]));
        let lists = |values: Vec<f32>| {
            Arc::new(
                FixedSizeListArray::try_new(
                    item.clone(),
                    SO101_JOINTS.len() as i32,
                    Arc::new(Float32Array::from(values)),
                    None,
                )
                .unwrap(),
            ) as ArrayRef
        };
        let mut actions = vec![0.0; SO101_JOINTS.len() * 2];
        actions[SO101_JOINTS.len()] = 1_000.0;
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                lists(vec![0.0; SO101_JOINTS.len() * 2]),
                lists(actions),
                Arc::new(Float32Array::from(vec![0.0, 0.0])),
                Arc::new(Int64Array::from(vec![0, 0])),
                Arc::new(Int64Array::from(vec![0, 1])),
            ],
        )
        .unwrap();
        let parquet = parquet_bytes(schema, batch).unwrap();
        let info = json!({
            "codebase_version": "v3.0",
            "robot_type": "so101_follower",
            "fps": 30,
            "features": {
                "observation.state": vector_feature(&SO101_JOINTS.iter().map(|joint| (*joint).to_owned()).collect::<Vec<_>>()),
                "action": vector_feature(&SO101_JOINTS.iter().map(|joint| (*joint).to_owned()).collect::<Vec<_>>())
            }
        });
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        write_zip_entry(
            &mut archive,
            options,
            "meta/info.json",
            pretty_json(&info).unwrap().as_bytes(),
        )
        .unwrap();
        write_zip_entry(
            &mut archive,
            options,
            "data/chunk-000/file-000.parquet",
            &parquet,
        )
        .unwrap();
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn a_session_that_recorded_the_motors_exports_velocity_and_effort() {
        let (_directory, store) = recorded_store();
        let archive = export_lerobot_archive(&store).unwrap();
        assert_eq!(archive.without_proprioception, 0);

        let (info, card) = archive_metadata(&archive.bytes);
        let features = &info["features"];
        assert_eq!(
            features["observation.velocity"]["names"][0],
            format!("{}.vel", SO101_JOINTS[0])
        );
        assert_eq!(
            features["observation.effort"]["names"][0],
            format!("{}.effort", SO101_JOINTS[0])
        );
        assert_eq!(features["observation.velocity"]["shape"][0], 6);
        assert!(card.contains("observation.effort"));

        // Declared features and the Parquet schema are written in two separate
        // places and have to agree, or a reader that trusts the metadata finds
        // a column that is not there.
        let columns = archive_columns(&archive.bytes);
        for declared in features.as_object().unwrap().keys() {
            assert!(
                columns.contains(declared),
                "{declared} is declared in info.json but absent from the data: {columns:?}"
            );
        }
        assert!(columns.contains(&"observation.effort".to_owned()));

        let stats: Value = {
            let mut zip = ZipArchive::new(Cursor::new(archive.bytes.as_slice())).unwrap();
            let mut file = zip.by_name("meta/stats.json").unwrap();
            serde_json::from_reader(&mut file).unwrap()
        };
        assert!(stats["observation.velocity"].is_object());
    }

    #[test]
    fn one_take_without_the_motors_readings_costs_the_whole_export_its_columns() {
        let directory = TempDir::new().unwrap();
        let store = EpisodeStore::open(directory.path().join("episodes")).unwrap();
        record_take(&store, true);
        record_take(&store, false);
        store.reload().unwrap();

        let archive = export_lerobot_archive(&store).unwrap();
        assert_eq!(archive.episodes, 2, "both takes are still exported");
        assert_eq!(archive.without_proprioception, 1);

        let (info, card) = archive_metadata(&archive.bytes);
        // Filling the gap with zeroes would put fabricated effort into a
        // training set, which is worse than shipping fewer columns.
        assert!(info["features"]["observation.effort"].is_null());
        assert!(info["features"]["observation.velocity"].is_null());
        assert!(info["features"]["action"].is_object());
        assert!(
            card.contains("1 of the exported takes"),
            "the dataset card says how many takes were the reason: {card}"
        );
    }

    #[test]
    fn legacy_take_without_actions_is_not_exported_with_fabricated_labels() {
        let directory = TempDir::new().unwrap();
        let store = EpisodeStore::open(directory.path().join("episodes")).unwrap();
        record_take_with_actions(&store, true, false);
        store.reload().unwrap();

        let error = match export_lerobot_archive(&store) {
            Ok(_) => panic!("observation.state must never be substituted for a missing action"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("no recorded follower action"),
            "{error}"
        );
    }

    #[test]
    fn state_and_action_statistics_describe_their_own_values() {
        let stats = stats_document(
            1,
            &[1.0, 3.0],
            &[10.0, 30.0],
            None,
            None,
            &[0.0, 1.0],
            &[0, 1],
            &[0, 0],
            &[0, 1],
        );
        assert_eq!(stats["observation.state"]["mean"][0], 2.0);
        assert_eq!(stats["action"]["mean"][0], 20.0);
    }

    #[test]
    fn exported_archive_is_lerobot_v3_and_imports_again() {
        let (_source_directory, source) = recorded_store();
        let archive = export_lerobot_archive(&source).unwrap();
        assert_eq!(archive.episodes, 1);
        assert_eq!(archive.frames, 4);

        let mut zip = ZipArchive::new(Cursor::new(archive.bytes.as_slice())).unwrap();
        let info: Value = {
            let mut file = zip.by_name("meta/info.json").unwrap();
            serde_json::from_reader(&mut file).unwrap()
        };
        assert_eq!(
            action_names(&info),
            SO101_JOINTS
                .iter()
                .map(|joint| format!("{joint}.pos"))
                .collect::<Vec<_>>()
        );
        assert!(info.get("video_path").is_none());
        assert!(info.get("video_files_size_in_mb").is_none());
        assert!(zip.by_name("data/chunk-000/file-000.parquet").is_ok());

        let target_directory = TempDir::new().unwrap();
        let target = EpisodeStore::open(target_directory.path().join("episodes")).unwrap();
        let imported = import_lerobot_archive(&target, &archive.bytes).unwrap();
        assert_eq!(imported.episodes, 1);
        assert_eq!(imported.frames, 4);
        let trajectory = target
            .trajectory_at(imported.first_episode.as_deref().unwrap(), "follower", 0.0)
            .unwrap()
            .frames;
        assert_eq!(trajectory[0].len(), SO101_JOINTS.len());
        assert!(
            SO101_JOINTS
                .iter()
                .all(|joint| trajectory[0].contains_key(*joint))
        );
        assert!(trajectory[0].keys().all(|joint| !joint.ends_with(".pos")));
        assert!((trajectory[3]["shoulder_pan"] - 0.3).abs() < 0.001);
        assert!((trajectory[3]["gripper"] - 0.3).abs() < 0.001);

        let completed = target
            .completed(imported.first_episode.as_deref().unwrap())
            .unwrap();
        assert!((completed.samples[3].normalized[0][0].unwrap() - 0.3).abs() < 0.001);
        assert!((completed.samples[3].action[0].as_ref().unwrap()[0].unwrap() - 0.8).abs() < 0.001);
        assert!(
            completed.samples[3].raw.is_empty(),
            "unavailable encoder counts stay absent instead of becoming fake zeroes"
        );
    }

    #[test]
    fn file_backed_archive_export_and_import_avoid_an_archive_sized_memory_copy() {
        let (_source_directory, source) = recorded_store();
        let archive_file = tempfile::NamedTempFile::new().unwrap();
        let summary = export_lerobot_archive_to_file_for_dataset(
            &source,
            None,
            archive_file.reopen().unwrap(),
        )
        .unwrap();
        assert_eq!(summary.episodes, 1);
        assert!(archive_file.as_file().metadata().unwrap().len() > 0);

        let target_directory = TempDir::new().unwrap();
        let target = EpisodeStore::open(target_directory.path().join("episodes")).unwrap();
        let imported = import_lerobot_archive_file_into_dataset(
            &target,
            archive_file.reopen().unwrap(),
            "default",
        )
        .unwrap();
        assert_eq!(imported.episodes, 1);
        assert_eq!(imported.frames, 4);
    }

    #[test]
    fn application_authorized_prepared_export_encodes_the_inspected_selection() {
        let (_source_directory, source) = recorded_store();
        let selected = source
            .list()
            .into_iter()
            .map(|episode| episode.id)
            .collect::<Vec<_>>();
        let prepared = prepare_lerobot_export(&source, &selected).unwrap();
        assert_eq!(prepared.inspections().len(), 1);
        assert!(prepared.inspections()[0].complete_observations);
        assert!(prepared.inspections()[0].complete_actions);

        let archive_file = tempfile::NamedTempFile::new().unwrap();
        export_prepared_lerobot_to_file(prepared, archive_file.reopen().unwrap(), 60, true)
            .unwrap();
        let (info, _) = archive_metadata(&std::fs::read(archive_file.path()).unwrap());
        assert_eq!(info["total_episodes"], 1);
        assert_eq!(info["total_frames"], 4);
        assert_eq!(info["fps"], 60);
    }

    #[test]
    fn import_refuses_action_only_data_instead_of_fabricating_observations() {
        let directory = TempDir::new().unwrap();
        let store = EpisodeStore::open(directory.path().join("episodes")).unwrap();
        let error = import_lerobot_archive(&store, &action_only_archive())
            .expect_err("action-only datasets are not truthful local episodes");
        assert!(error.to_string().contains("observation.state"), "{error}");
        assert!(store.list().is_empty());
    }

    #[test]
    fn invalid_later_episode_does_not_partially_publish_earlier_imports() {
        let directory = TempDir::new().unwrap();
        let store = EpisodeStore::open(directory.path().join("episodes")).unwrap();
        let error = import_lerobot_archive(&store, &two_episode_archive_with_invalid_second())
            .expect_err("the second episode is outside the SO-101 action range");
        assert!(error.to_string().contains("outside LeRobot"), "{error}");
        store.reload().unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn staged_import_is_invisible_and_a_late_publish_collision_rolls_back_every_link() {
        let source_directory = TempDir::new().unwrap();
        let source = EpisodeStore::open(source_directory.path().join("episodes")).unwrap();
        record_take(&source, true);
        record_take(&source, true);
        source.reload().unwrap();
        let archive = export_lerobot_archive(&source).unwrap();

        let target_directory = TempDir::new().unwrap();
        let target = EpisodeStore::open(target_directory.path().join("episodes")).unwrap();
        let staged = stage_lerobot_archive_reader_into_dataset(
            &target,
            Cursor::new(archive.bytes),
            0,
            "default",
        )
        .unwrap();
        assert_eq!(staged.summary.episodes, 2);
        target.reload().unwrap();
        assert!(
            target.list().is_empty(),
            "staging must not publish even the first decoded take"
        );

        let collision = target.directory().join(&staged.staged_files[1]);
        std::fs::write(&collision, "external writer won this identity\n").unwrap();
        let error = commit_staged_lerobot_import(&target, staged)
            .expect_err("the second destination identity already exists");
        assert!(matches!(error, EpisodeError::Io { .. }));
        std::fs::remove_file(collision).unwrap();
        target.reload().unwrap();
        assert!(
            target.list().is_empty(),
            "the first published link must be removed when a later link fails"
        );
    }

    #[test]
    fn archive_metadata_limits_reject_unsafe_names_and_compression_bombs() {
        assert!(validate_archive_entry_metadata("../escape", 1, 1).is_err());
        assert!(
            validate_archive_entry_metadata(
                "data/file.parquet",
                COMPRESSION_RATIO_CHECK_BYTES * (MAX_ARCHIVE_COMPRESSION_RATIO + 1),
                COMPRESSION_RATIO_CHECK_BYTES,
            )
            .is_err()
        );
    }

    #[test]
    fn official_so101_names_are_canonicalized_for_replay() {
        let names = SO101_JOINTS
            .iter()
            .map(|joint| format!("{joint}.pos"))
            .collect::<Vec<_>>();
        assert_eq!(
            normalized_joint_names(names, SO101_JOINTS.len(), "so101_follower").unwrap(),
            SO101_JOINTS
                .iter()
                .map(|joint| (*joint).to_owned())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn lerobot_so101_units_round_trip() {
        assert_eq!(to_lerobot_position(-1.0, "shoulder_pan"), -100.0);
        assert_eq!(to_lerobot_position(1.0, "gripper"), 100.0);
        assert_eq!(from_lerobot_position(-100.0, "shoulder_pan").unwrap(), -1.0);
        assert_eq!(from_lerobot_position(1.0, "shoulder_pan").unwrap(), 0.01);
        assert_eq!(from_lerobot_position(0.0, "gripper").unwrap(), -1.0);
        assert_eq!(from_lerobot_position(100.0, "gripper").unwrap(), 1.0);
    }
}
