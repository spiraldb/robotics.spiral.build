//! LeRobot dataset interchange.
//!
//! The studio keeps its live recording format deliberately simple and
//! self-describing. Interchange is a boundary concern: completed episodes are
//! projected into the current LeRobot v3 tabular layout and packaged as one
//! ZIP, while imported v2/v3 Parquet archives are converted back into local
//! episodes before replay. Camera video is not invented here—the recorder does
//! not capture it yet—so exports honestly contain low-dimensional state and
//! action data only.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
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
use rerobot_core::api::{EpisodeArm, ImportResponse};
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::episodes::{EpisodeError, EpisodeStore, Sample};

const TASK: &str = "Recorded with rerobot";
const SO101_JOINTS: [&str; 6] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
    "gripper",
];

/// A complete downloadable dataset package.
pub struct DatasetArchive {
    pub bytes: Vec<u8>,
    pub episodes: u64,
    pub frames: u64,
    pub skipped: u64,
}

#[derive(Debug)]
struct ExportEpisode {
    joints: Vec<String>,
    positions: Vec<Vec<f32>>,
    sample_hz: f64,
}

#[derive(Debug)]
struct ImportedFrame {
    frame: i64,
    timestamp: f64,
    action: Vec<f64>,
}

/// Export every complete, non-cancelled episode as a LeRobot v3 ZIP.
pub fn export_lerobot_archive(store: &EpisodeStore) -> Result<DatasetArchive, EpisodeError> {
    let listed = store.list();
    let candidates = listed
        .iter()
        .filter(|episode| episode.usable() && !episode.discarded)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(invalid("there are no recorded episodes to download"));
    }

    let mut skipped = listed.len().saturating_sub(candidates.len()) as u64;
    let mut episodes = Vec::new();
    for summary in candidates {
        match read_export_episode(store, summary) {
            Ok(episode) if !episode.positions.is_empty() => episodes.push(episode),
            Ok(_) | Err(_) => skipped += 1,
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

    let episode_count = episodes.len();
    let total_frames = episodes
        .iter()
        .map(|episode| episode.positions.len())
        .sum::<usize>();
    let dimension = joints.len();
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
            actions.extend(
                pose.iter()
                    .zip(&joints)
                    .map(|(value, joint)| to_lerobot_position(*value, joint)),
            );
            timestamps.push(frame_index as f32 / fps as f32);
            frame_indices.push(frame_index as i64);
            episode_indices.push(episode_index as i64);
            indices.push(cursor);
            cursor += 1;
        }
        episode_offsets.push((from, cursor, episode.positions.len() as i64));
    }

    let state_values = actions.clone();
    let data = data_parquet(
        dimension,
        state_values,
        actions.clone(),
        timestamps.clone(),
        frame_indices.clone(),
        episode_indices.clone(),
        indices.clone(),
    )?;
    let episode_meta = episodes_parquet(&episode_offsets)?;
    let tasks = tasks_parquet()?;

    let features = json!({
        "observation.state": vector_feature(&joints),
        "action": vector_feature(&joints),
        "timestamp": scalar_feature("float32"),
        "frame_index": scalar_feature("int64"),
        "episode_index": scalar_feature("int64"),
        "index": scalar_feature("int64"),
        "task_index": scalar_feature("int64"),
    });
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
        "video_path": "videos/{video_key}/chunk-{chunk_index:03d}/file-{file_index:03d}.mp4",
        "features": features,
        "data_files_size_in_mb": 100,
        "video_files_size_in_mb": 200,
    });
    let stats = stats_document(
        dimension,
        &actions,
        &timestamps,
        &frame_indices,
        &episode_indices,
        &indices,
    );

    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
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
        b"---\ntags:\n- LeRobot\n- robotics\n---\n\n# rerobot dataset\n\nLeRobot v3 low-dimensional SO-ARM101 state/action data exported by rerobot studio. Camera video is not included because rerobot does not record camera streams yet.\n",
    )?;
    let bytes = archive
        .finish()
        .map_err(|error| invalid(error.to_string()))?
        .into_inner();

    Ok(DatasetArchive {
        bytes,
        episodes: episode_count as u64,
        frames: total_frames as u64,
        skipped,
    })
}

