//! A LeRobot dataset imported from Hugging Face: a run the backend holds
//! while Spiral lists the repository and copies every episode, which the
//! browser polls for its phase.
use super::HuggingFaceDatasetSource;
use serde::{Deserialize, Serialize};

/// Where an import run has got, in the order it gets there.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LeRobotImportPhase {
    /// Creating the repository's Location and listing its files.
    ListingFiles,
    /// Creating the Dataset over the listed files.
    CreatingDataset,
    /// Spiral reads every episode and encodes its cameras again; the long
    /// part.
    CopyingEpisodes,
    /// Creating the table curations store model answers in.
    CreatingAnswers,
    Done,
    /// Spiral's error, as it said it; the import committed nothing.
    Failed {
        error: String,
    },
}

impl LeRobotImportPhase {
    /// Whether the run has ended, either way.
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. })
    }
    /// What the run is doing, as a status line says it.
    pub fn label(&self) -> &str {
        match self {
            Self::ListingFiles => "Listing the repository's files",
            Self::CreatingDataset => "Creating the dataset",
            Self::CopyingEpisodes => "Copying episodes and encoding their cameras",
            Self::CreatingAnswers => "Creating the answers table",
            Self::Done => "Imported",
            Self::Failed { error } => error,
        }
    }
}

/// One import run as a poll reads it: what it imports, at the revision it
/// resolved, and how far it has got.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeRobotImportStatus {
    id: String,
    source: HuggingFaceDatasetSource,
    phase: LeRobotImportPhase,
}

impl LeRobotImportStatus {
    /// Run `id` importing `source`, its revision a resolved commit.
    pub fn new(id: String, source: HuggingFaceDatasetSource, phase: LeRobotImportPhase) -> Self {
        Self { id, source, phase }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn source(&self) -> &HuggingFaceDatasetSource {
        &self.source
    }
    pub fn phase(&self) -> &LeRobotImportPhase {
        &self.phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_names_its_phase_and_a_failure_carries_spirals_words() {
        let status = LeRobotImportStatus::new(
            "lerobot-1".into(),
            HuggingFaceDatasetSource::new("pick", "lerobot/svla_so101_pickplace")
                .with_revision("f641879e22172be7e8161d5e6c1503c2d2feb657"),
            LeRobotImportPhase::Failed {
                error: "LeRobot codebase version `v2.1` is unsupported".into(),
            },
        );
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["phase"]["phase"], "failed");
        assert_eq!(
            status.phase().label(),
            "LeRobot codebase version `v2.1` is unsupported"
        );
        assert!(status.phase().is_settled());
        assert!(!LeRobotImportPhase::CopyingEpisodes.is_settled());
        assert_eq!(
            serde_json::from_value::<LeRobotImportStatus>(json).unwrap(),
            status
        );
    }
}
