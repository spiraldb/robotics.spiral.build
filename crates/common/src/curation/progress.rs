//! A replaceable native progress scorer and non-destructive training chunks.
use crate::sql::SqlLiteral;
use serde::{Deserialize, Serialize};

/// The released, task-specific WARP checkpoint. Its velocity is progress per
/// unit time relative to expert demonstrations; progress is normalized within
/// one episode, never a completion probability.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressSelection {
    scorer: ProgressScorer,
    sources_relation: String,
    #[serde(
        default,
        deserialize_with = "ProgressSelection::automatic_minimum_velocity"
    )]
    minimum_velocity: f64,
    chunk_frames: u32,
}
impl ProgressSelection {
    pub(super) fn new(sources_relation: String) -> Self {
        Self {
            scorer: ProgressScorer::WarpSimBottles,
            sources_relation,
            minimum_velocity: 0.0,
            chunk_frames: 30,
        }
    }
    pub(super) fn for_dataset(dataset: Option<&crate::datasets::DatasetSelection>) -> Self {
        let mut selection = Self::new(String::new());
        selection.resolve_dataset(dataset);
        selection
    }
    pub(super) fn resolve_dataset(&mut self, dataset: Option<&crate::datasets::DatasetSelection>) {
        self.sources_relation = match dataset
            .map(crate::datasets::DatasetSelection::sql_name)
            .as_deref()
        {
            Some("dataset_warp_sim_bottles_sample") => "warp_sim_bottles_sources_v1".into(),
            _ => String::new(),
        };
    }
    pub fn scorer(&self) -> &ProgressScorer {
        &self.scorer
    }
    pub fn sources_relation(&self) -> &str {
        &self.sources_relation
    }
    pub const fn minimum_velocity(&self) -> f64 {
        self.minimum_velocity
    }
    pub const fn chunk_frames(&self) -> u32 {
        self.chunk_frames
    }
    /// Legacy authored cutoffs are decoded into the same fixed policy used by
    /// new rules. Reading a recipe does not update its persisted document.
    fn automatic_minimum_velocity<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<f64, D::Error> {
        let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(0.0)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.sources_relation.is_empty()
            || !self
                .sources_relation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("automatic detection currently supports the imported simulated-bottles dataset with its original recordings".into());
        }
        if !(1..=4096).contains(&self.chunk_frames) {
            return Err("choose a training chunk length from 1 to 4096 frames".into());
        }
        Ok(())
    }
    /// A user-initiated whole-recipe run evaluates model scores as a transient
    /// SQL input and stores only its final chunk decisions.
    pub fn run_score_query(&self, roster: &str, incoming: &str) -> String {
        format!(
            "WITH scored AS (SELECT s.id, {} AS scores FROM \"{}\" s JOIN \"{roster}\" v ON s.id=v.id WHERE {incoming}) SELECT s.id,p.frame_index AS frame_index,p.timestamp_ns AS timestamp_ns,p.velocity AS velocity,p.support AS support FROM scored s CROSS JOIN LATERAL UNNEST(s.scores) p",
            self.scorer.expression(),
            self.sources_relation
        )
    }
    pub fn chunks_sql(&self, source: &str) -> String {
        format!(
            "SELECT id, CAST(FLOOR(frame_index / {}) AS UINT64) AS chunk_index, min(frame_index) AS start_frame, max(frame_index) + 1 AS end_frame, min(timestamp_ns) AS start_ns, CAST((CAST(max(frame_index) + 1 AS UINT64) * 1000000000 + 15) / 30 AS UINT64) AS end_ns, avg(velocity) AS mean_velocity, min(support) AS support, count(*) AS observed_frames FROM \"{source}\" GROUP BY id, CAST(FLOOR(frame_index / {}) AS UINT64)",
            self.chunk_frames, self.chunk_frames
        )
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

/// A review request scores only this recording, independently of other rows.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressScoreRequest {
    episode: crate::datasets::EpisodeSelection,
    selection: ProgressSelection,
}
impl ProgressScoreRequest {
    pub fn new(episode: crate::datasets::EpisodeSelection, selection: ProgressSelection) -> Self {
        Self { episode, selection }
    }
    pub fn resolve_source(&mut self) {
        self.selection.resolve_dataset(Some(self.episode.dataset()));
    }
    pub fn episode(&self) -> &crate::datasets::EpisodeSelection {
        &self.episode
    }
    pub fn selection(&self) -> &ProgressSelection {
        &self.selection
    }
}
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_cutoffs_decode_to_the_fixed_automatic_policy() {
        let original = ProgressSelection::new("warp_sim_bottles_sources_v1".into());
        for old_cutoff in [0.5, -1.0, 1.0] {
            let mut document = serde_json::to_value(&original).unwrap();
            document["minimum_velocity"] = serde_json::json!(old_cutoff);
            let decoded: ProgressSelection = serde_json::from_value(document.clone()).unwrap();
            assert_eq!(decoded.minimum_velocity(), 0.0);
            assert_eq!(decoded, original);
            assert_eq!(
                serde_json::to_value(&decoded).unwrap()["minimum_velocity"],
                0.0
            );
            assert_eq!(document["minimum_velocity"], old_cutoff);
        }
    }
}
