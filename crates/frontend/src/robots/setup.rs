//! Robot setup queries live only in hardware views, beneath the immutable project scope.
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::robots::RobotClient;
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use spiral_robotics_common::robots::RobotSetupSnapshot;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RobotSetupState {
    state: Signal<RobotSetupLoad>,
}

#[derive(Default)]
pub(crate) struct RobotSetupLoad {
    snapshot: RobotSetupSnapshot,
    loaded: bool,
    current: bool,
    error: Option<String>,
}

impl RobotSetupState {
    pub(super) fn use_state(active: ReadSignal<bool>, sessions: ProjectSessionSignals) -> Self {
        let state = Self::use_empty_state();
        state.use_activity(active);
        // Camera panels cross the docking and editor layers without either
        // intermediate component consuming setup. Other consumers use props.
        use_context_provider(|| state);
        let request = QueryPolicy::new(QueryDomain::RobotSetup, 2_000).use_when(
            move || active() && sessions.connected(),
            || RobotClient::current().setup_snapshot(),
        );
        state.use_response(request);
        state
    }

    fn use_activity(mut self, active: ReadSignal<bool>) {
        use_effect(move || {
            if !active() && self.state.peek().current {
                self.state.write().current = false;
            }
        });
    }

    fn use_empty_state() -> Self {
        use_hook(|| Self {
            state: Signal::new(Default::default()),
        })
    }

    #[cfg(test)]
    pub(super) fn use_ready_state() -> Self {
        let mut state = Self::use_empty_state();
        use_hook(move || {
            let mut state = state.state.write();
            state.loaded = true;
            state.current = true;
        });
        state
    }

    fn use_response(
        mut self,
        request: Resource<Result<RobotSetupSnapshot, crate::client::CommandError>>,
    ) {
        use_effect(move || {
            let response = request.read();
            if !request.finished() {
                return;
            }
            match response.as_ref() {
                Some(Ok(snapshot)) => {
                    if self.state.peek().snapshot != *snapshot
                        || !self.state.peek().loaded
                        || !self.state.peek().current
                        || self.state.peek().error.is_some()
                    {
                        *self.state.write() = RobotSetupLoad {
                            snapshot: snapshot.clone(),
                            loaded: true,
                            current: true,
                            error: None,
                        };
                    }
                }
                Some(Err(error)) => self.state.write().error = Some(error.to_string()),
                None => {}
            }
        });
    }

    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn snapshot(self) -> MappedSignal<RobotSetupSnapshot, Signal<RobotSetupLoad>> {
        self.state.map(|state| &state.snapshot)
    }
    pub(crate) fn available(self) -> bool {
        self.state.read().current && self.state.read().error.is_none()
    }
    pub(crate) fn loaded(self) -> bool {
        self.state.read().loaded
    }
    pub(crate) fn error(self) -> Option<String> {
        self.state.read().error.clone()
    }
    pub(crate) fn retry(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::RobotSetup]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct SetupHarness(Rc<RefCell<Option<SetupInputs>>>);
    #[derive(Clone, Copy)]
    struct SetupInputs {
        setup: RobotSetupState,
        phase: Signal<u8>,
        active: Signal<bool>,
    }
    impl SetupHarness {
        fn render(self) -> Element {
            let phase = use_signal(|| 0_u8);
            let active = use_signal(|| true);
            let setup = RobotSetupState::use_empty_state();
            setup.use_activity(active.into());
            let request =
                use_resource(move || {
                    let phase = phase();
                    async move {
                        match phase {
                        0 => std::future::pending().await,
                        1 => Ok(RobotSetupSnapshot::default().with_robots(vec![
                            spiral_robotics_common::robots::RobotSummary::new(
                                "follower", None, Default::default(),
                            ),
                        ])),
                        _ => Err(crate::client::CommandError::new(
                            spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                            "Robot catalog is unavailable",
                        )),
                    }
                    }
                });
            setup.use_response(request);
            *self.0.borrow_mut() = Some(SetupInputs {
                setup,
                phase,
                active,
            });
            rsx! {}
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn loading_and_errors_disable_controls_without_erasing_the_last_robot_configuration() {
        let harness = SetupHarness::default();
        let mut dom = VirtualDom::new_with_props(SetupHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let mut inputs = harness.0.borrow().unwrap();
        assert!(!inputs.setup.loaded());
        assert!(!inputs.setup.available());
        dom.in_runtime(|| inputs.phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(inputs.setup.available());
        assert_eq!(inputs.setup.snapshot().read().robots()[0].id(), "follower");
        dom.in_runtime(|| inputs.phase.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(!inputs.setup.available());
        assert!(inputs.setup.loaded());
        assert_eq!(
            inputs.setup.error().as_deref(),
            Some("Robot catalog is unavailable")
        );
        assert_eq!(inputs.setup.snapshot().read().robots()[0].id(), "follower");
        dom.in_runtime(|| inputs.phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(inputs.setup.available());
        assert!(inputs.setup.error().is_none());
        dom.in_runtime(|| inputs.active.set(false));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(!inputs.setup.available());
        dom.in_runtime(|| {
            inputs.phase.set(0);
            inputs.active.set(true);
        });
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(!inputs.setup.available());
        assert_eq!(inputs.setup.snapshot().read().robots()[0].id(), "follower");
        dom.in_runtime(|| inputs.phase.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(inputs.setup.available());
        drop(dom);
        assert!(inputs.setup.state.try_read().is_err());
    }
}
