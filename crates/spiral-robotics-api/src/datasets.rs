//! Dataset identity and import contracts.

use serde::{Deserialize, Serialize};

/// What replacing the active SpiralDB project from a portable snapshot imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectImportSummary {
    relations: u64,
    rows: u64,
    /// Non-transactional derived resources that still need repair after the
    /// durable relational replacement committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl ProjectImportSummary {
    pub const fn relations(&self) -> u64 {
        self.relations
    }
    pub const fn rows(&self) -> u64 {
        self.rows
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub const fn record_relation(&mut self, rows: u64) {
        self.relations = self.relations.saturating_add(1);
        self.rows = self.rows.saturating_add(rows);
    }
    pub fn push_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }
}

/// What merging a portable episode bundle into one dataset imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeImportSummary {
    dataset_id: String,
    episodes: u64,
    rows: u64,
    first_episode: Option<String>,
    /// Non-transactional derived resources that still need repair after the
    /// durable episode rows committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl EpisodeImportSummary {
    pub fn new(dataset_id: impl Into<String>, episodes: u64, rows: u64) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            episodes,
            rows,
            first_episode: None,
            warnings: Vec::new(),
        }
    }
    pub fn with_first_episode(mut self, first_episode: Option<String>) -> Self {
        self.first_episode = first_episode;
        self
    }
    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }
    pub const fn episodes(&self) -> u64 {
        self.episodes
    }
    pub const fn rows(&self) -> u64 {
        self.rows
    }
    pub fn first_episode(&self) -> Option<&str> {
        self.first_episode.as_deref()
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn push_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }
}

/// Dataset layouts that Studio can materialize from a Hugging Face dataset
/// repository using Spiral's native readers and Dataset producers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HuggingFaceDatasetFormat {
    #[serde(rename = "lerobot")]
    LeRobot,
    Mcap,
}

impl HuggingFaceDatasetFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::LeRobot => "LeRobot",
            Self::Mcap => "MCAP",
        }
    }
}

/// Source and destination shared by inspection and the eventual import.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetSource {
    dataset_id: String,
    repository: String,
    #[serde(default = "HuggingFaceDatasetSource::default_revision")]
    revision: String,
    format: HuggingFaceDatasetFormat,
}

impl HuggingFaceDatasetSource {
    fn default_revision() -> String {
        "main".to_owned()
    }

    pub fn new(
        dataset_id: impl Into<String>,
        repository: impl Into<String>,
        format: HuggingFaceDatasetFormat,
    ) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            repository: repository.into(),
            revision: Self::default_revision(),
            format,
        }
    }
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = revision.into();
        self
    }
    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub const fn format(&self) -> HuggingFaceDatasetFormat {
        self.format
    }
}

/// One immutable repository file selected for import.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetFile {
    path: String,
    #[serde(default)]
    bytes: u64,
}

impl HuggingFaceDatasetFile {
    pub fn new(path: impl Into<String>, bytes: u64) -> Self {
        Self {
            path: path.into(),
            bytes,
        }
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Read-only source inspection shown before Studio changes the Spiral catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetInspection {
    #[serde(flatten)]
    source: HuggingFaceInspectionSource,
    #[serde(flatten)]
    contents: HuggingFaceInspectionContents,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceInspectionSource {
    dataset_id: String,
    repository: String,
    requested_revision: String,
    resolved_revision: String,
    format: HuggingFaceDatasetFormat,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceInspectionContents {
    files: Vec<HuggingFaceDatasetFile>,
    total_bytes: u64,
    planned_relations: Vec<String>,
}

/// Durable result of materializing one inspected Hugging Face snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetImportSummary {
    #[serde(flatten)]
    source: HuggingFaceResolvedSource,
    #[serde(flatten)]
    outcome: HuggingFaceImportOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceResolvedSource {
    dataset_id: String,
    repository: String,
    resolved_revision: String,
    format: HuggingFaceDatasetFormat,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceImportOutcome {
    files: u64,
    total_bytes: u64,
    relations: Vec<String>,
    suggested_sql: String,
}

impl HuggingFaceInspectionSource {
    pub fn new(
        dataset_id: impl Into<String>,
        repository: impl Into<String>,
        format: HuggingFaceDatasetFormat,
    ) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            repository: repository.into(),
            requested_revision: String::new(),
            resolved_revision: String::new(),
            format,
        }
    }
    pub fn with_revisions(
        mut self,
        requested_revision: impl Into<String>,
        resolved_revision: impl Into<String>,
    ) -> Self {
        self.requested_revision = requested_revision.into();
        self.resolved_revision = resolved_revision.into();
        self
    }
}

impl HuggingFaceInspectionContents {
    pub fn new(
        files: Vec<HuggingFaceDatasetFile>,
        total_bytes: u64,
        planned_relations: Vec<String>,
    ) -> Self {
        Self {
            files,
            total_bytes,
            planned_relations,
        }
    }
}

impl HuggingFaceDatasetInspection {
    pub fn new(
        source: HuggingFaceInspectionSource,
        contents: HuggingFaceInspectionContents,
    ) -> Self {
        Self {
            source,
            contents,
            warnings: Vec::new(),
        }
    }
    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }
    pub fn dataset_id(&self) -> &str {
        &self.source.dataset_id
    }
    pub fn repository(&self) -> &str {
        &self.source.repository
    }
    pub fn resolved_revision(&self) -> &str {
        &self.source.resolved_revision
    }
    pub const fn format(&self) -> HuggingFaceDatasetFormat {
        self.source.format
    }
    pub fn files(&self) -> &[HuggingFaceDatasetFile] {
        &self.contents.files
    }
    pub const fn total_bytes(&self) -> u64 {
        self.contents.total_bytes
    }
    pub fn planned_relations(&self) -> &[String] {
        &self.contents.planned_relations
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl HuggingFaceResolvedSource {
    pub fn new(
        dataset_id: impl Into<String>,
        repository: impl Into<String>,
        format: HuggingFaceDatasetFormat,
    ) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            repository: repository.into(),
            resolved_revision: String::new(),
            format,
        }
    }
    pub fn with_resolved_revision(mut self, resolved_revision: impl Into<String>) -> Self {
        self.resolved_revision = resolved_revision.into();
        self
    }
}

impl HuggingFaceImportOutcome {
    pub fn new(files: u64, total_bytes: u64, relations: Vec<String>) -> Self {
        Self {
            files,
            total_bytes,
            relations,
            suggested_sql: String::new(),
        }
    }
    pub fn with_suggested_sql(mut self, suggested_sql: impl Into<String>) -> Self {
        self.suggested_sql = suggested_sql.into();
        self
    }
}

impl HuggingFaceDatasetImportSummary {
    pub fn new(source: HuggingFaceResolvedSource, outcome: HuggingFaceImportOutcome) -> Self {
        Self {
            source,
            outcome,
            warnings: Vec::new(),
        }
    }
    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }
    pub fn repository(&self) -> &str {
        &self.source.repository
    }
    pub fn resolved_revision(&self) -> &str {
        &self.source.resolved_revision
    }
    pub const fn format(&self) -> HuggingFaceDatasetFormat {
        self.source.format
    }
    pub const fn files(&self) -> u64 {
        self.outcome.files
    }
    pub fn relations(&self) -> &[String] {
        &self.outcome.relations
    }
    pub fn suggested_sql(&self) -> &str {
        &self.outcome.suggested_sql
    }
}
