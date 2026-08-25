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

pub(crate) fn use_rollout_controller() -> RolloutController {
    let controller = RolloutController {
        checkpoint: use_signal(|| None),
        duration_draft: use_signal(|| DEFAULT_ROLLOUT_DURATION_SECONDS.to_string()),
    };
    use_context_provider(|| controller)
}

pub(crate) fn rollout_controller() -> RolloutController {
    use_context::<RolloutController>()
}

impl RolloutController {
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
        parse_duration(&self.duration_draft.read())
    }

    pub(crate) fn set_duration_draft(mut self, draft: String) {
        self.duration_draft.set(draft);
    }
}

fn parse_duration(draft: &str) -> Option<u32> {
    draft
        .parse::<u32>()
        .ok()
        .filter(|seconds| (1..=MAX_ROLLOUT_DURATION_SECONDS).contains(seconds))
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn duration_draft_must_be_a_bounded_whole_number() {
        assert_eq!(parse_duration("30"), Some(30));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("1.5"), None);
        assert_eq!(parse_duration("-1"), None);
        assert_eq!(parse_duration("601"), None);
    }
}
