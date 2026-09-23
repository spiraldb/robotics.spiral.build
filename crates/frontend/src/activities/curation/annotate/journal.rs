//! Where a pass writes into the curation: one edit and one save per
//! episode, so a reload keeps every episode already annotated.
use super::pipeline::AnnotationRecorder;
use crate::activities::curation::document::SubtaskRow;
use crate::activities::curation::state::CurationController;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// The curation as one pass writes to it.
#[derive(Clone)]
pub(super) struct CurationJournal {
    node: String,
    controller: CurationController,
    /// Reports save failures through the project's notifications.
    studio: StudioProjectState,
    /// What the operator still wants: cleared by Stop, after which the pass
    /// starts no further episode but finishes the ones in flight.
    wanted: Signal<bool>,
}

impl CurationJournal {
    pub(super) fn new(
        node: impl Into<String>,
        controller: CurationController,
        studio: StudioProjectState,
        wanted: Signal<bool>,
    ) -> Self {
        Self {
            node: node.into(),
            controller,
            studio,
            wanted,
        }
    }
}

impl CurationJournal {
    /// Write one episode's outcome into the document.
    pub(super) fn write(
        controller: CurationController,
        node: &str,
        episode: &EpisodeId,
        outcome: Result<Vec<SubtaskRow>, String>,
    ) {
        let stored = match outcome {
            Ok(rows) => {
                controller.edit(|payload| payload.replace_episode_subtasks(node, episode, rows))
            }
            Err(reason) => {
                controller.edit(|payload| payload.record_episode_failure(node, episode, reason))
            }
        };
        if let Err(refused) = stored {
            // Rows the document will not take are this episode's failure, not
            // the pass's; the operator can retry the episode on its own.
            let _ =
                controller.edit(|payload| payload.record_episode_failure(node, episode, refused));
        }
    }
}

impl AnnotationRecorder for CurationJournal {
    fn record(&self, episode: &EpisodeId, outcome: Result<Vec<SubtaskRow>, String>) {
        Self::write(self.controller, &self.node, episode, outcome);
    }

    async fn persist(&self) {
        self.controller.save_when_idle(self.studio).await;
    }

    fn running(&self) -> bool {
        *self.wanted.peek()
    }
}
