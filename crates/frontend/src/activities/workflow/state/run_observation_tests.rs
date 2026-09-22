use super::*;
use spiral_robotics_common::workflow::WorkflowRunPhase;
use std::{
    cell::RefCell,
    rc::Rc,
    task::{Poll, Waker},
};

#[derive(Default)]
struct PendingRunRead {
    response: RefCell<Option<Result<Vec<WorkflowRun>, crate::client::CommandError>>>,
    waker: RefCell<Option<Waker>>,
}
impl PendingRunRead {
    fn respond(&self, response: Result<Vec<WorkflowRun>, crate::client::CommandError>) {
        *self.response.borrow_mut() = Some(response);
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }
}

#[derive(Clone, Default)]
struct RunObservationHarness {
    controller: Rc<RefCell<Option<WorkflowController>>>,
    read: Rc<PendingRunRead>,
}
impl RunObservationHarness {
    fn render(self) -> Element {
        let mut controller = WorkflowController::use_controller();
        use_hook(move || {
            let workflow = controller.spec();
            controller.runs.runs.set(vec![WorkflowRun::new(
                workflow.clone(),
                WorkflowRunSnapshot::started(&workflow, "run-one").with_started_ms(1_000),
            )]);
        });
        let read = self.read.clone();
        let resource = use_resource(move || {
            let read = read.clone();
            std::future::poll_fn(move |context| {
                if let Some(response) = read.response.borrow_mut().take() {
                    Poll::Ready(response)
                } else {
                    *read.waker.borrow_mut() = Some(context.waker().clone());
                    Poll::Pending
                }
            })
        });
        controller.use_run_observations(resource);
        *self.controller.borrow_mut() = Some(controller);
        rsx! {}
    }

    fn controller(&self) -> WorkflowController {
        self.controller.borrow().unwrap()
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
fn a_pending_first_poll_delivers_the_terminal_run_without_an_unrelated_render() {
    let harness = RunObservationHarness::default();
    let mut dom = VirtualDom::new_with_props(RunObservationHarness::render, harness.clone());
    dom.rebuild_in_place();
    dom.render_immediate_to_vec();
    let controller = harness.controller();
    assert_eq!(controller.snapshot().phase(), WorkflowRunPhase::Validating);
    let mut failed = controller.snapshot();
    failed.set_phase(WorkflowRunPhase::Failed);
    failed.set_error(Some("Source SQL is invalid".into()));
    failed.set_finished_ms(1_923);
    failed.set_updated_ms(1_923);
    harness
        .read
        .respond(Ok(vec![WorkflowRun::new(controller.spec(), failed)]));
    dom.render_immediate_to_vec();
    assert_eq!(controller.snapshot().phase(), WorkflowRunPhase::Failed);
    assert_eq!(controller.snapshot().error(), Some("Source SQL is invalid"));
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn reopening_and_discard_reuse_saved_plans_without_a_backend_request() {
    let harness = RunObservationHarness::default();
    let mut dom = VirtualDom::new_with_props(RunObservationHarness::render, harness.clone());
    dom.rebuild_in_place();
    let controller = harness.controller();
    let workflow = WorkflowSpec::new("saved-plan", "Saved query").with_sql("SELECT 42");
    let graph = serde_json::json!({"schema_version":1,"root":0,"nodes":[
        {"id":0,"operator":"Values","inputs":[],"output":{},"properties":{}}
    ]});
    let plan = WorkflowPlan::from_native_json(
        &serde_json::json!({
            "explain_schema_version":2,"logical_plan":graph,"optimized_logical_plan":graph,
            "physical_plan":graph,"physical_lowering_error":null
        })
        .to_string(),
    )
    .unwrap();
    let document = WorkflowDocument::new(workflow.clone())
        .with_plan(Some(WorkflowSqlPlan::new(&workflow, plan.clone())));
    let install = |document: WorkflowDocument| {
        controller
            .session
            .install(document, |opened| controller.adopt(opened));
    };
    dom.in_scope(ScopeId::ROOT, || install(document.clone()));
    // This harness has no ProjectClient: a backend request would panic.
    assert_eq!(
        controller.plan_query(&controller.spec()).await.unwrap(),
        plan
    );
    dom.in_scope(ScopeId::ROOT, || {
        controller.set_name("Renamed query".into())
    });
    assert_eq!(
        controller.plan_query(&controller.spec()).await.unwrap(),
        plan
    );
    dom.in_scope(ScopeId::ROOT, || {
        controller
            .session
            .edit(|document| document.workflow_mut().set_sql("SELECT 99"));
        controller.remember_plan(&workflow, plan.clone());
    });
    assert!(controller.plan_for(&controller.spec()).is_none());
    dom.in_scope(ScopeId::ROOT, || install(document));
    assert_eq!(
        controller.plan_query(&controller.spec()).await.unwrap(),
        plan
    );
    dom.in_scope(ScopeId::ROOT, || install(WorkflowDocument::new(workflow)));
    assert!(controller.plan_for(&controller.spec()).is_none());
}
