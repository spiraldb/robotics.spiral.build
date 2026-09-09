//! Durable browser-side choices for the Rollout screen.
//!
//! The backend will own activation, inference, and motion. These signals only
//! preserve the operator's checkpoint and execution draft while they move
//! between Studio activities.

use dioxus::prelude::*;
use spiral_robotics_common::rollout::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RolloutController {
    snapshot: Signal<spiral_robotics_common::rollout::RolloutSnapshot>,
    checkpoint: Signal<Option<String>>,
    loaded: Signal<bool>,
    error: Signal<Option<String>>,
    duration_draft: Signal<String>,
}

impl RolloutController {
    pub(crate) fn use_controller(is_active: ReadSignal<bool>) -> Self {
        let controller = Self::use_state();
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let active = use_memo(move || controller.snapshot.read().phase().active());
        let request = crate::client::queries::QueryPolicy::new(
            crate::client::queries::QueryDomain::Rollout,
            500,
        )
        .with_activity(active.into())
        .use_when(
            move || *is_active.read() && sessions.catalog_available(),
            || super::RolloutClient::current().rollout_snapshot(),
        );
        controller.use_response(request);
        controller
    }
    fn use_state() -> Self {
        use_hook(|| Self {
            snapshot: Signal::new(Default::default()),
            checkpoint: Signal::new(None),
            loaded: Signal::new(false),
            error: Signal::new(None),
            duration_draft: Signal::new(DEFAULT_ROLLOUT_DURATION_SECONDS.to_string()),
        })
    }
    fn use_response(
        self,
        request: Resource<
            Result<spiral_robotics_common::rollout::RolloutSnapshot, crate::client::CommandError>,
        >,
    ) {
        let mut controller = self;
        use_effect(move || {
            let response = request.read();
            if !request.finished() {
                return;
            }
            match response.as_ref() {
                Some(Ok(value)) => {
                    if *controller.snapshot.peek() != *value {
                        controller.snapshot.set(value.clone());
                    }
                    controller.loaded.set(true);
                    controller.error.set(None);
                }
                Some(Err(error)) => controller.error.set(Some(error.to_string())),
                None => {}
            }
        });
    }
    pub(crate) fn loaded(self) -> bool {
        *self.loaded.read()
    }

    pub(crate) fn error(self) -> Option<String> {
        self.error.read().clone()
    }

    pub(crate) fn retry(self) {
        crate::client::queries::QueryRefresh::current()
            .invalidate(&[crate::client::queries::QueryDomain::Rollout]);
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
    #[derive(Clone, Copy, PartialEq)]
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
        dom.in_runtime(|| {
            assert_eq!(*controller.snapshot().peek(), RolloutSnapshot::default());
            assert!(!controller.loaded());
            assert!(controller.error().is_none());
        });
        dom.in_runtime(|| phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        let expected = RolloutSnapshot::new(Some("session".into()), Some("run".into()))
            .with_robot(Some("follower".into()));
        dom.in_runtime(|| {
            assert_eq!(*controller.snapshot().peek(), expected);
            assert!(controller.loaded());
            assert!(controller.error().is_none());
        });
        dom.in_runtime(|| phase.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert_eq!(*controller.snapshot().peek(), expected);
            assert!(controller.loaded());
            assert_eq!(controller.error().as_deref(), Some("unavailable"));
        });
        dom.in_runtime(|| phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert!(controller.loaded());
            assert!(controller.error().is_none());
        });
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
