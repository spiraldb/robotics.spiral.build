//! A dataset imported from Hugging Face: a run the backend holds while it
//! downloads, decodes and writes every episode, which the browser polls for
//! how far it has got.
use super::{DatasetFormat, HuggingFaceDatasetSource};
use serde::{Deserialize, Serialize};

/// What the browser asks to import: a repository, read as `format`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetImportRequest {
    format: DatasetFormat,
    source: HuggingFaceDatasetSource,
}

impl DatasetImportRequest {
    pub fn new(format: DatasetFormat, source: HuggingFaceDatasetSource) -> Self {
        Self { format, source }
    }
    pub fn format(&self) -> DatasetFormat {
        self.format
    }
    pub fn source(&self) -> &HuggingFaceDatasetSource {
        &self.source
    }
    pub fn into_source(self) -> HuggingFaceDatasetSource {
        self.source
    }
}

/// Where an import run has got, in the order it gets there.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum DatasetImportPhase {
    /// Reading the repository's file list and metadata.
    ListingFiles,
    /// Writing episodes, 32 at a time; `written` of `total` are committed.
    ImportingEpisodes {
        written: u64,
        total: u64,
    },
    Done,
    /// What stopped it. The batches it committed before stay.
    Failed {
        error: String,
    },
}

impl DatasetImportPhase {
    /// Whether the run has ended, either way.
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. })
    }
    /// What the run is doing, as a status line says it.
    pub fn label(&self) -> String {
        match self {
            Self::ListingFiles => "Reading the repository".to_owned(),
            Self::ImportingEpisodes { written, total } => {
                format!("Imported {written} of {total} episodes")
            }
            Self::Done => "Imported".to_owned(),
            Self::Failed { error } => error.clone(),
        }
    }
}

/// One import run as a poll reads it: what it imports, at the revision it
/// resolved, and how far it has got.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatasetImportStatus {
    id: String,
    request: DatasetImportRequest,
    phase: DatasetImportPhase,
}

impl DatasetImportStatus {
    /// Run `id` importing `request`, its revision a resolved commit.
    pub fn new(id: String, request: DatasetImportRequest, phase: DatasetImportPhase) -> Self {
        Self { id, request, phase }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn request(&self) -> &DatasetImportRequest {
        &self.request
    }
    pub fn phase(&self) -> &DatasetImportPhase {
        &self.phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_names_its_phase_and_a_failure_carries_its_words() {
        let status = DatasetImportStatus::new(
            "dataset-import-1".into(),
            DatasetImportRequest::new(
                DatasetFormat::LeRobot,
                HuggingFaceDatasetSource::new("pick", "lerobot/svla_so101_pickplace")
                    .with_revision("f641879e22172be7e8161d5e6c1503c2d2feb657"),
            ),
            DatasetImportPhase::Failed {
                error: "LeRobot codebase version `v2.1` is unsupported".into(),
            },
        );
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["phase"]["phase"], "failed");
        assert_eq!(json["request"]["format"], "lerobot");
        assert_eq!(
            status.phase().label(),
            "LeRobot codebase version `v2.1` is unsupported"
        );
        assert!(status.phase().is_settled());
        let importing = DatasetImportPhase::ImportingEpisodes {
            written: 64,
            total: 2438,
        };
        assert!(!importing.is_settled());
        assert_eq!(importing.label(), "Imported 64 of 2438 episodes");
        assert_eq!(
            serde_json::from_value::<DatasetImportStatus>(json).unwrap(),
            status
        );
    }
}
