//! Where a pass writes into the exploration: one edit and one save per
//! episode, so a reload keeps every episode already annotated.
use super::pipeline::AnnotationRecorder;
use crate::activities::workflow::explore::document::SubtaskRow;
use crate::activities::workflow::explore::state::ExplorationController;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// A save is refused while the document has another command in flight, so the
/// journal waits for it rather than dropping the episode's rows.
const SAVE_WAIT_MS: u32 = 50;
const SAVE_WAIT_ROUNDS: usize = 200;

/// The exploration as one pass writes to it.
#[derive(Clone)]
pub(super) struct ExplorationJournal {
    node: String,
    controller: ExplorationController,
    /// Reports save failures through the project's notifications.
    studio: StudioProjectState,
    /// What the operator still wants: cleared by Stop, after which the pass
    /// starts no further episode but finishes the ones in flight.
    wanted: Signal<bool>,
}

impl ExplorationJournal {
    pub(super) fn new(
        node: impl Into<String>,
        controller: ExplorationController,
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

impl ExplorationJournal {
    /// Write one episode's outcome into the document.
    pub(super) fn write(
        controller: ExplorationController,
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

impl AnnotationRecorder for ExplorationJournal {
    fn record(&self, episode: &EpisodeId, outcome: Result<Vec<SubtaskRow>, String>) {
        Self::write(self.controller, &self.node, episode, outcome);
    }

    async fn persist(&self) {
        for _ in 0..SAVE_WAIT_ROUNDS {
            if self.controller.command().is_none() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(SAVE_WAIT_MS).await;
        }
        self.controller.save(self.studio);
    }

    fn running(&self) -> bool {
        *self.wanted.peek()
    }
}
