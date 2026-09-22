//! The episodes one annotation node walks: what the pass needs to sample them
//! and what the player needs to show them.
use crate::activities::data::{DatasetClient, OpenedEpisode, StreamCatalog};
use dioxus::prelude::*;
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;

/// An annotation pass reads whole recordings, so the list stays small enough
/// that one operator can watch it finish.
const MAX_ANNOTATED_EPISODES: usize = 2_000;

/// The topic a dataset records the operator's own words on, when it has one.
const INSTRUCTION_TOPIC: &str = "/instruction";

/// Camera names worth annotating first: a view of the whole workspace shows
/// what the arm is doing to what, which a camera on the gripper cannot.
const OVERVIEW_NAMES: [&str; 4] = ["top", "front", "head", "overview"];
/// Camera names worth annotating last: they follow the gripper, so the scene
/// leaves the frame exactly when a subtask boundary matters.
const CLOSE_UP_NAMES: [&str; 2] = ["wrist", "gripper"];

/// The camera a Break down subtasks node starts with, out of the cameras this
/// dataset recorded. The operator can still choose another.
pub(super) fn preferred_camera(cameras: &[String]) -> Option<&str> {
    fn rank(camera: &str) -> u8 {
        let name = camera.to_lowercase();
        if OVERVIEW_NAMES.iter().any(|word| name.contains(word)) {
            0
        } else if CLOSE_UP_NAMES.iter().any(|word| name.contains(word)) {
            2
        } else {
            1
        }
    }
    // `min_by_key` keeps the first of equal ranks, so the dataset's own topic
    // order decides between two cameras that read alike.
    cameras
        .iter()
        .min_by_key(|camera| rank(camera))
        .map(String::as_str)
}

/// One episode a pass annotates.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AnnotationEpisode {
    id: String,
    /// The recording's first and last message time, in nanoseconds; camera
    /// packets are read over this span and timed from its start.
    bounds: (u64, u64),
    opened: OpenedEpisode,
}

impl AnnotationEpisode {
    /// One row of the episode listing below.
    fn from_row(dataset: &DatasetSelection, row: Map<String, Value>) -> Result<Self, String> {
        let read = |name: &str| row.get(name).and_then(Value::as_u64);
        let (start, end) = (read("message_start_time"), read("message_end_time"));
        let (Some(start), Some(end)) = (start, end) else {
            return Err("This episode has no recorded time range.".into());
        };
        let opened = OpenedEpisode::from_row(dataset.clone(), row)?;
        Ok(Self {
            id: opened.selection().episode_id().to_owned(),
            bounds: (start, end.max(start)),
            opened,
        })
    }

