//! Robot setup and hardware observations owned by a mounted hardware view.
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::{project::ProjectSessionSignals, state::StudioProjectState};
use dioxus::prelude::*;
use spiral_robotics_common::recording::{RecordingProgress, ReplayProgress};
use spiral_robotics_common::robots::{ModeSelection, RobotSummary, RobotTelemetry, TelemetryFrame};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct HardwareViewState {
    setup: super::RobotSetupState,
    telemetry: Signal<TelemetryFrame>,
    last_telemetry_ms: Signal<f64>,
    clock_ms: Signal<f64>,
    connected: Memo<bool>,
    catalog_available: Memo<bool>,
    is_active: ReadSignal<bool>,
}
impl HardwareViewState {
    pub(crate) fn use_state(
        project: StudioProjectState,
        is_active: ReadSignal<bool>,
        sessions: ProjectSessionSignals,
    ) -> Self {
        let catalog_available = use_memo(move || sessions.catalog_available());
        let setup = super::RobotSetupState::use_state(is_active, sessions);
        let telemetry = use_hook(|| Self {
            setup,
            telemetry: Signal::new(TelemetryFrame::default()),
            last_telemetry_ms: Signal::new(0.0),
            clock_ms: project.clock_ms(),
            connected: project.live(),
            catalog_available,
            is_active,
        });
        telemetry.use_hardware_polling(is_active);
        telemetry
    }
    pub(crate) fn setup(self) -> super::RobotSetupState {
        self.setup
    }
    pub(crate) fn snapshot(
        self,
    ) -> MappedSignal<crate::robots::RobotSetupSnapshot, Signal<super::setup::RobotSetupLoad>> {
        self.setup.snapshot()
    }
    pub(crate) fn connected_robots(&self) -> Vec<RobotSummary> {
        self.setup
            .snapshot()
            .read()
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .cloned()
            .collect()
    }
    pub(crate) fn mode(&self) -> ModeSelection {
        self.setup.snapshot().read().mode().selection().to_owned()
    }
    pub(crate) fn mode_active(&self) -> bool {
        self.setup.snapshot().read().mode().active()
    }
    pub(crate) fn is_manual_target(&self, id: &str) -> bool {
        self.mode_active() && matches!(self.mode(), ModeSelection::Manual { robot } if robot == id)
    }
    pub(crate) fn is_active(self) -> bool {
        (self.is_active)()
    }
    pub(crate) fn is_active_signal(self) -> ReadSignal<bool> {
        self.is_active
    }
    pub(crate) fn frame(self) -> MappedSignal<TelemetryFrame, Signal<TelemetryFrame>> {
        self.telemetry.map(|frame| frame)
    }
    fn use_hardware_polling(self, is_active: ReadSignal<bool>) {
        let telemetry = QueryPolicy::new(QueryDomain::Telemetry, 50).use_when(
            move || *is_active.read(),
            || super::RobotClient::current().telemetry_snapshot(),
        );
        self.use_response(telemetry);
    }
    fn use_response(
        mut self,
        telemetry: Resource<Result<TelemetryFrame, crate::client::CommandError>>,
    ) {
        let is_active = self.is_active;
        // Reactivation must receive a fresh sample before enabling controls.
        use_effect(move || {
            if !is_active() {
                self.last_telemetry_ms.set(0.0);
            }
        });
        use_drop(move || {
            self.last_telemetry_ms.set(0.0);
        });
        use_effect(move || {
            let result = telemetry.read();
            if telemetry.finished()
                && let Some(Ok(frame)) = result.as_ref()
                && self.telemetry.peek().sequence() != frame.sequence()
            {
                self.last_telemetry_ms.set(js_sys::Date::now());
                self.telemetry.set(frame.clone());
            }
        });
    }
    pub(crate) fn robot_telemetry(&self, id: &str) -> Option<RobotTelemetry> {
        if !self.controls_available() {
            return None;
        }
        self.telemetry
            .read()
            .robots()
            .iter()
            .find(|robot| robot.id() == id)
            .cloned()
    }
    pub(crate) fn controls_available(&self) -> bool {
        let age = self.telemetry_age_seconds();
        (self.is_active)()
            && (self.connected)()
            && (self.catalog_available)()
            && self.setup.available()
            && age.is_finite()
            && age < 2.5
    }
    pub(crate) fn telemetry_age_seconds(&self) -> f64 {
        let last = (self.last_telemetry_ms)();
        if last <= 0.0 {
            return f64::INFINITY;
        }
        (((self.clock_ms)() - last) / 1000.0).max(0.0)
    }

