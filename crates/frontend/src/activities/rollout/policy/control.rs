//! The Policy control source's state: the checkpoints Studio could run, and
//! the operator's choice for the next run. These signals only preserve the
//! choice while the operator moves around Studio.

use dioxus::prelude::*;

use super::checkpoints::CheckpointChoice;
use crate::activities::training::history::TrainingHistory;

const DEFAULT_DURATION_SECONDS: u32 = 30;

#[derive(Clone, Copy, PartialEq)]
pub(in crate::activities::rollout) struct PolicyControl {
    draft: PolicyDraft,
    training: TrainingHistory,
}

/// The next run as the operator is choosing it.
#[derive(Clone, Copy, PartialEq)]
struct PolicyDraft {
    checkpoint: Signal<Option<String>>,
    duration: Signal<String>,
}

impl PolicyControl {
    /// What the operator hears when they ask a checkpoint to drive the arm.
    pub(in crate::activities::rollout) const UNSUPPORTED: &str =
        "Running a policy on a robot isn't supported yet.";
    /// The longest run the duration field accepts.
    pub(in crate::activities::rollout) const MAX_DURATION_SECONDS: u32 = 600;

    pub(in crate::activities::rollout) fn use_control(is_active: ReadSignal<bool>) -> Self {
        let training = TrainingHistory::use_history(is_active, || None);
        use_hook(|| Self {
            draft: PolicyDraft {
                checkpoint: Signal::new(None),
                duration: Signal::new(DEFAULT_DURATION_SECONDS.to_string()),
            },
            training,
        })
    }

    /// Every checkpoint a policy can run from, newest step first.
    pub(super) fn checkpoints(self) -> Vec<CheckpointChoice> {
        CheckpointChoice::listed(
            self.training.snapshot().read().runs(),
            &self.training.checkpoints().read(),
        )
    }

    /// The chosen checkpoint, or the newest one until the operator chooses.
    pub(in crate::activities::rollout) fn selected(self) -> Option<CheckpointChoice> {
        let mut choices = self.checkpoints();
        let chosen = self.draft.checkpoint.read().clone();
        let index = chosen
            .and_then(|key| choices.iter().position(|choice| choice.key() == key))
            .unwrap_or(0);
        (index < choices.len()).then(|| choices.swap_remove(index))
    }

    pub(super) fn select_checkpoint(mut self, key: Option<String>) {
        self.draft.checkpoint.set(key);
    }

    pub(super) fn duration_draft(self) -> String {
        self.draft.duration.read().to_owned()
    }

    pub(in crate::activities::rollout) fn duration_seconds(self) -> Option<u32> {
        self.draft.duration.read().bounded_duration()
    }

    pub(super) fn set_duration_draft(mut self, draft: String) {
        self.draft.duration.set(draft);
    }
}

trait RolloutDurationDraft {
    fn bounded_duration(&self) -> Option<u32>;
}

impl RolloutDurationDraft for str {
    fn bounded_duration(&self) -> Option<u32> {
        self.parse::<u32>()
            .ok()
            .filter(|seconds| (1..=PolicyControl::MAX_DURATION_SECONDS).contains(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::RolloutDurationDraft;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn duration_draft_must_be_a_bounded_whole_number() {
        assert_eq!("30".bounded_duration(), Some(30));
        assert_eq!("".bounded_duration(), None);
        assert_eq!("0".bounded_duration(), None);
        assert_eq!("1.5".bounded_duration(), None);
        assert_eq!("-1".bounded_duration(), None);
        assert_eq!("601".bounded_duration(), None);
    }
}