    /// Every episode of the dataset, in recording order, as the Data rail
    /// lists them.
    pub(super) async fn load(dataset: &DatasetSelection) -> Result<Vec<Self>, String> {
        let statement = format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, message_end_time, duration_ns, channel_count \
             FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source_uri",
            dataset.sql_name().replace('\'', "''")
        );
        let snapshot = DatasetClient::current()
            .run_spiral_query(statement, MAX_ANNOTATED_EPISODES)
            .await
            .map_err(|error| error.to_string())?;
        snapshot
            .into_rows()
            .into_iter()
            .map(|row| Self::from_row(dataset, row))
            .collect()
    }

    /// The camera topics the operator may annotate, read from one episode of
    /// the dataset: every episode of one recording session carries the same
    /// cameras, so the first one answers for the rest.
    pub(super) fn use_cameras(first: Option<Self>) -> Vec<String> {
        let cameras = use_resource(use_reactive(&first, move |first| async move {
            let Some(first) = first else {
                return Ok(Vec::new());
            };
            let selection = first.opened().selection();
            let topics = DatasetClient::current()
                .episode_topics(selection.clone())
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(
                StreamCatalog::new(topics, &selection)
                    .videos()
                    .map(|stream| stream.read().topic().topic().to_owned())
                    .collect(),
            )
        }));
        let listed = cameras
            .read()
            .as_ref()
            .and_then(|loaded| loaded.as_ref().ok())
            .cloned();
        listed.unwrap_or_default()
    }

    /// What the operator asked the robot to do, when the dataset records it on
    /// an instruction topic. A dataset without one simply has no task text, so
    /// a missing topic or an unreadable row is silence rather than a failure.
    pub(super) async fn task(&self, datasets: &DatasetClient) -> Option<String> {
        let selection = self.opened().selection();
        let statement = format!(
            "SELECT r.value.data AS instruction FROM spql.mcap.read_topic({}, {}) r \
             WHERE r.id = $1",
            SqlLiteral::text(&selection.dataset().sql_name()),
            SqlLiteral::text(INSTRUCTION_TOPIC)
        );
        let snapshot = datasets
            .run_episode_query(&EpisodeQueryRequest::new(selection, statement, 1))
            .await
            .ok()?;
        snapshot
            .rows()
            .first()?
            .get("instruction")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|task| !task.is_empty())
            .map(str::to_owned)
    }

    pub(super) fn id(&self) -> &str {
        &self.id
    }
    /// Seconds from the first recorded message to the last.
    pub(super) fn duration(&self) -> f64 {
        self.bounds.1.saturating_sub(self.bounds.0) as f64 / 1e9
    }
    pub(super) fn opened(&self) -> &OpenedEpisode {
        &self.opened
    }

    /// One episode of a fixed length, for tests that only need its identity
    /// and duration.
    #[cfg(test)]
    pub(super) fn fixture(id: &str, duration_ns: u64) -> Self {
        let Value::Object(row) = serde_json::json!({
            "id": id,
            "source_uri": format!("s3://recordings/{id}.mcap"),
            "source_bytes": 1_024,
            "message_count": 10,
            "message_start_time": 1_000_000_000_u64,
            "message_end_time": 1_000_000_000_u64 + duration_ns,
            "duration_ns": duration_ns,
            "channel_count": 3,
        }) else {
            panic!("an episode row is an object");
        };
        Self::from_row(
            &DatasetSelection::new(vec!["folding".into()]).expect("a dataset name"),
            row,
        )
        .expect("the fixture row describes one episode")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_episode_takes_its_identity_and_length_from_the_recorded_bounds() {
        let episode = AnnotationEpisode::fixture("ep-1", 20_500_000_000);
        assert_eq!(episode.id(), "ep-1");
        assert!((episode.duration() - 20.5).abs() < 1e-9);
        assert_eq!(episode.opened().title(), "ep-1");
    }

    #[test]
    fn the_default_camera_looks_at_the_workspace_before_the_gripper() {
        let named = |names: [&str; 3]| names.map(str::to_owned).to_vec();
        assert_eq!(
            preferred_camera(&named([
                "/left-wrist-camera",
                "/top-camera",
                "/right-wrist-camera"
            ])),
            Some("/top-camera")
        );
        assert_eq!(
            preferred_camera(&named(["/cam_low", "/cam_right_wrist", "/HEAD_CAM"])),
            Some("/HEAD_CAM")
        );
        assert_eq!(
            preferred_camera(&named(["/gripper_cam", "/scene", "/front_rgb"])),
            Some("/front_rgb")
        );
        assert_eq!(preferred_camera(&[]), None);
    }

    #[test]
    fn an_episode_without_a_recorded_range_is_not_annotated() {
        let Value::Object(row) = json!({
            "id": "ep-2",
            "source_uri": Value::Null,
            "source_bytes": Value::Null,
            "message_count": Value::Null,
            "message_start_time": Value::Null,
            "message_end_time": Value::Null,
            "duration_ns": Value::Null,
            "channel_count": Value::Null,
        }) else {
            panic!("an episode row is an object");
        };
        let dataset = DatasetSelection::new(vec!["folding".into()]).unwrap();
        assert!(AnnotationEpisode::from_row(&dataset, row).is_err());
    }
}
