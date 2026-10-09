//! The Policy control source's state: the robot connector's run as it reports it,
//! the checkpoints Studio could run, and the operator's choice for the next
//! run. The robot connector owns activation, inference, and motion; these
//! signals only preserve the choice while the operator moves around Studio.

use dioxus::prelude::*;
use spiral_robotics_common::rollout::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_DURATION_SECONDS, RolloutConfig, RolloutSnapshot,
};

use super::PolicyClient;
use super::checkpoints::CheckpointChoice;
use crate::activities::training::history::TrainingHistory;
use crate::client::CommandError;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::robot_connector::RobotConnector;

#[derive(Clone, Copy, PartialEq)]
pub(in crate::activities::rollout) struct PolicyControl {
    run: Signal<RolloutSnapshot>,
    report: Signal<RunReport>,
    draft: PolicyDraft,
    training: TrainingHistory,
}

/// Whether the robot connector has answered for its run yet, and how the
/// last ask went.
#[derive(Clone, Debug, Default, PartialEq)]
struct RunReport {
    loaded: bool,
    error: Option<String>,
}

/// The next run as the operator is choosing it.
#[derive(Clone, Copy, PartialEq)]
struct PolicyDraft {
    checkpoint: Signal<Option<String>>,
    duration: Signal<String>,
}

impl PolicyControl {
    pub(in crate::activities::rollout) fn use_control(is_active: ReadSignal<bool>) -> Self {
        let training = TrainingHistory::use_history(is_active, || None);
        let control = Self::use_state(training);
        let connector = RobotConnector::current();
        let request = QueryPolicy::new(QueryDomain::Rollout, 500).use_when(
            move || is_active() && connector.linked(),
            || async { PolicyClient::current().run().await },
        );
        control.use_response(request);
        control
    }

    fn use_state(training: TrainingHistory) -> Self {
        use_hook(|| Self {
            run: Signal::new(RolloutSnapshot::default()),
            report: Signal::new(RunReport::default()),
            draft: PolicyDraft {
                checkpoint: Signal::new(None),
                duration: Signal::new(DEFAULT_ROLLOUT_DURATION_SECONDS.to_string()),
            },
            training,
        })
    }

    fn use_response(self, request: Resource<Result<RolloutSnapshot, CommandError>>) {
        let mut control = self;
        use_effect(move || {
            let response = request.read();
            if !request.finished() {
                return;
            }
            match response.as_ref() {
                Some(Ok(run)) => {
                    if *control.run.peek() != *run {
                        control.run.set(run.clone());
                    }
                    control.report.set(RunReport {
                        loaded: true,
                        error: None,
                    });
                }
                Some(Err(error)) => control.report.write().error = Some(error.to_string()),
                None => {}
            }
        });
    }

    /// The robot connector's current or last run.
    pub(in crate::activities::rollout) fn run(
        self,
    ) -> MappedSignal<RolloutSnapshot, Signal<RolloutSnapshot>> {
        self.run.map(|run| run)
    }

    /// The session of a run that is loading, driving, or stopping.
    pub(in crate::activities::rollout) fn active_session(self) -> Option<String> {
        let run = self.run.read();
        run.phase()
            .active()
            .then(|| run.session_id().map(str::to_owned))
            .flatten()
    }

    pub(super) fn loaded(self) -> bool {
        self.report.read().loaded
    }

    pub(super) fn error(self) -> Option<String> {
        self.report.read().error.clone()
    }

    pub(super) fn retry(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Rollout]);
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

    /// The run the operator has chosen, on `follower`, if it is complete. It
    /// asks the workspace's policy runtime for actions.
    pub(in crate::activities::rollout) fn next_run(self, follower: &str) -> Option<RolloutConfig> {
        let checkpoint = self.selected()?;
        Some(
            RolloutConfig::new(
                checkpoint.run_id(),
                checkpoint.step(),
                follower,
                RobotConnector::current().policy_runtime(),
            )
            .with_duration(self.duration_seconds()?),
        )
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
    use super::{PolicyControl, RolloutDurationDraft};
    use crate::activities::training::history::TrainingHistory;
    use dioxus::prelude::*;
    use spiral_robotics_common::rollout::RolloutSnapshot;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct ResponseHarness(Rc<RefCell<Option<ResponseObservation>>>);
    #[derive(Clone, Copy, PartialEq)]
    struct ResponseObservation {
        phase: Signal<u8>,
        control: PolicyControl,
    }
    impl ResponseHarness {
        fn render(self) -> Element {
            let phase = use_signal(|| 0_u8);
            let control = PolicyControl::use_state(TrainingHistory::use_unloaded());
            let response = use_resource(move || {
                let phase = phase();
                async move {
                    match phase {
                        0 => std::future::pending().await,
                        1 => Ok(RolloutSnapshot::new(
                            Some("session".into()),
                            Some("run".into()),
                        )),
                        _ => Err(crate::client::CommandError::new(
                            spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                            "unavailable",
                        )),
                    }
                }
            });
            control.use_response(response);
            *self.0.borrow_mut() = Some(ResponseObservation { phase, control });
            rsx! {}
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_failed_poll_keeps_the_last_run_it_saw() {
        let harness = ResponseHarness::default();
        let mut dom = VirtualDom::new_with_props(ResponseHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let ResponseObservation { mut phase, control } = harness.0.borrow().unwrap();
        dom.in_runtime(|| {
            assert_eq!(*control.run().peek(), RolloutSnapshot::default());
            assert!(!control.loaded());
            assert!(control.error().is_none());
        });
        dom.in_runtime(|| phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        let expected = RolloutSnapshot::new(Some("session".into()), Some("run".into()));
        dom.in_runtime(|| {
            assert_eq!(*control.run().peek(), expected);
            assert!(control.loaded());
            assert!(control.error().is_none());
        });
        dom.in_runtime(|| phase.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert_eq!(*control.run().peek(), expected);
            assert!(control.loaded());
            assert_eq!(control.error().as_deref(), Some("unavailable"));
        });
        dom.in_runtime(|| phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            assert!(control.loaded());
            assert!(control.error().is_none());
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