/// Import a zipped LeRobot v2/v3 dataset into the local episode workspace.
pub fn import_lerobot_archive(
    store: &EpisodeStore,
    bytes: &[u8],
) -> Result<ImportResponse, EpisodeError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| invalid(format!("the selected file is not a readable ZIP: {error}")))?;
    let names = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|file| file.name().to_owned())
        })
        .collect::<Vec<_>>();
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
        .filter(|fps| fps.is_finite() && *fps > 0.0)
        .ok_or_else(|| invalid("LeRobot metadata does not declare a usable fps"))?;
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
            let episode_index = batch.column_by_name("episode_index");
            let frame_index = batch.column_by_name("frame_index");
            let timestamp = batch.column_by_name("timestamp");
            for row in 0..batch.num_rows() {
                let action = vector_at(actions.as_ref(), row)?;
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
                let frame = frame_index
                    .map(|array| integer_at(array.as_ref(), row))
                    .transpose()?
                    .unwrap_or_else(|| {
                        grouped
                            .get(&episode)
                            .map_or(0, |frames| frames.len() as i64)
                    });
                let time = timestamp
                    .map(|array| number_at(array.as_ref(), row))
                    .transpose()?
                    .unwrap_or(frame as f64 / fps);
                grouped.entry(episode).or_default().push(ImportedFrame {
                    frame,
                    timestamp: time,
                    action,
                });
            }
        }
    }

    let dimension = dimension
        .filter(|dimension| *dimension > 0)
        .ok_or_else(|| invalid("the LeRobot dataset has no action samples to import"))?;
    let joints = normalized_joint_names(declared_names, dimension, robot_type)?;

    let mut first_episode = None;
    let mut imported_episodes = 0_u64;
    let mut imported_frames = 0_u64;
    for frames in grouped.values_mut() {
        frames.sort_by_key(|frame| frame.frame);
        if frames.is_empty() {
            continue;
        }
        let writer = store.begin(
            "Imported — LeRobot dataset".to_owned(),
            Some("Imported from a LeRobot ZIP".to_owned()),
            fps,
            vec![EpisodeArm {
                id: "follower".to_owned(),
                joints: joints.clone(),
            }],
        )?;
        let id = writer.id().to_owned();
        let mut last_time = 0.0_f64;
        for frame in frames.iter() {
            let normalized = frame
                .action
                .iter()
                .zip(&joints)
                .map(|(value, joint)| from_lerobot_position(*value, joint))
                .collect::<Result<Vec<_>, _>>()?;
            last_time = frame.timestamp.max(last_time);
            if !writer.push(Sample {
                seconds: frame.timestamp.max(0.0),
                normalized: vec![normalized.into_iter().map(Some).collect()],
                raw: vec![vec![0; dimension]],
                follow_error_degrees: None,
            }) {
                return Err(invalid(format!("could not write imported episode {id}")));
            }
        }
        let summary = writer.finish(last_time.max(frames.len() as f64 / fps), None, false)?;
        first_episode.get_or_insert(summary.id);
        imported_episodes += 1;
        imported_frames += summary.frames;
    }
    store.reload()?;

    Ok(ImportResponse {
        episodes: imported_episodes,
        frames: imported_frames,
        first_episode,
    })
}

fn read_export_episode(
    store: &EpisodeStore,
    summary: &rerobot_core::api::EpisodeSummary,
) -> Result<ExportEpisode, EpisodeError> {
    let path = store.directory().join(&summary.file);
    let file = File::open(&path).map_err(|source| EpisodeError::Io {
        path: path.clone(),
        source,
    })?;
    let mut lines = BufReader::new(file).lines();
    let header = lines
        .next()
        .transpose()
        .map_err(|source| EpisodeError::Io {
            path: path.clone(),
            source,
        })?
        .ok_or_else(|| invalid(format!("{} has no episode header", summary.id)))?;
    let header: Value = serde_json::from_str(&header)
        .map_err(|error| invalid(format!("could not read {}: {error}", summary.id)))?;
    let arms: Vec<EpisodeArm> = serde_json::from_value(
        header
            .get("arms")
            .cloned()
            .ok_or_else(|| invalid(format!("{} names no recorded arms", summary.id)))?,
    )
    .map_err(|error| invalid(format!("could not read {}: {error}", summary.id)))?;
    let arm_index = arms
        .iter()
        .position(|arm| arm.id.to_ascii_lowercase().contains("follower"))
        .or_else(|| (arms.len() == 1).then_some(0))
        .ok_or_else(|| invalid(format!("{} has no unambiguous follower arm", summary.id)))?;
    let joints = arms[arm_index].joints.clone();
    let mut positions = Vec::with_capacity(summary.frames as usize);
    for line in lines {
        let line = line.map_err(|source| EpisodeError::Io {
            path: path.clone(),
            source,
        })?;
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(values) = frame
            .get("n")
            .and_then(Value::as_array)
            .and_then(|arms| arms.get(arm_index))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let pose = values.iter().map(Value::as_f64).collect::<Option<Vec<_>>>();
        if let Some(pose) = pose.filter(|pose| pose.len() == joints.len()) {
            positions.push(pose.into_iter().map(|value| value as f32).collect());
        }
    }
    Ok(ExportEpisode {
        joints,
        positions,
        sample_hz: summary.sample_hz,
    })
}

