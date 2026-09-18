//! One judged pass over a dataset: which episodes stayed, which left, and why.
use super::CurationSpec;
use crate::runs::{ObservedRun, RunObservation, RunPhase};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RunIdentity {
    curation_id: String,
    curation_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RunExecution {
    phase: RunPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    result_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_relation: Option<String>,
    #[serde(default)]
    derived_relations: Vec<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct RunTiming {
    #[serde(default)]
    started_ms: u64,
    #[serde(default)]
    finished_ms: u64,
    #[serde(default)]
    updated_ms: u64,
}

/// The authoritative state of one run, published by the backend.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CurationRunSnapshot {
    #[serde(flatten)]
    identity: RunIdentity,
    #[serde(flatten)]
    execution: RunExecution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<CurationOutcome>,
    #[serde(flatten)]
    timing: RunTiming,
}

impl CurationRunSnapshot {
    pub fn idle_for(curation: &CurationSpec) -> Self {
        Self {
            identity: RunIdentity {
                curation_id: curation.id().to_owned(),
                curation_revision: curation.revision(),
                run_id: None,
            },
            ..Self::default()
        }
    }
    pub fn started(curation: &CurationSpec, run_id: impl Into<String>) -> Self {
        let mut snapshot = Self::idle_for(curation);
        snapshot.identity.run_id = Some(run_id.into());
        snapshot.execution.phase = RunPhase::Validating;
        snapshot
    }
    pub const fn with_started_ms(mut self, started_ms: u64) -> Self {
        self.timing.started_ms = started_ms;
        self.timing.updated_ms = started_ms;
        self
    }
    pub fn curation_id(&self) -> &str {
        &self.identity.curation_id
    }
    pub fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    pub const fn phase(&self) -> RunPhase {
        self.execution.phase
    }
    pub fn sql(&self) -> Option<&str> {
        self.execution.sql.as_deref()
    }
    pub fn error(&self) -> Option<&str> {
        self.execution.error.as_deref()
    }
    pub fn output_relation(&self) -> Option<&str> {
        self.execution.output_relation.as_deref()
    }
    pub fn set_output_relation(&mut self, relation: String) {
        self.execution.output_relation = Some(relation);
    }
    pub fn derived_relations(&self) -> &[String] {
        &self.execution.derived_relations
    }
    pub fn set_derived_relations(&mut self, relations: Vec<String>) {
        self.execution.derived_relations = relations;
    }
    pub const fn result_stale(&self) -> bool {
        self.execution.result_stale
    }
    pub const fn outcome(&self) -> Option<&CurationOutcome> {
        self.outcome.as_ref()
    }
    pub const fn started_ms(&self) -> u64 {
        self.timing.started_ms
    }
    pub const fn updated_ms(&self) -> u64 {
        self.timing.updated_ms
    }
    pub const fn set_phase(&mut self, phase: RunPhase) {
        self.execution.phase = phase;
    }
    pub fn set_sql(&mut self, sql: Option<String>) {
        self.execution.sql = sql;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.execution.error = error;
    }
    pub fn set_outcome(&mut self, outcome: Option<CurationOutcome>) {
        self.outcome = outcome;
    }
    pub const fn set_result_stale(&mut self, stale: bool) {
        self.execution.result_stale = stale;
    }
    pub const fn set_finished_ms(&mut self, finished_ms: u64) {
        self.timing.finished_ms = finished_ms;
    }
    pub const fn set_updated_ms(&mut self, updated_ms: u64) {
        self.timing.updated_ms = updated_ms;
    }
}

impl RunObservation for CurationRunSnapshot {
    fn document(&self) -> (&str, u64) {
        (&self.identity.curation_id, self.identity.curation_revision)
    }
    fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    fn phase(&self) -> RunPhase {
        self.execution.phase
    }
    fn started_ms(&self) -> u64 {
        self.timing.started_ms
    }
    fn updated_ms(&self) -> u64 {
        self.timing.updated_ms
    }
}

/// Exact counts over the whole population, even when the listed roster is
/// bounded.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurationTotals {
    episodes: u64,
    kept: u64,
    listed: u64,
    #[serde(default)]
    truncated: bool,
}

