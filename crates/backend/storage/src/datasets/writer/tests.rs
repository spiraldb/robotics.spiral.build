use super::*;
use crate::datasets::fixtures::RolloutFixture;
use crate::db::{ArrowColumns, UnquotedMemory};
use std::sync::atomic::Ordering;

/// Rollout recording `episode`: twenty joint samples of one arm, with a
/// camera's eight frames when `filmed`.
fn decoded(episode: &str, filmed: bool) -> DecodedEpisode {
    let bytes = RolloutFixture::mcap(episode, filmed);
    DecodedEpisode::of_mcap(
        McapFile::held(&format!("{episode}.mcap"), &bytes).unwrap(),
        &bytes,
    )
    .unwrap()
}

/// How many rows `sql`'s first column counts.
async fn count(store: &ProjectStore, sql: String) -> u64 {
    let batches = store
        .connect()
        .await
        .unwrap()
        .read_batches(&sql)
        .await
        .unwrap();
    ArrowColumns::new(&batches[0], "count")
        .optional_unsigned("rows", 0)
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn an_import_commits_32_episodes_a_transaction() {
    let (store, commits) = UnquotedMemory::store().await;
    let dataset = DatasetSelection::new("demo").unwrap();
    let mut import = DatasetImport::new(&store, &dataset, DatasetFormat::Mcap);
    let episodes = (0..70)
        .map(|index| decoded(&format!("take-{index}"), index % 2 == 0))
        .collect::<Vec<_>>();
    let mut episodes = episodes.into_iter();
    for batch in [32, 32, 6] {
        import
            .commit(episodes.by_ref().take(batch).collect())
            .await
            .unwrap();
    }
    assert_eq!(commits.load(Ordering::SeqCst), 3);
    for table in [dataset.episodes(), dataset.answers(), dataset.files()] {
        assert_eq!(
            count(&store, format!("SELECT count(*) AS rows FROM {table}")).await,
            70,
            "{}",
            table.target()
        );
    }
    let mut connection = store.connect().await.unwrap();
    let topics = TopicCounts::read(&mut connection, &dataset).await.unwrap();
    drop(connection);
    let topics = topics.topics();
    let camera = topics
        .iter()
        .find(|topic| topic.name() == "/camera/top/video")
        .unwrap();
    assert_eq!((camera.members(), camera.samples()), (35, 35 * 8));
    assert!(camera.is_video());
    let joints = topics
        .iter()
        .find(|topic| topic.name() == "/studio/joints/leader")
        .unwrap();
    assert_eq!((joints.members(), joints.samples()), (70, 70 * 20));
    for topic in &topics {
        let messages = count(
            &store,
            format!(
                "SELECT count(*) AS rows FROM {}",
                dataset.messages(topic.revision())
            ),
        )
        .await;
        assert_eq!(messages, topic.samples(), "{}", topic.name());
    }
    // Each camera row says whether its access unit is a keyframe.
    let mut units = spiral_robotics_recording::AccessUnits::default();
    let mut frames = units.push(RolloutFixture::CAMERA);
    frames.extend(units.finish());
    let keyframes = frames
        .iter()
        .filter(|frame| spiral_robotics_common::datasets::H264AccessUnit::holds_idr(frame))
        .count() as u64;
    assert!(keyframes > 0);
    assert_eq!(
        count(
            &store,
            format!(
                "SELECT count(*) AS rows FROM {} WHERE keyframe",
                dataset.messages(camera.revision())
            )
        )
        .await,
        35 * keyframes
    );
}

#[tokio::test]
async fn an_episode_rows_hold_its_facts_and_streams() {
    let (store, _) = UnquotedMemory::store().await;
    let dataset = DatasetSelection::new("demo").unwrap();
    let mut import = DatasetImport::new(&store, &dataset, DatasetFormat::Mcap);
    import
        .commit(vec![decoded("take-1", true), decoded("take-2", false)])
        .await
        .unwrap();
    let batches = store
        .connect()
        .await
        .unwrap()
        .read_batches(&format!(
            "SELECT id, key, source_uri, sha256, message_count, channel_count, message_start_time, \
             duration_ns, spql.list.len(streams) AS stream_count FROM {} ORDER BY key",
            dataset.episodes()
        ))
        .await
        .unwrap();
    let batch = arrow_select::concat::concat_batches(&batches[0].schema(), &batches).unwrap();
    let columns = ArrowColumns::new(&batch, "episodes");
    let ids = super::super::EpisodeUuids::read(batch.column(0).as_ref()).unwrap();
    assert!(
        ids.iter().all(|id| id.as_str().as_bytes()[12] == b'7'),
        "every new episode is a UUIDv7"
    );
    assert_ne!(ids[0], ids[1]);
    assert_eq!(columns.text("key", 0).unwrap(), "take-1.mcap");
    assert_eq!(columns.optional_text("source_uri", 0).unwrap(), None);
    assert_eq!(columns.text("sha256", 0).unwrap().len(), 64);
    let channels = columns
        .optional_unsigned("channel_count", 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        columns.optional_unsigned("stream_count", 0).unwrap(),
        Some(channels)
    );
    assert_eq!(
        columns.optional_unsigned("channel_count", 1).unwrap(),
        Some(channels - 2),
        "the unfilmed take holds no camera stream, nor the camera's metadata"
    );
    assert_eq!(
        columns.optional_unsigned("message_start_time", 0).unwrap(),
        Some(1_700_000_000_000_000_000)
    );
    assert!(
        columns
            .optional_unsigned("duration_ns", 0)
            .unwrap()
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn a_write_takes_at_most_32_episodes_of_its_format() {
    let (store, _) = UnquotedMemory::store().await;
    let dataset = DatasetSelection::new("demo").unwrap();
    let mut connection = store.connect_exclusive().await.unwrap();
    let mut write = DatasetWrite::create(&mut connection, &dataset, DatasetFormat::LeRobot)
        .await
        .unwrap();
    assert!(
        write
            .write(&mut connection, vec![decoded("take", false)])
            .await
            .unwrap_err()
            .contains("another format")
    );
    let many = (0..33)
        .map(|index| decoded(&format!("take-{index}"), false))
        .collect();
    assert!(write.write(&mut connection, many).await.is_err());
}