fn data_parquet(
    dimension: usize,
    state: Vec<f32>,
    action: Vec<f32>,
    timestamp: Vec<f32>,
    frame_index: Vec<i64>,
    episode_index: Vec<i64>,
    index: Vec<i64>,
) -> Result<Vec<u8>, EpisodeError> {
    let item = Arc::new(Field::new("item", DataType::Float32, false));
    let state = Arc::new(
        FixedSizeListArray::try_new(
            item.clone(),
            dimension as i32,
            Arc::new(Float32Array::from(state)),
            None,
        )
        .map_err(|error| invalid(error.to_string()))?,
    ) as ArrayRef;
    let action = Arc::new(
        FixedSizeListArray::try_new(
            item.clone(),
            dimension as i32,
            Arc::new(Float32Array::from(action)),
            None,
        )
        .map_err(|error| invalid(error.to_string()))?,
    ) as ArrayRef;
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "observation.state",
            DataType::FixedSizeList(item.clone(), dimension as i32),
            false,
        ),
        Field::new(
            "action",
            DataType::FixedSizeList(item, dimension as i32),
            false,
        ),
        Field::new("timestamp", DataType::Float32, false),
        Field::new("frame_index", DataType::Int64, false),
        Field::new("episode_index", DataType::Int64, false),
        Field::new("index", DataType::Int64, false),
        Field::new("task_index", DataType::Int64, false),
    ]));
    let rows = timestamp.len();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            state,
            action,
            Arc::new(Float32Array::from(timestamp)),
            Arc::new(Int64Array::from(frame_index)),
            Arc::new(Int64Array::from(episode_index)),
            Arc::new(Int64Array::from(index)),
            Arc::new(Int64Array::from(vec![0; rows])),
        ],
    )
    .map_err(|error| invalid(error.to_string()))?;
    parquet_bytes(schema, batch)
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

fn scalar_feature(dtype: &str) -> Value {
    json!({ "dtype": dtype, "shape": [1], "names": Value::Null })
}

fn stats_document(
    dimension: usize,
    actions: &[f32],
    timestamps: &[f32],
    frames: &[i64],
    episodes: &[i64],
    indices: &[i64],
) -> Value {
    let vector = vector_stats(actions, dimension);
    json!({
        "observation.state": vector,
        "action": vector_stats(actions, dimension),
        "timestamp": scalar_stats(timestamps.iter().map(|value| *value as f64)),
        "frame_index": scalar_stats(frames.iter().map(|value| *value as f64)),
        "episode_index": scalar_stats(episodes.iter().map(|value| *value as f64)),
        "index": scalar_stats(indices.iter().map(|value| *value as f64)),
        "task_index": scalar_stats(std::iter::repeat_n(0.0, timestamps.len())),
    })
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

fn write_zip_entry(
    archive: &mut ZipWriter<Cursor<Vec<u8>>>,
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

fn read_zip_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, EpisodeError> {
    let mut file = archive
        .by_name(name)
        .map_err(|error| invalid(format!("could not read {name}: {error}")))?;
    if file.is_dir() {
        return Err(invalid(format!("{name} is a directory, not a file")));
    }
    let mut bytes = Vec::with_capacity(file.size().min(64 * 1024 * 1024) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("could not read {name}: {error}")))?;
    Ok(bytes)
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
        let writer = store
            .begin(
                "Teleoperation".to_owned(),
                None,
                30.0,
                vec![EpisodeArm {
                    id: "follower".to_owned(),
                    joints: SO101_JOINTS
                        .iter()
                        .map(|joint| (*joint).to_owned())
                        .collect(),
                }],
            )
            .unwrap();
        for frame in 0..4 {
            writer.push(Sample {
                seconds: frame as f64 / 30.0,
                normalized: vec![vec![Some(frame as f64 / 10.0); SO101_JOINTS.len()]],
                raw: vec![vec![0; SO101_JOINTS.len()]],
                follow_error_degrees: None,
            });
        }
        writer.finish(0.1, None, false).unwrap();
        store.reload().unwrap();
        (directory, store)
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
        assert!(zip.by_name("data/chunk-000/file-000.parquet").is_ok());

        let target_directory = TempDir::new().unwrap();
        let target = EpisodeStore::open(target_directory.path().join("episodes")).unwrap();
        let imported = import_lerobot_archive(&target, &archive.bytes).unwrap();
        assert_eq!(imported.episodes, 1);
        assert_eq!(imported.frames, 4);
        let trajectory = target
            .trajectory(imported.first_episode.as_deref().unwrap(), "follower")
            .unwrap();
        assert_eq!(trajectory[0].len(), SO101_JOINTS.len());
        assert!(
            SO101_JOINTS
                .iter()
                .all(|joint| trajectory[0].contains_key(*joint))
        );
        assert!(trajectory[0].keys().all(|joint| !joint.ends_with(".pos")));
        assert!((trajectory[3]["shoulder_pan"] - 0.3).abs() < 0.001);
        assert!((trajectory[3]["gripper"] - 0.3).abs() < 0.001);
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