impl CurationTotals {
    pub const fn new(episodes: u64, kept: u64, listed: (u64, bool)) -> Self {
        Self {
            episodes,
            kept,
            listed: listed.0,
            truncated: listed.1,
        }
    }
    pub const fn episodes(&self) -> u64 {
        self.episodes
    }
    pub const fn kept(&self) -> u64 {
        self.kept
    }
    pub const fn excluded(&self) -> u64 {
        self.episodes.saturating_sub(self.kept)
    }
    pub const fn listed(&self) -> u64 {
        self.listed
    }
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

/// The episode statistics every verdict carries, straight from the roster.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeFacts {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_start_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ns: Option<u64>,
}

impl EpisodeFacts {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn source_uri(&self) -> Option<&str> {
        self.source_uri.as_deref()
    }
    pub const fn source_bytes(&self) -> Option<u64> {
        self.source_bytes
    }
    pub const fn message_count(&self) -> Option<u64> {
        self.message_count
    }
    pub const fn channel_count(&self) -> Option<u64> {
        self.channel_count
    }
    pub const fn message_start_time(&self) -> Option<u64> {
        self.message_start_time
    }
    pub const fn duration_ns(&self) -> Option<u64> {
        self.duration_ns
    }
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration_ns.map(|nanos| nanos as f64 / 1e9)
    }
    /// The file name at the end of the source path, when there is one.
    pub fn file_name(&self) -> &str {
        self.source_uri
            .as_deref()
            .and_then(|uri| uri.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }
}

/// One episode's judgement: its facts, the rules that fired, and the
/// transforms it would receive.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CurationVerdict {
    episode: EpisodeFacts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    facts: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    excluded_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    transforms: Vec<String>,
    #[serde(default)]
    eligible_transforms: Vec<String>,
}

impl CurationVerdict {
    pub fn new(episode: EpisodeFacts) -> Self {
        Self {
            episode,
            ..Self::default()
        }
    }
    pub fn with_fact(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.facts.insert(key.into(), value);
        self
    }
    pub fn with_exclusion(mut self, rule_id: impl Into<String>) -> Self {
        self.excluded_by.push(rule_id.into());
        self
    }
    pub fn with_transform(mut self, transform_id: impl Into<String>) -> Self {
        self.transforms.push(transform_id.into());
        self
    }
    pub const fn episode(&self) -> &EpisodeFacts {
        &self.episode
    }
    pub const fn facts(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.facts
    }
    pub fn fact(&self, key: &str) -> Option<&serde_json::Value> {
        self.facts.get(key)
    }
    pub fn excluded_by(&self) -> &[String] {
        &self.excluded_by
    }
    pub fn with_eligible_transform(mut self, id: String) -> Self {
        self.eligible_transforms.push(id);
        self
    }
    pub fn eligible_transforms(&self) -> &[String] {
        &self.eligible_transforms
    }
    pub fn transforms(&self) -> &[String] {
        &self.transforms
    }
    pub fn kept(&self) -> bool {
        self.excluded_by.is_empty()
    }
}

/// Everything a completed run produced.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CurationOutcome {
    totals: CurationTotals,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rule_counts: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    transform_counts: BTreeMap<String, u64>,
    #[serde(default)]
    verdicts: Vec<CurationVerdict>,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    stage_impacts: Vec<CurationStageImpact>,
}

