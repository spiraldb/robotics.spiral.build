use super::*;
use crate::datasets::ArrowTypeText;
use arrow_array::builder::{Float32Builder, ListBuilder};
use arrow_array::{Float32Array, Int64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use serde_json::json;

fn parquet(columns: Vec<(&str, ArrayRef)>) -> Bytes {
    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .map(|(name, column)| Field::new(*name, column.data_type().clone(), true))
            .collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        columns.into_iter().map(|(_, column)| column).collect(),
    )
    .unwrap();
    let mut bytes = Vec::new();
    let mut writer = parquet::arrow::ArrowWriter::try_new(&mut bytes, schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    Bytes::from(bytes)
}

fn info() -> LeRobotInfo {
    LeRobotInfo::parse(
        json!({
            "codebase_version": "v3.0",
            "fps": 30,
            "data_path": "data/chunk-{chunk_index:03d}/file-{file_index:05d}.parquet",
            "video_path": "videos/{video_key}/chunk-{chunk_index:03d}/file-{file_index:03d}.mp4",
            "features": {
                "action": {"dtype": "float32", "shape": [2], "names": ["shoulder_pan.pos", "gripper.pos"]},
                "state": {"dtype": "float32", "shape": [2], "names": ["state"]},
                "observation.images.up": {"dtype": "video", "shape": [480, 640, 3], "names": ["height", "width", "channels"]},
                "timestamp": {"dtype": "float32", "shape": [1], "names": null},
                "frame_index": {"dtype": "int64", "shape": [1], "names": null}
            }
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap()
}

#[test]
fn info_names_the_features_and_where_each_file_lies() {
    let info = info();
    assert_eq!(
        info.features
            .iter()
            .map(|feature| (feature.name(), feature.is_camera()))
            .collect::<Vec<_>>(),
        [
            ("action", false),
            ("observation.images.up", true),
            ("state", false)
        ]
    );
    let place = FilePlace { chunk: 0, file: 7 };
    assert_eq!(
        info.data_file(place).unwrap(),
        "data/chunk-000/file-00007.parquet"
    );
    assert_eq!(
        info.video_file("observation.images.up", place).unwrap(),
        "videos/observation.images.up/chunk-000/file-007.mp4"
    );
    let older = json!({"codebase_version": "v2.1", "fps": 30, "data_path": "", "features": {}});
    assert!(
        LeRobotInfo::parse(older.to_string().as_bytes())
            .err()
            .unwrap()
            .contains("`v2.1` is unsupported")
    );
    assert!(PathTemplate("{episode_index}").fill(&[], place).is_err());
}

#[test]
fn tasks_are_read_from_their_column_or_the_pandas_index() {
    for text in ["task", "__index_level_0__"] {
        let tasks = LeRobotRepository::tasks(parquet(vec![
            ("task_index", Arc::new(Int64Array::from(vec![0, 1]))),
            (text, Arc::new(StringArray::from(vec!["pick", "place"]))),
        ]))
        .unwrap();
        assert_eq!(tasks[&1], "place");
    }
}

#[test]
fn the_episodes_table_places_each_episode_s_frames_and_videos() {
    let table = ParquetTable::read(
        "meta/episodes/chunk-000/file-000.parquet",
        parquet(vec![
            ("episode_index", Arc::new(Int64Array::from(vec![0, 1]))),
            ("length", Arc::new(Int64Array::from(vec![303, 266]))),
            ("data/chunk_index", Arc::new(Int64Array::from(vec![0, 0]))),
            ("data/file_index", Arc::new(Int64Array::from(vec![0, 0]))),
            (
                "videos/observation.images.up/chunk_index",
                Arc::new(Int64Array::from(vec![0, 0])),
            ),
            (
                "videos/observation.images.up/file_index",
                Arc::new(Int64Array::from(vec![0, 1])),
            ),
            (
                "videos/observation.images.up/from_timestamp",
                Arc::new(arrow_array::Float64Array::from(vec![0.0, 10.1])),
            ),
        ]),
        None,
    )
    .unwrap();
    let entries = LeRobotRepository::entries(&table, &info()).unwrap();
    assert_eq!(entries[1].index, 1);
    assert_eq!(entries[1].frames, 266);
    assert_eq!(
        entries[1].videos["observation.images.up"],
        (FilePlace { chunk: 0, file: 1 }, 10.1)
    );
}

/// A frame table as LeRobot writes it: every vector feature a list of floats.
#[test]
fn a_frame_table_s_features_read_as_their_stream_values() {
    let mut action = ListBuilder::new(Float32Builder::new());
    for frame in [[1.0, 2.0], [3.0, 4.0]] {
        action.values().append_slice(&frame);
        action.append(true);
    }
    let mut state = ListBuilder::new(Float32Builder::new());
    for frame in [[5.0, 6.0], [7.0, 8.0]] {
        state.values().append_slice(&frame);
        state.append(true);
    }
    let table = ParquetTable::read(
        "data/chunk-000/file-000.parquet",
        parquet(vec![
            ("action", Arc::new(action.finish())),
            ("state", Arc::new(state.finish())),
            (
                "timestamp",
                Arc::new(Float32Array::from(vec![0.0, 1.0 / 30.0])),
            ),
        ]),
        Some(&["action", "state"]),
    )
    .unwrap();
    assert!(
        table.column("timestamp").is_err(),
        "only the columns asked for are read"
    );
    let info = info();
    let types = info
        .features
        .iter()
        .filter(|feature| !feature.is_camera())
        .map(|feature| {
            let values = feature
                .values(table.column(feature.name()).unwrap())
                .unwrap();
            ArrowTypeText::try_from(values.data_type())
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        types,
        [
            "struct<shoulder_pan.pos: float not null, gripper.pos: float not null>",
            "struct<data: fixed_size_list<item: float not null>[2] not null>",
        ]
    );
    let camera = LeRobotRepository::camera_values(&[vec![0, 0, 1, 0x65]]).unwrap();
    assert_eq!(
        ArrowTypeText::try_from(camera.data_type())
            .unwrap()
            .to_string(),
        "struct<data: binary_view not null, format: string_view not null>"
    );
    assert_eq!(camera.data_type(), &LeRobotFeature::camera_type());
    assert!(matches!(camera.data_type(), DataType::Struct(_)));
}

/// What re-encoding real episodes costs: two or three of
/// sim-bottles-mjwarp-v1's (H.264, a keyframe every 10 frames, three
/// cameras) and one of svla_so101_pickplace's (AV1, two cameras), each
/// timed whole.
#[tokio::test]
#[ignore = "downloads public Hugging Face files and runs FFmpeg; prints each episode's time"]
async fn real_episodes_encode_one_access_unit_per_frame() {
    use spiral_robotics_common::datasets::HuggingFaceDatasetSource;
    for (repository, commit, positions) in [
        (
            "uynitsuj/sim-bottles-mjwarp-v1",
            "897b86679272a47897cc5ecca78c2205586a29d5",
            &[0, 150, 301][..],
        ),
        (
            "lerobot/svla_so101_pickplace",
            "f641879e22172be7e8161d5e6c1503c2d2feb657",
            &[1][..],
        ),
    ] {
        let inspection = super::super::HuggingFaceFileSelection::LeRobot
            .inspect(HuggingFaceDatasetSource::new("probe", repository).with_revision(commit))
            .await
            .unwrap();
        let downloads = HuggingFaceDownloads::new(
            super::super::HuggingFaceRepository::try_from(repository).unwrap(),
            commit,
        )
        .unwrap();
        let mut episodes = LeRobotRepository::open(&downloads, inspection.files())
            .await
            .unwrap();
        // The frame table is read once, before the timed episodes.
        episodes.frames_of(positions[0]).await.unwrap();
        for &position in positions {
            let started = std::time::Instant::now();
            let episode = episodes.episode(position).await.unwrap();
            let frames = episodes.episodes[position].frames;
            let cameras = episode
                .streams()
                .iter()
                .filter(|messages| messages.stream().is_video())
                .inspect(|messages| assert_eq!(messages.len(), frames))
                .count();
            println!(
                "{repository} episode {position}: {frames} frames x {cameras} cameras in {:.2} s",
                started.elapsed().as_secs_f64()
            );
        }
    }
}

/// One frame's luma, as FFmpeg decodes it.
struct LumaFrame(Vec<u8>);

impl LumaFrame {
    /// How close `other` is, in dB: higher is closer.
    fn psnr(&self, other: &Self) -> f64 {
        assert_eq!(self.0.len(), other.0.len());
        let squared = self
            .0
            .iter()
            .zip(&other.0)
            .map(|(left, right)| (f64::from(*left) - f64::from(*right)).powi(2))
            .sum::<f64>();
        let mean = squared / self.0.len() as f64;
        if mean == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0_f64.powi(2) / mean).log10()
        }
    }
}

/// Frames FFmpeg decodes to compare a cut with the video it was cut from.
struct FrameReader {
    executable: String,
}

impl FrameReader {
    async fn luma(&self, arguments: Vec<String>) -> Vec<u8> {
        let output = tokio::process::Command::new(&self.executable)
            .args(["-v", "error", "-nostdin"])
            .args(arguments)
            .args([
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "pipe:1",
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// The source's frame `index`, the one presented at `index / fps`, read
    /// by its own timestamp: FFmpeg keeps the file's timestamps and seeks to
    /// two seconds before it. None before the first frame or past the last.
    async fn source(&self, video: &std::path::Path, index: i64, fps: f64) -> Option<LumaFrame> {
        if index < 0 {
            return None;
        }
        let at = index as f64 / fps;
        let mut arguments = Vec::new();
        arguments.extend([
            "-copyts".to_owned(),
            "-ss".to_owned(),
            format!("{:.6}", (at - 2.0).max(0.0)),
            "-i".to_owned(),
            video.display().to_string(),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-vf".to_owned(),
            format!(
                "select='between(t\\,{:.6}\\,{:.6})'",
                at - 0.5 / fps,
                at + 0.5 / fps
            ),
            "-frames:v".to_owned(),
            "1".to_owned(),
        ]);
        // A frame past the video's end decodes as nothing.
        Some(LumaFrame(self.luma(arguments).await)).filter(|frame| !frame.0.is_empty())
    }

    /// The first and last frames of the cut `units`.
    async fn cut(&self, units: &[Vec<u8>]) -> (LumaFrame, LumaFrame) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cut.h264");
        std::fs::write(&path, units.concat()).unwrap();
        let last = units.len() - 1;
        let frames = self
            .luma(vec![
                "-i".to_owned(),
                path.display().to_string(),
                "-vf".to_owned(),
                format!("select='eq(n\\,0)+eq(n\\,{last})'"),
            ])
            .await;
        let (first, last) = frames.split_at(frames.len() / 2);
        (LumaFrame(first.to_vec()), LumaFrame(last.to_vec()))
    }
}

/// Every camera of a few episodes of each repository starts at the source
/// frame of its first frame row, `from_timestamp × fps`, and ends at the
/// frame of its last row: each end of the cut is closer to that source frame
/// than to its neighbours, the frame across the episode's boundary and the
/// one beside it within the episode, unless that neighbour is a copy of it.
/// svla_so101_pickplace's camera repeats frames, which no comparison of
/// pixels tells apart; the frames across each boundary always differ.
#[tokio::test]
#[ignore = "downloads public Hugging Face files and runs FFmpeg; prints each comparison"]
async fn cut_episodes_start_and_end_at_their_own_frames() {
    use spiral_robotics_common::datasets::HuggingFaceDatasetSource;
    for (repository, commit, positions) in [
        (
            "uynitsuj/sim-bottles-mjwarp-v1",
            "897b86679272a47897cc5ecca78c2205586a29d5",
            &[0, 150, 301, 1000][..],
        ),
        (
            "lerobot/svla_so101_pickplace",
            "f641879e22172be7e8161d5e6c1503c2d2feb657",
            &[1, 25, 49][..],
        ),
    ] {
        let inspection = super::super::HuggingFaceFileSelection::LeRobot
            .inspect(HuggingFaceDatasetSource::new("probe", repository).with_revision(commit))
            .await
            .unwrap();
        let downloads = HuggingFaceDownloads::new(
            super::super::HuggingFaceRepository::try_from(repository).unwrap(),
            commit,
        )
        .unwrap();
        let mut episodes = LeRobotRepository::open(&downloads, inspection.files())
            .await
            .unwrap();
        let reader = FrameReader {
            executable: std::env::var("SPIRAL_ROBOTICS_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
        };
        let fps = episodes.info.fps;
        for &position in positions {
            episodes.hold_videos(position).await.unwrap();
            let entry = &episodes.episodes[position];
            for (camera, (place, from_seconds)) in &entry.videos {
                let video = episodes.videos.path(camera, *place).unwrap();
                let units = episodes.camera(position, camera).await.unwrap();
                let (first, last) = reader.cut(&units).await;
                let start = (from_seconds * fps).round() as i64;
                let end = start + entry.frames as i64 - 1;
                for (end_name, frame, index) in [("first", &first, start), ("last", &last, end)] {
                    let own_source = reader.source(video, index, fps).await.unwrap();
                    let own = frame.psnr(&own_source);
                    let mut report = vec![format!("{index}: {own:.1} dB")];
                    assert!(
                        own > 30.0,
                        "{end_name} frame is no copy of source frame {index}"
                    );
                    for neighbour in [index - 1, index + 1] {
                        let Some(source) = reader.source(video, neighbour, fps).await else {
                            continue;
                        };
                        let closeness = frame.psnr(&source);
                        let copy = own_source.psnr(&source);
                        report.push(format!(
                            "{neighbour}: {closeness:.1} dB (itself {copy:.1} dB from {index})"
                        ));
                        assert!(
                            closeness <= own || copy >= 50.0,
                            "{end_name} frame is closer to source frame {neighbour} than to {index}"
                        );
                    }
                    println!(
                        "{repository} episode {} {camera} {end_name} frame vs source frames {}",
                        entry.index,
                        report.join(", ")
                    );
                }
            }
        }
    }
}
