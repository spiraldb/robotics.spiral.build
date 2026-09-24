//! Where a pass writes into the curation: one edit and one save per
//! episode, so a reload keeps every episode already annotated.
use super::pipeline::AnnotationRecorder;
use crate::activities::curation::document::SubtaskRow;
use crate::activities::curation::state::{CurationController, RunHandle};
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::datasets::EpisodeId;
use std::cell::Cell;
use std::rc::Rc;

/// The curation as one pass writes to it.
#[derive(Clone)]
pub(super) struct CurationJournal {
    node: String,
    controller: CurationController,
    /// Reports save failures through the project's notifications.
    studio: StudioProjectState,
    /// The run as the curation holds it: whether the operator still wants it
    /// — Stop clears that, after which the pass starts no further episode but
    /// finishes the ones in flight — and where it says how far it has got.
    run: RunHandle,
    progress: PassProgress,
}

/// How many of one pass's episodes have been written, of how many.
#[derive(Clone)]
struct PassProgress {
    written: Rc<Cell<usize>>,
    total: usize,
}

impl PassProgress {
    fn line(&self) -> String {
        format!(
            "Annotating {} of {} episodes",
            self.written.get(),
            self.total
        )
    }
}

impl CurationJournal {
    /// The journal of one run that will write `total` episodes.
    pub(super) fn new(
        controller: CurationController,
        studio: StudioProjectState,
        run: RunHandle,
        total: usize,
    ) -> Self {
        let progress = PassProgress {
            written: Rc::new(Cell::new(0)),
            total,
        };
        run.report(progress.line());
        Self {
            node: run.node().to_owned(),
            controller,
            studio,
            run,
            progress,
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
        self.progress.written.set(self.progress.written.get() + 1);
        self.run.report(self.progress.line());
    }

    async fn persist(&self) {
        self.controller.save_when_idle(self.studio).await;
    }

    fn running(&self) -> bool {
        self.run.wanted()
    }
}
