//! Durable browser-side choices for the Rollout screen.
//!
//! The backend will own activation, inference, and motion. These signals only
//! preserve the operator's checkpoint and execution draft while they move
//! between Studio activities.

use dioxus::prelude::*;
use spiral_robotics_api::rollout::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Copy)]
pub(crate) struct RolloutController {
    checkpoint: Signal<Option<String>>,
    duration_draft: Signal<String>,
}

impl RolloutController {
    pub(crate) fn use_controller() -> Self {
        Self {
            checkpoint: use_signal(|| None),
            duration_draft: use_signal(|| DEFAULT_ROLLOUT_DURATION_SECONDS.to_string()),
        }
    }

    pub(crate) fn checkpoint(&self) -> Option<String> {
        self.checkpoint.read().to_owned()
    }

    pub(crate) fn select_checkpoint(mut self, checkpoint: Option<String>) {
        self.checkpoint.set(checkpoint);
    }

    pub(crate) fn duration_draft(&self) -> String {
        self.duration_draft.read().to_owned()
    }

    pub(crate) fn duration_seconds(&self) -> Option<u32> {
        self.duration_draft.read().bounded_duration()
    }

    pub(crate) fn set_duration_draft(mut self, draft: String) {
        self.duration_draft.set(draft);
    }
}

trait RolloutDurationDraft {
    fn bounded_duration(&self) -> Option<u32>;
}

impl RolloutDurationDraft for str {
    fn bounded_duration(&self) -> Option<u32> {
        self.parse::<u32>()
            .ok()
            .filter(|seconds| (1..=MAX_ROLLOUT_DURATION_SECONDS).contains(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::RolloutDurationDraft;

    #[test]
    fn duration_draft_must_be_a_bounded_whole_number() {
        assert_eq!("30".bounded_duration(), Some(30));
        assert_eq!("".bounded_duration(), None);
        assert_eq!("0".bounded_duration(), None);
        assert_eq!("1.5".bounded_duration(), None);
        assert_eq!("-1".bounded_duration(), None);
        assert_eq!("601".bounded_duration(), None);
    }
}
