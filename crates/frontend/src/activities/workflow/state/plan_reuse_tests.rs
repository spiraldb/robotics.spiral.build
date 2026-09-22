use super::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Default)]
struct ControllerHarness {
    controller: Rc<RefCell<Option<WorkflowController>>>,
}
impl ControllerHarness {
    fn render(self) -> Element {
        *self.controller.borrow_mut() = Some(WorkflowController::use_controller());
        rsx! {}
    }

    fn controller(&self) -> WorkflowController {
        self.controller.borrow().unwrap()
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn reopening_and_discard_reuse_saved_plans_without_a_backend_request() {
    let harness = ControllerHarness::default();
    let mut dom = VirtualDom::new_with_props(ControllerHarness::render, harness.clone());
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
