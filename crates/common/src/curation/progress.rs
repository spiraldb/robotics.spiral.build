//! A replaceable native progress scorer, run on one recording at a time.
use crate::sql::SqlLiteral;
use serde::{Deserialize, Serialize};

/// The released, task-specific WARP checkpoint. Its velocity is progress per
/// unit time relative to expert demonstrations; progress is normalized within
/// one episode, never a completion probability.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
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
    fn expression(&self) -> String {
        format!(
            "spql.ai.score_progress(spql.media.from_blob(spql.blob.external(s.source_uri), 'mp4'), CAST(s.start_frame AS UINT64), CAST(s.frame_count AS UINT64), model => {})",
            SqlLiteral::text(self.model())
        )
    }
}

/// Which scorer reads a dataset's recordings, and the relation that binds each
/// of its episodes to the original video that scorer was trained on.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressSelection {
    scorer: ProgressScorer,
    sources_relation: String,
}
impl ProgressSelection {
    /// The scorer for `dataset`. Only the imported simulated-bottles sample has
    /// its original recordings bound, so every other dataset has no relation
    /// and fails validation.
    pub fn for_dataset(dataset: &crate::datasets::DatasetSelection) -> Self {
        let sources_relation = match dataset.sql_name().as_str() {
            "dataset_warp_sim_bottles_sample" => "warp_sim_bottles_sources_v1".into(),
            _ => String::new(),
        };
        Self {
            scorer: ProgressScorer::WarpSimBottles,
            sources_relation,
        }
    }
    pub fn scorer(&self) -> &ProgressScorer {
        &self.scorer
    }
    pub fn sources_relation(&self) -> &str {
        &self.sources_relation
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.sources_relation.is_empty()
            || !self
                .sources_relation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("Progress scoring currently supports the imported simulated-bottles dataset with its original recordings.".into());
        }
        Ok(())
    }
    pub fn episode_score_query(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "WITH scored AS (SELECT {} AS scores FROM \"{}\" s WHERE s.id=$1) SELECT p.frame_index AS frame_index,p.source_frame_index AS source_frame_index,p.timestamp_ns AS timestamp_ns,p.velocity AS velocity,p.progress AS progress,p.window_stride AS window_stride,p.support AS support FROM scored CROSS JOIN LATERAL UNNEST(scored.scores) p ORDER BY frame_index",
            self.scorer.expression(),
            self.sources_relation
        ))
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
    /// The scorer and bound recordings this episode's dataset is read with.
    pub fn selection(&self) -> ProgressSelection {
        ProgressSelection::for_dataset(self.episode.dataset())
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
    source_frame_index: u32,
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