    pub(crate) fn recording(self) -> Option<RecordingProgress> {
        self.telemetry.read().recording().cloned()
    }
    pub(crate) fn replay(self) -> Option<ReplayProgress> {
        self.telemetry.read().replay().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct TelemetryHarness(Rc<RefCell<Option<TelemetryInputs>>>);
    #[derive(Clone, Copy)]
    struct TelemetryInputs {
        telemetry: HardwareViewState,
        sequence: Signal<u64>,
        active: Signal<bool>,
        connected: Signal<bool>,
        catalog: Signal<bool>,
    }
    impl TelemetryHarness {
        fn render(self) -> Element {
            let sequence = use_signal(|| 0);
            let active = use_signal(|| true);
            let connected = use_signal(|| true);
            let catalog = use_signal(|| true);
            let connected_memo = use_memo(move || *connected.read());
            let catalog_memo = use_memo(move || *catalog.read());
            let setup = super::super::RobotSetupState::use_ready_state();
            let telemetry = use_hook(|| HardwareViewState {
                setup,
                telemetry: Signal::new(TelemetryFrame::default()),
                last_telemetry_ms: Signal::new(0.0),
                clock_ms: Signal::new(js_sys::Date::now()),
                connected: connected_memo,
                catalog_available: catalog_memo,
                is_active: active.into(),
            });
            let request = use_resource(move || {
                let sequence = sequence();
                async move { Ok(TelemetryFrame::new(sequence, 60.0, 60.0)) }
            });
            telemetry.use_response(request);
            *self.0.borrow_mut() = Some(TelemetryInputs {
                telemetry,
                sequence,
                active,
                connected,
                catalog,
            });
            rsx! {}
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn controls_require_fresh_samples_and_reactivation_requires_a_new_sample() {
        let harness = TelemetryHarness::default();
        let mut dom = VirtualDom::new_with_props(TelemetryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let mut inputs = harness.0.borrow().unwrap();
        assert!(!inputs.telemetry.controls_available());
        dom.in_runtime(|| inputs.sequence.set(1));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(inputs.telemetry.controls_available());

        let received = *inputs.telemetry.last_telemetry_ms.peek();
        dom.in_runtime(|| inputs.telemetry.clock_ms.set(received + 2_500.0));
        assert!(!inputs.telemetry.controls_available());
        dom.in_runtime(|| inputs.telemetry.clock_ms.set(received));
        assert!(inputs.telemetry.controls_available());

        dom.in_runtime(|| inputs.active.set(false));
        dom.render_immediate_to_vec();
        assert!(!inputs.telemetry.controls_available());
        assert_eq!(inputs.telemetry.frame().peek().sequence(), 1);
        dom.in_runtime(|| inputs.active.set(true));
        dom.render_immediate_to_vec();
        assert!(!inputs.telemetry.controls_available());
        dom.in_runtime(|| inputs.sequence.set(2));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert!(inputs.telemetry.controls_available());

        dom.in_runtime(|| inputs.connected.set(false));
        dom.render_immediate_to_vec();
        assert!(!inputs.telemetry.controls_available());
        dom.in_runtime(|| {
            inputs.connected.set(true);
            inputs.catalog.set(false);
        });
        dom.render_immediate_to_vec();
        assert!(!inputs.telemetry.controls_available());
        dom.in_runtime(|| inputs.catalog.set(true));
        dom.render_immediate_to_vec();
        assert!(inputs.telemetry.controls_available());
        drop(dom);
        assert!(inputs.telemetry.frame().try_read().is_err());
    }
}
