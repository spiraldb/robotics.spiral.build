use super::*;
use crate::StudioStore;
use crate::datasets::TestJournal;
use crate::datasets::fixtures::RolloutFixture;
use crate::db::{ArrowColumns, MemoryDatabase};
use spiral_robotics_common::datasets::{
    DatasetTopic, EpisodeId, EpisodeSelection, EpisodeTopicRequest, MessageCursor, MessageRead,
    MessageWindow,
};
use std::sync::Arc;
#[test]
fn the_same_file_again_is_a_retry_and_another_is_refused() {
    let stored = |sha256: &[Option<&str>]| StoredFile {
        sha256: sha256.iter().map(|sha| sha.map(str::to_owned)).collect(),
    };
    assert!(!stored(&[]).holds("abc").unwrap());
    assert!(stored(&[Some("abc")]).holds("abc").unwrap());
    assert!(stored(&[Some("def")]).holds("abc").is_err());
    assert!(stored(&[None]).holds("abc").is_err());
    assert!(stored(&[Some("abc"), Some("abc")]).holds("abc").is_err());
}

/// Episodes are stored as the robot connector sends them, and the journal
/// names a dataset's tables once its first episode has made them.
struct RecordingProject {
    journal: TestJournal,
    store: Arc<crate::ProjectStore>,
    storage: StorageManager,
}

impl RecordingProject {
    async fn new() -> Self {
        let journal = TestJournal::start().await;
        let (store, _) = MemoryDatabase::store_with_catalog(journal.endpoint()).await;
        let store = Arc::new(store);
        let storage = StorageManager::new(
            store.clone(),
            StudioStore::in_memory().project(&store.connection_settings()),
        );
        Self {
            journal,
            store,
            storage,
        }
    }

    async fn store(&self, episode: &str, bytes: &[u8]) -> Result<(), String> {
        let stored = self
            .storage
            .store_recording(&EpisodeName::new("demo", episode).unwrap(), bytes)
            .await;
        let dataset = DatasetSelection::new("demo").unwrap();
        self.journal
            .name(&[dataset.episodes(), dataset.topics(), dataset.files()]);
        stored
    }

