//! Exercise prompt construction, provider transport, and binding together in the browser.
use super::*;
use crate::client::{
    project::{ProjectClient, ProjectSessionState},
    queries::QueryRefresh,
};
use dioxus::prelude::*;
use spiral_robotics_common::project::ProjectSessionInfo;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let previousFetch, requests;
export function installInterpretationFetch() {
    previousFetch = globalThis.fetch;
    requests = [];
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const path = new URL(request.url).pathname;
        requests.push({path, session: request.headers.get('X-Project-Session'), body: await request.json()});
        if (path !== '/api/models/decide') throw new Error('Unexpected request ' + path);
        const choice = value => ({type:'choice', choice:value, confidence:0.99});
        return Response.json({answers:{operation:choice('duration'), comparison:choice('below'), quantity:choice('q0')}, usage:{tokens:17}});
    };
}
export function restoreInterpretationFetch() { globalThis.fetch = previousFetch; }
export function interpretationRequests() { return JSON.stringify(requests); }
"#)]
extern "C" {
    fn installInterpretationFetch();
    fn restoreInterpretationFetch();
    fn interpretationRequests() -> String;
}

struct InterpretationFetch;
impl InterpretationFetch {
    fn install() -> Self {
        installInterpretationFetch();
        Self
    }
    fn requests(&self) -> Vec<Value> {
        serde_json::from_str(&interpretationRequests()).unwrap()
    }
}
impl Drop for InterpretationFetch {
    fn drop(&mut self) {
        restoreInterpretationFetch();
    }
}

#[derive(Clone, Default)]
struct InterpretationHarness {
    result: Rc<RefCell<Option<Result<RuleProposal, String>>>>,
    model: Rc<RefCell<Option<ModelClient>>>,
}
impl InterpretationHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let session = ProjectSessionInfo::new("interpretation-session", "test-project");
        let current = session.clone();
        let state = use_signal(move || ProjectSessionState::Unavailable(Some(current)));
        use_context_provider(|| ProjectClient::new(session, state.into(), refresh));
        *self.model.borrow_mut() = Some(ModelClient::current());
        rsx! {}
    }
    fn start(&self, dom: &VirtualDom, request: InterpretationRequest) {
        let result = self.result.clone();
        let model = self.model.borrow().as_ref().unwrap().clone();
        dom.in_scope(ScopeId::ROOT, || {
            spawn(async move {
                *result.borrow_mut() = Some(RuleInterpreter::interpret(model, request).await);
            });
        });
    }
    async fn finish(&self, dom: &mut VirtualDom) -> RuleProposal {
        for _ in 0..200 {
            dom.render_immediate_to_vec();
            if let Some(result) = self.result.borrow_mut().take() {
                return result.expect("browser interpretation should succeed");
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("browser interpretation did not finish");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn browser_builds_decisions_and_binds_the_selected_quantity() {
    let fetch = InterpretationFetch::install();
    let harness = InterpretationHarness::default();
    let mut dom = VirtualDom::new_with_props(InterpretationHarness::render, harness.clone());
    dom.rebuild_in_place();
    let prompt = "Exclude episodes shorter than 1.5 minutes";
    harness.start(
        &dom,
        InterpretationRequest::new(prompt.into(), vec!["left_camera".into()]),
    );
    let proposal = harness.finish(&mut dom).await;

    assert_eq!(proposal.intent(), RuleIntent::Duration);
    assert_eq!(proposal.comparison(), Some(ConditionOp::Below));
    assert_eq!(proposal.value(), Some(90.0));
    assert!(proposal.problems().is_empty());
    let requests = fetch.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["session"], "interpretation-session");
    let body = &requests[0]["body"];
    assert_eq!(body["model"], "typesafe/jev-1.13");
    assert_eq!(body["state"]["request"], prompt);
    assert_eq!(body["state"]["quantities"][0]["text"], "1.5 minutes");
    assert_eq!(body["questions"]["camera"]["criteria"]["c0"], "left_camera");
    assert!(body["questions"]["operation"]["criteria"]["duration"].is_string());
}
