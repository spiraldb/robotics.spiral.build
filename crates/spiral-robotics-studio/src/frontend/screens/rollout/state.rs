//! Durable browser-side choices for the Rollout screen.
//!
//! The backend will own activation, inference, and motion. These signals only
//! preserve the operator's checkpoint and execution draft while they move
//! between Studio activities.

use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub(crate) struct RolloutController {
    checkpoint: Signal<Option<String>>,
    action_steps: Signal<usize>,
    control_rate_hz: Signal<u32>,
}

pub(crate) fn use_rollout_controller() -> RolloutController {
    let controller = RolloutController {
        checkpoint: use_signal(|| None),
        action_steps: use_signal(|| 20),
        control_rate_hz: use_signal(|| 50),
    };
    use_context_provider(|| controller)
}

pub(crate) fn rollout_controller() -> RolloutController {
    use_context::<RolloutController>()
}

impl RolloutController {
    pub(crate) fn checkpoint(&self) -> Option<String> {
        self.checkpoint.read().clone()
    }

    pub(crate) fn action_steps(&self) -> usize {
        *self.action_steps.read()
    }

    pub(crate) fn control_rate_hz(&self) -> u32 {
        *self.control_rate_hz.read()
    }

    pub(crate) fn select_checkpoint(mut self, checkpoint: Option<String>) {
        self.checkpoint.set(checkpoint);
    }

    pub(crate) fn set_action_steps(mut self, steps: usize) {
        self.action_steps.set(steps.clamp(1, 256));
    }

    pub(crate) fn set_control_rate(mut self, rate: u32) {
        self.control_rate_hz.set(rate.clamp(1, 100));
    }
}
