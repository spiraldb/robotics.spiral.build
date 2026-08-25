//! Dataset identity and import contracts.

use serde::{Deserialize, Serialize};

/// What replacing the active SpiralDB project from a portable snapshot imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectImportSummary {
    pub relations: u64,
    pub rows: u64,
    /// Non-transactional derived resources that still need repair after the
    /// durable relational replacement committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// What merging a portable episode bundle into one dataset imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeImportSummary {
    pub dataset_id: String,
    pub episodes: u64,
    pub rows: u64,
    pub first_episode: Option<String>,
    /// Non-transactional derived resources that still need repair after the
    /// durable episode rows committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
pub struct HuggingFaceDatasetRequest {
    pub dataset_id: String,
    pub repository: String,
    #[serde(default = "default_huggingface_revision")]
    pub revision: String,
    pub format: HuggingFaceDatasetFormat,
}

fn default_huggingface_revision() -> String {
    "main".to_owned()
}

/// One immutable repository file selected for import.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetFile {
    pub path: String,
    #[serde(default)]
    pub bytes: u64,
}

/// Read-only source inspection shown before Studio changes the Spiral catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetInspection {
    pub dataset_id: String,
    pub repository: String,
    pub requested_revision: String,
    pub resolved_revision: String,
    pub format: HuggingFaceDatasetFormat,
    pub files: Vec<HuggingFaceDatasetFile>,
    pub total_bytes: u64,
    pub planned_relations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Durable result of materializing one inspected Hugging Face snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HuggingFaceDatasetImportSummary {
    pub dataset_id: String,
    pub repository: String,
    pub resolved_revision: String,
    pub format: HuggingFaceDatasetFormat,
    pub files: u64,
    pub total_bytes: u64,
    pub relations: Vec<String>,
    pub suggested_sql: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Stable fallback for old recordings and clients created before datasets
/// became first-class rows.
pub(crate) fn default_dataset_id() -> String {
    "default".to_owned()
}