    async fn episodes(&self) -> u64 {
        let batches = self
            .store
            .connect()
            .await
            .unwrap()
            .read_batches(&format!(
                "SELECT count(*) AS episodes FROM {}",
                DatasetSelection::new("demo").unwrap().episodes()
            ))
            .await
            .unwrap();
        ArrowColumns::new(&batches[0], "count")
            .optional_unsigned("episodes", 0)
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn a_recording_stored_again_is_kept_once_and_other_bytes_are_refused() {
    let project = RecordingProject::new().await;
    let take = RolloutFixture::mcap("take-1", true);
    project.store("take-1", &take).await.unwrap();
    assert_eq!(project.episodes().await, 1);
    project.store("take-1", &take).await.unwrap();
    assert_eq!(project.episodes().await, 1, "a retry stores nothing new");
    let other = RolloutFixture::mcap("take-1", false);
    assert!(
        project
            .store("take-1", &other)
            .await
            .unwrap_err()
            .contains("different bytes")
    );
    project.store("take-2", &other).await.unwrap();
    assert_eq!(project.episodes().await, 2);
    let dataset = DatasetSelection::new("demo").unwrap();
    let mut connection = project.store.connect().await.unwrap();
    let topics = crate::datasets::topics::TopicCounts::read(&mut connection, &dataset)
        .await
        .unwrap()
        .topics();
    drop(connection);
    let joints = topics
        .iter()
        .find(|topic| topic.name() == "/studio/joints/leader")
        .unwrap();
    assert_eq!((joints.members(), joints.samples()), (2, 40));
    let camera = topics.iter().find(|topic| topic.is_video()).unwrap();
    assert_eq!((camera.members(), camera.samples()), (1, 8));
}

#[tokio::test]
async fn a_recording_joins_only_an_mcap_dataset() {
    let project = RecordingProject::new().await;
    let lerobot = DatasetSelection::new("svla").unwrap();
    project
        .journal
        .name(&[lerobot.episodes(), lerobot.topics()]);
    let error = project
        .storage
        .store_recording(
            &EpisodeName::new("svla", "take-1").unwrap(),
            &RolloutFixture::mcap("take-1", false),
        )
        .await
        .unwrap_err();
    assert!(error.contains("LeRobot dataset"), "{error}");
}

#[tokio::test]
#[ignore = "requires SPIRAL_MCAP_TEST_PROJECT and SPIRAL_ACCESS_TOKEN (endpoints from SPIRAL__SERVER__URL and SPIRAL__SPFS__URL); creates and drops uniquely named datasets"]
async fn stored_recordings_read_back_by_episode_and_stream() {
    use spiral_robotics_recording::{CameraRecording, EpisodeFixture, McapRecording, Sample};
    let (store, storage) = crate::datasets::HostedProject::storage();
    let name = format!("source_probe_{}", uuid::Uuid::new_v4().simple());
    let other_name = format!("{name}_other");
    let dataset = DatasetSelection::new(name.clone()).unwrap();
    let cameras = ["wrist", "overhead"]
        .into_iter()
        .map(|stream| {
            CameraRecording::from_annex_b(
                stream.into(),
                (32, 32, 30),
                (
                    RolloutFixture::CAMERA.to_vec(),
                    (0..8)
                        .map(|index| {
                            if stream == "overhead" {
                                25_000_000 + index * 25_000_000
                            } else {
                                index * 33_333_333
                            }
                        })
                        .collect(),
                ),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let outcome: Result<(), String> = async {
        for (episode, camera_count) in [("first", 2), ("second", 1), ("third", 0)] {
            let samples = (0..120)
                .map(|index| {
                    let state = if index == 60 {
                        None
                    } else {
                        Some(index as f64 / 120.0)
                    };
                    let sample =
                        Sample::new(index as f64 * 0.002, vec![vec![state]], vec![vec![index]]);
                    if camera_count > 0 {
                        sample.with_action(vec![Some(vec![Some(0.5)])])
                    } else {
                        sample
                    }
                })
                .collect();
            let recording = EpisodeFixture::new(episode, &name)
                .with_arms(vec![spiral_robotics_common::recording::EpisodeArm::new(
                    "arm",
                    None,
                    vec!["joint".into()],
                )])
                .with_timing(1_700_000_000_000, 500.0)
                .with_samples(samples)
                .with_duration_seconds(0.3)
                .into_episode();
            let bytes = McapRecording::encode(&recording, &cameras[..camera_count])?;
            let episode = EpisodeName::new(&name, episode)?;
            storage.store_recording(&episode, &bytes).await?;
            storage.store_recording(&episode, &bytes).await?;
        }
        let rows = store
            .connect()
            .await?
            .query_rows(&format!("SELECT id, key FROM {}", dataset.episodes()), 10)
            .await?;
        assert_eq!(rows.rows().len(), 3, "a retry stores nothing new");
        for row in rows.rows() {
            let member = EpisodeId::new(row["id"].as_u64().unwrap());
            let key = row["key"].as_str().unwrap();
            let available = storage
                .episode_topics(EpisodeSelection::new(dataset.clone(), member.clone()))
                .await?;
            let expected = match key {
                "first.mcap" => vec!["/camera/overhead/video", "/camera/wrist/video"],
                "second.mcap" => vec!["/camera/wrist/video"],
                _ => vec![],
            };
            assert_eq!(
                available
                    .iter()
                    .filter(|topic| topic.is_video())
                    .map(DatasetTopic::name)
                    .collect::<Vec<_>>(),
                expected,
                "{key}"
            );
            assert!(available.iter().all(|topic| topic.members() == 1));
            let topic = available
                .iter()
                .find(|topic| topic.name() == "/studio/joints/arm")
                .unwrap();
            let read = MessageRead::schema(
                EpisodeTopicRequest::new(dataset.clone(), topic.name().into(), member.clone()),
                topic.revision().clone(),
            );
            let window = MessageWindow::new(
                (1_700_000_000_000_000_000, 1_700_000_000_300_000_000),
                vec![],
            )
            .with_limit(64);
            let mut next = None;
            let mut all = Vec::<serde_json::Value>::new();
            loop {
                let page = MessagePage::read(
                    &storage,
                    &read.clone().with_window(window.clone().with_cursor(next)),
                )
                .await?;
                next = page.next;
                all.extend(page.rows);
                if next.is_none() {
                    break;
                }
            }
            assert_eq!(all.len(), 120);
            assert_eq!(
                all[100]["log_time"].as_u64(),
                Some(1_700_000_000_200_000_000)
            );
            assert_eq!(
                all[100]["value"]["state_position"]["values"][0]["value"].as_f64(),
                Some(100.0 / 120.0)
            );
            assert!(all[60]["value"]["state_position"]["values"][0]["value"].is_null());
            assert_eq!(
                all[0]["value"]["action_valid"].as_bool(),
                Some(key != "third.mcap")
            );
        }
        // An episode the dataset does not hold holds no topics.
        let stranger = EpisodeId::new(u64::MAX);
        assert!(
            storage
                .episode_topics(EpisodeSelection::new(dataset.clone(), stranger))
                .await?
                .is_empty()
        );
        // The same episode in another dataset is independent.
        let bytes = RolloutFixture::mcap("first", false);
        storage
            .store_recording(&EpisodeName::new(&other_name, "first")?, &bytes)
            .await?;
        let inspection = storage
            .inspect_dataset(DatasetSelection::new(other_name.clone())?)
            .await?;
        assert_eq!(inspection.members(), 1);
        Ok(())
    }
    .await;
    for probe in [&name, &other_name] {
        storage.drop_probe_dataset(probe).await.unwrap();
    }
    outcome.expect("stored recordings");
}

/// One page of a message read, fetched the way the browser pages: one Arrow
/// episode query, continuing after the last kept row.
struct MessagePage {
    rows: Vec<serde_json::Value>,
    next: Option<MessageCursor>,
}

impl MessagePage {
    async fn read(storage: &StorageManager, read: &MessageRead) -> Result<Self, String> {
        let request = spiral_robotics_common::relations::EpisodeQueryRequest::new(
            read.episode(),
            read.sql()?,
            read.max_rows(),
        );
        let bytes = storage.queries().arrow_for_episode(&request).await?;
        let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
            .map_err(|error| error.to_string())?;
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut rows = Vec::<serde_json::Value>::new();
        if !batches.is_empty() {
            let mut json = Vec::new();
            let mut writer = arrow_json::writer::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow_json::writer::JsonArray>(&mut json);
            writer
                .write_batches(&batches.iter().collect::<Vec<_>>())
                .map_err(|error| error.to_string())?;
            writer.finish().map_err(|error| error.to_string())?;
            drop(writer);
            rows = serde_json::from_slice(&json).map_err(|error| error.to_string())?;
        }
        let limit = read.window().map_or(0, |window| window.limit());
        let next = if rows.len() > limit {
            let last = &rows[limit - 1];
            let position =
                |name: &str| last[name].as_u64().ok_or_else(|| format!("Missing {name}"));
            Some(MessageCursor::new(
                position("log_time")?,
                position("ordinal")?,
            ))
        } else {
            None
        };
        rows.truncate(limit);
        Ok(Self { rows, next })
    }
}
