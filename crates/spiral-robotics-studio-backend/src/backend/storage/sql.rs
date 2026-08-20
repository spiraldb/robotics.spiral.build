//! Backend-only SQL construction and policy adaptation for SpiralDB.
//!
//! Owned relation and column names come from `spiral_robotics_contracts::spiral`. Keeping
//! string literals and dynamic-resource naming here makes every generated
//! statement auditable. Raw user queries delegate to the framework-free core
//! authorization policy before a database driver sees them.

use spiral_robotics_contracts::spiral::{self, Table, column};
pub(crate) use spiral_robotics_core::query::ReadOnlyMode;

pub(crate) fn relation(dataset: &str, table: Table) -> String {
    spiral::relation(dataset, table)
}

#[cfg(test)]
pub(crate) fn count_rows(relation: &str) -> String {
    format!("SELECT COUNT(*) AS rows FROM {relation}")
}

pub(crate) fn dataset_exists(relation: &str, dataset_id: &str) -> String {
    format!(
        "SELECT {dataset_id_column} FROM {relation} WHERE {dataset_id_column} = {dataset_id} LIMIT 1",
        dataset_id_column = column::DATASET_ID,
        dataset_id = string_literal(dataset_id),
    )
}

pub(crate) fn delete_episode(relation: &str, episode_id: &str) -> String {
    format!(
        "DELETE FROM {relation} WHERE {} = {}",
        column::EPISODE_ID,
        string_literal(episode_id)
    )
}

pub(crate) fn delete_training_run(relation: &str, run_id: &str) -> String {
    format!(
        "DELETE FROM {relation} WHERE {} = {}",
        column::RUN_ID,
        string_literal(run_id)
    )
}

pub(crate) fn trajectory(dataset: &str, episode_id: &str, stream_id: &str) -> String {
    let episodes = relation(dataset, Table::Episodes);
    let streams = relation(dataset, Table::EpisodeStreams);
    let samples = relation(dataset, Table::JointSamples);
    format!(
        "SELECT e.{sample_hz}, s.{joint_names}, \
         COALESCE(j.{action}, j.{state}) AS {replay} \
         FROM {episodes} e JOIN {streams} s ON e.{episode_id} = s.{episode_id} \
         JOIN {samples} j ON j.{episode_id} = s.{episode_id} AND j.{stream_id} = s.{stream_id} \
         WHERE e.{episode_id} = {episode_value} AND s.{stream_id} = {stream_value} \
         ORDER BY j.{sample_index}",
        sample_hz = column::SAMPLE_HZ,
        joint_names = column::JOINT_NAMES,
        action = column::ACTION_POSITION,
        state = column::STATE_POSITION,
        replay = column::REPLAY_POSITION,
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        sample_index = column::SAMPLE_INDEX,
        episode_value = string_literal(episode_id),
        stream_value = string_literal(stream_id),
    )
}

pub(crate) fn episode_samples(dataset: &str, episode_id: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = {} ORDER BY {}, {}",
        relation(dataset, Table::JointSamples),
        column::EPISODE_ID,
        string_literal(episode_id),
        column::SAMPLE_INDEX,
        column::STREAM_ID,
    )
}

pub(crate) fn video_asset_size(
    dataset: &str,
    episode_id: &str,
    stream_id: &str,
    segment: Option<u32>,
) -> String {
    let assets = relation(dataset, Table::VideoAssets);
    let segment_filter = segment
        .map(|segment| format!(" AND {} = {segment}", column::SEGMENT_INDEX))
        .unwrap_or_default();
    format!(
        "SELECT {byte_size} FROM {assets} WHERE {episode_id_column} = {episode_id} \
         AND {stream_id_column} = {stream_id}{segment_filter} \
         ORDER BY {segment_index} LIMIT 1",
        byte_size = column::BYTE_SIZE,
        episode_id_column = column::EPISODE_ID,
        episode_id = string_literal(episode_id),
        stream_id_column = column::STREAM_ID,
        stream_id = string_literal(stream_id),
        segment_index = column::SEGMENT_INDEX,
    )
}