impl CurationOutcome {
    pub fn new(totals: CurationTotals, verdicts: Vec<CurationVerdict>) -> Self {
        Self {
            totals,
            rule_counts: BTreeMap::new(),
            transform_counts: BTreeMap::new(),
            verdicts,
            elapsed_ms: 0,
            stage_impacts: Vec::new(),
        }
    }
    pub fn stage_impacts(&self) -> &[CurationStageImpact] {
        &self.stage_impacts
    }
    pub fn with_stage_impacts(mut self, impacts: Vec<CurationStageImpact>) -> Self {
        self.stage_impacts = impacts;
        self
    }
    pub fn with_counts(
        mut self,
        rule_counts: BTreeMap<String, u64>,
        transform_counts: BTreeMap<String, u64>,
    ) -> Self {
        self.rule_counts = rule_counts;
        self.transform_counts = transform_counts;
        self
    }
    pub const fn with_elapsed_ms(mut self, elapsed_ms: u64) -> Self {
        self.elapsed_ms = elapsed_ms;
        self
    }
    pub const fn totals(&self) -> &CurationTotals {
        &self.totals
    }
    pub fn rule_count(&self, rule_id: &str) -> u64 {
        self.rule_counts.get(rule_id).copied().unwrap_or(0)
    }
    pub fn transform_count(&self, transform_id: &str) -> u64 {
        self.transform_counts
            .get(transform_id)
            .copied()
            .unwrap_or(0)
    }
    pub fn verdicts(&self) -> &[CurationVerdict] {
        &self.verdicts
    }
    pub const fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
}

/// The exact judged recipe travels with its run.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CurationRun {
    curation: CurationSpec,
    snapshot: CurationRunSnapshot,
}

impl CurationRun {
    pub fn new(curation: CurationSpec, snapshot: CurationRunSnapshot) -> Self {
        Self { curation, snapshot }
    }
    pub const fn curation(&self) -> &CurationSpec {
        &self.curation
    }
    pub const fn snapshot(&self) -> &CurationRunSnapshot {
        &self.snapshot
    }
    pub fn snapshot_mut(&mut self) -> &mut CurationRunSnapshot {
        &mut self.snapshot
    }
}

impl ObservedRun for CurationRun {
    type Snapshot = CurationRunSnapshot;
    fn snapshot(&self) -> &CurationRunSnapshot {
        &self.snapshot
    }
}

/// Exact sequential impact, measured over the population entering this node.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CurationStageImpact {
    id: String,
    incoming: u64,
    retained: u64,
    #[serde(default)]
    time: Option<StageTimeImpact>,
    #[serde(default)]
    affected_episodes: Option<u64>,
    #[serde(default)]
    scored_episode_ids: Option<Vec<String>>,
}
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct StageTimeImpact {
    retained_ns: u64,
    excluded_ns: u64,
}
impl CurationStageImpact {
    pub fn new(id: String, incoming: u64, retained: u64) -> Self {
        Self {
            id,
            incoming,
            retained,
            time: None,
            affected_episodes: None,
            scored_episode_ids: None,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn incoming(&self) -> u64 {
        self.incoming
    }
    pub const fn retained(&self) -> u64 {
        self.retained
    }
    pub fn excluded(&self) -> u64 {
        self.incoming.saturating_sub(self.retained)
    }
    pub fn retained_ns(&self) -> Option<u64> {
        self.time.as_ref().map(|time| time.retained_ns)
    }
    pub fn excluded_ns(&self) -> Option<u64> {
        self.time.as_ref().map(|time| time.excluded_ns)
    }
    pub fn affected_episodes(&self) -> Option<u64> {
        self.affected_episodes
    }
    pub fn set_affected_episodes(&mut self, count: u64) {
        self.affected_episodes = Some(count);
    }
    pub fn scored_episodes(&self) -> Option<u64> {
        self.scored_episode_ids.as_ref().map(|ids| ids.len() as u64)
    }
    pub fn scored_episode_ids(&self) -> &[String] {
        self.scored_episode_ids.as_deref().unwrap_or_default()
    }
    pub fn set_scored_episode_ids(&mut self, ids: Vec<String>) {
        self.scored_episode_ids = Some(ids);
    }
    pub fn set_time(&mut self, retained_ns: u64, excluded_ns: u64) {
        self.time = Some(StageTimeImpact {
            retained_ns,
            excluded_ns,
        });
    }
}
