//! Where a dataset records each episode's task: what Break down tells its
//! model the robot was asked to do.
use serde::{Deserialize, Serialize};

/// Where a dataset records each episode's task.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeTask {
    /// The first message an episode records on this topic.
    Recorded(String),
    /// The first task its dataset lists beside the episode: a LeRobot
    /// episode's `tasks`.
    Listed,
}