pub(crate) fn video_asset_range(
    dataset: &str,
    episode_id: &str,
    stream_id: &str,
    segment: Option<u32>,
    offset: u64,
    size: u64,
) -> String {
    let assets = video_dataset_output(dataset);
    let segment_filter = segment
        .map(|segment| format!(" AND {} = {segment}", column::SEGMENT_INDEX))
        .unwrap_or_default();
    format!(
        "SELECT spql.io.read_bytes({source}, CAST({offset} AS UINT64), \
         CAST({size} AS UINT64)) AS {payload} \
         FROM {assets} WHERE {episode_id_column} = {episode_id} \
         AND {stream_id_column} = {stream_id}{segment_filter} \
         ORDER BY {segment_index} LIMIT 1",
        source = column::SOURCE,
        payload = column::PAYLOAD,
        episode_id_column = column::EPISODE_ID,
        episode_id = string_literal(episode_id),
        stream_id_column = column::STREAM_ID,
        stream_id = string_literal(stream_id),
        segment_index = column::SEGMENT_INDEX,
    )
}

pub(crate) fn training_checkpoint(dataset: &str, run_id: &str, step: u64) -> String {
    let checkpoints = relation(dataset, Table::TrainingCheckpoints);
    format!(
        "SELECT {config}, {checkpoint} FROM {checkpoints} \
         WHERE {run_column} = {run_id} AND {step_column} = {step} \
         ORDER BY {created} DESC LIMIT 2",
        config = column::CONFIG_JSON,
        checkpoint = column::CHECKPOINT,
        run_column = column::RUN_ID,
        run_id = string_literal(run_id),
        step_column = column::STEP,
        created = column::CREATED_MS,
    )
}

pub(crate) fn create_dataset_namespace(dataset: &str) -> String {
    format!(
        "CREATE TABLE {dataset} AS SELECT {} AS {}",
        string_literal(dataset),
        spiral::DATASET_NAMESPACE_MARKER
    )
}

pub(crate) fn video_sources_relation(dataset: &str) -> String {
    format!("{dataset}_video_sources")
}

pub(crate) fn video_dataset_relation(dataset: &str) -> String {
    format!("{dataset}.video")
}

pub(crate) fn video_dataset_output(dataset: &str) -> String {
    format!("{}.assets", video_dataset_relation(dataset))
}

pub(crate) fn create_video_sources(sources: &str, assets: &str) -> String {
    format!(
        "CREATE OR REPLACE VIEW {sources} AS SELECT assets.{episode_id}, assets.{stream_id}, \
         assets.{segment_index}, assets.{timeline_offset}, \
         assets.{payload} AS {source} \
         FROM {assets} AS assets",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        segment_index = column::SEGMENT_INDEX,
        timeline_offset = column::TIMELINE_OFFSET_NS,
        payload = column::PAYLOAD,
        source = column::SOURCE,
    )
}

pub(crate) fn migrate_video_assets_to_blob(assets: &str) -> String {
    format!(
        "CREATE OR REPLACE TABLE {assets} ENGINE 'arbor' AS SELECT \
         {episode_id}, {stream_id}, {segment_index}, {timeline_offset}, {camera_id}, \
         spql.blob.inline({payload}, {byte_size}, 'video/mp4') AS {payload}, \
         {byte_size}, {width}, {height}, {fps}, {frames}, {media_relation} FROM {assets}",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        segment_index = column::SEGMENT_INDEX,
        timeline_offset = column::TIMELINE_OFFSET_NS,
        camera_id = column::CAMERA_ID,
        payload = column::PAYLOAD,
        byte_size = column::BYTE_SIZE,
        width = column::WIDTH,
        height = column::HEIGHT,
        fps = column::FPS,
        frames = column::FRAMES,
        media_relation = column::MEDIA_RELATION,
    )
}

pub(crate) fn create_video_dataset(dataset: &str, sources: &str) -> String {
    format!(
        "CREATE DATASET {dataset} USING spql.mp4.dataset FROM {sources} \
         WITH (blob = '{}', index_tier = 'dependency')",
        column::SOURCE
    )
}

pub(crate) fn refresh_dataset(dataset: &str) -> String {
    format!("REFRESH DATASET {dataset}")
}

pub(crate) fn drop_video_dataset(dataset: &str) -> String {
    format!("DROP DATASET IF EXISTS {dataset}")
}

pub(crate) fn drop_video_sources(sources: &str) -> String {
    format!("DROP VIEW IF EXISTS {sources}")
}

