//! Durable browser-side choices for the Rollout screen.
//!
//! The backend will own activation, inference, and motion. These signals only
//! preserve the operator's checkpoint and execution draft while they move
//! between Studio activities.

use dioxus::prelude::*;
use spiral_robotics_common::rollout::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Copy)]
pub(crate) struct RolloutController {
    snapshot: Signal<spiral_robotics_common::rollout::RolloutSnapshot>,
    checkpoint: Signal<Option<String>>,
    duration_draft: Signal<String>,
}

impl RolloutController {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn use_provider(is_active: ReadSignal<bool>) -> Self {
        let controller = Self::use_state();
        let active = use_memo(move || controller.snapshot.read().phase().active());
        let request = crate::client::queries::QueryPolicy::new(
            crate::client::queries::QueryDomain::Rollout,
            500,
        )
        .with_activity(active.into())
        .use_when(
            move || *is_active.read(),
            || super::RolloutClient::current().rollout_snapshot(),
        );
        controller.use_response(request);
        controller
    }
    fn use_state() -> Self {
        use_context_provider(|| Self {
            snapshot: Signal::new(Default::default()),
            checkpoint: Signal::new(None),
            duration_draft: Signal::new(DEFAULT_ROLLOUT_DURATION_SECONDS.to_string()),
        })
    }
    fn use_response(
        self,
        request: Resource<
            Result<spiral_robotics_common::rollout::RolloutSnapshot, crate::client::CommandError>,
        >,
    ) {
        let mut snapshot = self.snapshot;
        use_effect(move || {
            let response = request.read();
            if request.finished()
                && let Some(Ok(value)) = response.as_ref()
                && *snapshot.peek() != *value
            {
                snapshot.set(value.clone());
            }
        });
    }
    pub(crate) fn snapshot(
        self,
    ) -> MappedSignal<
        spiral_robotics_common::rollout::RolloutSnapshot,
        Signal<spiral_robotics_common::rollout::RolloutSnapshot>,
    > {
        self.snapshot.map(|value| value)
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

    use super::RolloutController;
    use dioxus::prelude::*;
    use spiral_robotics_common::rollout::RolloutSnapshot;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct ResponseHarness(Rc<RefCell<Option<ResponseObservation>>>);
    #[derive(Clone, Copy)]
    struct ResponseObservation {
        phase: Signal<u8>,
        controller: RolloutController,
    }
    impl ResponseHarness {
        fn render(self) -> Element {
            let phase = use_signal(|| 0_u8);
            let controller = RolloutController::use_state();
            let response = use_resource(move || {
                let phase = phase();
                async move {
                    match phase {
                        0 => std::future::pending().await,
                        1 => Ok(
                            RolloutSnapshot::new(Some("session".into()), Some("run".into()))
                                .with_robot(Some("follower".into())),
                        ),
                        _ => Err(crate::client::CommandError::new(
                            spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                            "unavailable",
                        )),
                    }
                }
            });
            controller.use_response(response);
            *self.0.borrow_mut() = Some(ResponseObservation { phase, controller });
            rsx! {}
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_observes_delayed_responses_and_preserves_the_last_success_on_failure() {
        let harness = ResponseHarness::default();
        let mut dom = VirtualDom::new_with_props(ResponseHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let ResponseObservation {
            mut phase,
            controller,
        } = harness.0.borrow().unwrap();
        dom.in_runtime(|| assert_eq!(*controller.snapshot().peek(), RolloutSnapshot::default()));
        dom.in_runtime(|| phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        let expected = RolloutSnapshot::new(Some("session".into()), Some("run".into()))
            .with_robot(Some("follower".into()));
        dom.in_runtime(|| assert_eq!(*controller.snapshot().peek(), expected));
        dom.in_runtime(|| phase.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        dom.in_runtime(|| assert_eq!(*controller.snapshot().peek(), expected));
    }

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
