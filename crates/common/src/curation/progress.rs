//! A replaceable native progress scorer, run on one recording at a time.
use crate::datasets::{CameraVideo, DatasetSelection};
use crate::sql::SqlLiteral;
use serde::{Deserialize, Serialize};

/// The released, task-specific WARP checkpoint. Its velocity is progress per
/// unit time relative to expert demonstrations; progress is normalized within
/// one episode, never a completion probability.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProgressScorer {
    WarpSimBottles,
}
impl ProgressScorer {
    pub const fn model(&self) -> &'static str {
        match self {
            Self::WarpSimBottles => {
                "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
            }
        }
    }
    pub const fn version(&self) -> &'static str {
        "warp_sim_bottles_v1"
    }
    /// The camera topic the checkpoint was trained on. A dataset that does not
    /// record it cannot be scored.
    pub const fn camera(&self) -> &'static str {
        match self {
            Self::WarpSimBottles => "/cameras/top_camera-images-rgb",
        }
    }
    /// The episode query that scores every frame of this scorer's camera in
    /// `dataset`, in display order. `$1` binds the episode's UUID.
    pub fn episode_score_query(&self, dataset: &DatasetSelection) -> String {
        format!(
            "{}, scored AS (SELECT spql.ai.score_progress(s.source, model => {}) AS scores FROM streams s) SELECT p.frame_index AS frame_index,p.timestamp_ns AS timestamp_ns,p.velocity AS velocity,p.progress AS progress,p.window_stride AS window_stride,p.support AS support FROM scored CROSS JOIN LATERAL UNNEST(scored.scores) p ORDER BY frame_index",
            CameraVideo::new(&dataset.sql_name(), self.camera()).streams(),
            SqlLiteral::text(self.model())
        )
    }
}

/// A request scores one recording, independently of every other.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressScoreRequest {
    episode: crate::datasets::EpisodeSelection,
}
impl ProgressScoreRequest {
    pub fn new(episode: crate::datasets::EpisodeSelection) -> Self {
        Self { episode }
    }
    pub fn episode(&self) -> &crate::datasets::EpisodeSelection {
        &self.episode
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProgressScoreResult {
    episode: crate::datasets::EpisodeSelection,
    frames: Vec<ProgressFrame>,
}
impl ProgressScoreResult {
    pub fn new(episode: crate::datasets::EpisodeSelection, frames: Vec<ProgressFrame>) -> Self {
        Self { episode, frames }
    }
    pub fn episode(&self) -> &crate::datasets::EpisodeSelection {
        &self.episode
    }
    pub fn frames(&self) -> &[ProgressFrame] {
        &self.frames
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProgressFrame {
    frame_index: u32,
    timestamp_ns: u64,
    velocity: f64,
    progress: Option<f64>,
    window_stride: u32,
    support: u32,
}
impl ProgressFrame {
    pub fn frame_index(&self) -> u32 {
        self.frame_index
    }
    pub fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }
    pub fn velocity(&self) -> f64 {
        self.velocity
    }
    pub fn progress(&self) -> Option<f64> {
        self.progress
    }
    pub fn support(&self) -> u32 {
        self.support
    }
}