pub(crate) fn ensure_read_only(sql: &str, mode: ReadOnlyMode) -> Result<(), String> {
    spiral_robotics_core::query::ensure_read_only(sql, mode).map_err(|error| error.to_string())
}

/// Escape a value for the SQL string-literal grammar. ADBC parameters are not
/// uniformly implemented by the Spiral drivers currently supported here, so
/// generated statements isolate this compatibility fallback in one function.
pub(crate) fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_values_are_escaped_in_one_place() {
        fn assert_single_literal(sql: &str, value: &str) {
            let literal = string_literal(value);
            assert!(
                sql.contains(&literal),
                "{sql:?} does not contain {literal:?}"
            );
            assert!(
                !sql.contains(&format!("'{literal}'")),
                "{sql:?} wraps the encoded literal {literal:?} in another pair of quotes"
            );
        }

        let cases = [
            (
                dataset_exists("robot.datasets", "dataset-'one"),
                "dataset-'one",
            ),
            (delete_episode("robot.episodes", "take-'one"), "take-'one"),
            (
                delete_training_run("robot.training_checkpoints", "run-'one"),
                "run-'one",
            ),
            (episode_samples("robot", "episode-'one"), "episode-'one"),
            (training_checkpoint("robot", "run-'two", 42), "run-'two"),
        ];
        for (sql, value) in cases {
            assert_single_literal(&sql, value);
        }

        let trajectory = trajectory("robot", "episode-'three", "stream-'three");
        assert_single_literal(&trajectory, "episode-'three");
        assert_single_literal(&trajectory, "stream-'three");

        let video = video_asset_size("robot", "episode-'four", "stream-'four", Some(0));
        assert_single_literal(&video, "episode-'four");
        assert_single_literal(&video, "stream-'four");
    }

    #[test]
    fn video_resources_are_shared_by_the_namespace() {
        assert_eq!(video_sources_relation("robot"), "robot_video_sources");
        assert_eq!(video_dataset_relation("robot"), "robot.video");
        assert_eq!(video_dataset_output("robot"), "robot.video.assets");
        assert_eq!(
            create_video_sources("robot_video_sources", "robot.video_assets"),
            "CREATE OR REPLACE VIEW robot_video_sources AS SELECT assets.episode_id, assets.stream_id, assets.segment_index, assets.timeline_offset_ns, assets.payload AS source FROM robot.video_assets AS assets"
        );
        assert_eq!(
            video_asset_range("robot", "episode", "camera.main", Some(2), 512, 1024),
            "SELECT spql.io.read_bytes(source, CAST(512 AS UINT64), CAST(1024 AS UINT64)) AS payload FROM robot.video.assets WHERE episode_id = 'episode' AND stream_id = 'camera.main' AND segment_index = 2 ORDER BY segment_index LIMIT 1"
        );
    }

    #[test]
    fn generated_training_relations_follow_the_shared_contract() {
        let sql = trajectory("robotics", "episode-1", "leader");
        for table in [Table::Episodes, Table::EpisodeStreams, Table::JointSamples] {
            assert!(sql.contains(&relation("robotics", table)));
        }
    }

    #[test]
    fn episode_sample_id_is_encoded_as_exactly_one_string_literal() {
        assert_eq!(
            episode_samples("robotics", "episode_000051"),
            "SELECT * FROM robotics.joint_samples WHERE episode_id = 'episode_000051' ORDER BY sample_index, stream_id"
        );
        assert_eq!(
            episode_samples("robotics", "episode_'quoted"),
            "SELECT * FROM robotics.joint_samples WHERE episode_id = 'episode_''quoted' ORDER BY sample_index, stream_id"
        );
    }

    #[test]
    fn read_only_gate_handles_comments_quotes_and_cte_mutation() {
        assert!(
            ensure_read_only("SELECT ';' AS punctuation; -- okay", ReadOnlyMode::Preview).is_ok()
        );
        assert!(
            ensure_read_only(
                "WITH recent AS (SELECT 1) SELECT * FROM recent",
                ReadOnlyMode::Training
            )
            .is_ok()
        );
        assert!(
            ensure_read_only(
                "WITH removed AS (DELETE FROM t) SELECT 1",
                ReadOnlyMode::Training
            )
            .is_err()
        );
        assert!(ensure_read_only("SELECT 1; DROP TABLE t", ReadOnlyMode::Preview).is_err());
    }
}
