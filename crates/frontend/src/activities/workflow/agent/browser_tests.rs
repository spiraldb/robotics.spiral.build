//! Exercise the real browser loop and fetch transport with a scripted provider.
use super::*;
use crate::client::{
    project::{ProjectClient, ProjectSessionState},
    queries::QueryRefresh,
};
use spiral_robotics_common::project::ProjectSessionInfo;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let originalFetch, calls, mode, releaseResponse, aborted;
export function installWorkflowFetch(nextMode) {
    originalFetch = globalThis.fetch;
    calls = []; mode = nextMode; aborted = false;
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const url = new URL(request.url).pathname;
        const body = request.method === 'GET' ? null : await request.clone().json();
        calls.push({url, body});
        if (url === '/api/models/capabilities') return Response.json({columns:[],rows:[],truncated:false,elapsed_ms:0});
        if (url === '/api/workflows/plan') {
            const graph = {schema_version:1,root:0,nodes:[{id:0,operator:'Values',inputs:[],output:{},properties:{}}]};
            return Response.json({report:{explain_schema_version:2,logical_plan:graph,optimized_logical_plan:graph,physical_plan:graph,physical_lowering_error:null},error:null});
        }
        if (url !== '/api/models/complete') throw new Error('Unexpected request ' + url);
        if (mode === 'pending') {
            await new Promise((resolve, reject) => {
                releaseResponse = resolve;
                request.signal.addEventListener('abort', () => { aborted = true; reject(new DOMException('Cancelled', 'AbortError')); });
            });
        }
        const first = calls.filter(call => call.url === '/api/models/complete').length === 1;
        const graph = {name:'Authored in browser',sql:'SELECT 42 AS answer'};
        const message = first
            ? {role:'assistant',content:'I’ll build it.',tool_calls:[{id:'graph-call',type:'function',function:{name:'set_workflow',arguments:JSON.stringify({workflow:graph})}}]}
            : {role:'assistant',content:'Ready to run.'};
        const payload = new TextEncoder().encode(JSON.stringify({event:'text',data:message.content})+'\n'+JSON.stringify({event:'complete',data:message})+'\n');
        // Deliberately split inside the UTF-8 apostrophe and across JSON records.
        const stream = new ReadableStream({start(controller) {
            for(let offset=0; offset<payload.length; offset+=7) controller.enqueue(payload.slice(offset,offset+7));
            controller.close();
        }});
        return new Response(stream, {headers:{'Content-Type':'application/x-ndjson'}});
    };
}
export function restoreWorkflowFetch() { globalThis.fetch = originalFetch; }
export function workflowFetchCalls() { return JSON.stringify(calls); }
export function releaseWorkflowResponse() { mode='ready'; releaseResponse(); }
export function workflowFetchAborted() { return aborted; }
"#)]
extern "C" {
    fn installWorkflowFetch(mode: &str);
    fn restoreWorkflowFetch();
    fn workflowFetchCalls() -> String;
    fn releaseWorkflowResponse();
    fn workflowFetchAborted() -> bool;
}
struct FetchFixture;
impl FetchFixture {
    fn new(mode: &str) -> Self {
        installWorkflowFetch(mode);
        Self
    }
    fn calls(&self) -> Vec<Value> {
        serde_json::from_str(&workflowFetchCalls()).unwrap()
    }
}
impl Drop for FetchFixture {
    fn drop(&mut self) {
        restoreWorkflowFetch();
    }
}

#[derive(Clone, Default)]
struct AgentHarness(Rc<RefCell<Option<(WorkflowController, WorkflowAssistant)>>>);
impl AgentHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let session = ProjectSessionInfo::new("test-session", "test-project");
        let current = session.clone();
        let state = use_signal(move || ProjectSessionState::Unavailable(Some(current)));
        use_context_provider(|| ProjectClient::new(session, state.into(), refresh));
        let controller = WorkflowController::use_controller();
        let assistant = use_hook(|| WorkflowAssistant::new(controller.spec().id()));
        *self.0.borrow_mut() = Some((controller, assistant));
        rsx! {}
    }
    fn editor(&self) -> WorkflowController {
        self.0.borrow().unwrap().0
    }
    fn assistant(&self) -> WorkflowAssistant {
        self.0.borrow().unwrap().1
    }
    fn start(&self, dom: &VirtualDom) {
        dom.in_scope(ScopeId::ROOT, || {
            self.assistant()
                .send(self.editor(), "Build a SQL workflow".into())
        });
    }
    async fn pump(&self, dom: &mut VirtualDom) {
        dom.render_immediate_to_vec();
        gloo_timers::future::TimeoutFuture::new(5).await;
        dom.render_immediate_to_vec();
    }
    async fn finished(&self, dom: &mut VirtualDom) {
        for _ in 0..200 {
            self.pump(dom).await;
            if self.assistant().task.peek().is_none() {
                return;
            }
        }
        panic!("assistant did not finish");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn browser_loop_plans_and_updates_the_local_draft_without_applying_or_saving_it() {
    let fetch = FetchFixture::new("ready");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    harness.finished(&mut dom).await;
    assert_eq!(harness.editor().spec().name(), "Authored in browser");
    let conversation = harness.assistant().conversation();
    assert_eq!(
        conversation.session().phase(),
        WorkflowAgentPhase::Idle,
        "{:?}",
        conversation.session().error()
    );
    assert!(
        conversation
            .provider_messages()
            .is_structurally_valid(false)
    );
    assert_eq!(
        conversation.session().messages().last().unwrap().text(),
        "Ready to run."
    );
    let calls = fetch.calls();
    assert_eq!(
        calls
            .iter()
            .map(|call| call["url"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "/api/models/capabilities",
            "/api/models/complete",
            "/api/workflows/plan",
            "/api/models/complete"
        ]
    );
    assert!(
        calls[3]["body"]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Authored in browser")
    );
    assert!(
        calls[3]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "graph-call")
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn cancelling_a_browser_turn_aborts_fetch_and_keeps_valid_history() {
    let fetch = FetchFixture::new("pending");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    for _ in 0..200 {
        if fetch
            .calls()
            .iter()
            .any(|call| call["url"] == "/api/models/complete")
        {
            break;
        }
        harness.pump(&mut dom).await;
    }
    assert!(!fetch.calls().is_empty(), "model request was dispatched");
    dom.in_scope(ScopeId::ROOT, || harness.assistant().cancel());
    harness.pump(&mut dom).await;
    assert!(workflowFetchAborted());
    assert_eq!(
        harness.assistant().conversation().session().phase(),
        WorkflowAgentPhase::Cancelled
    );
    assert!(
        harness
            .assistant()
            .conversation()
            .provider_messages()
            .is_structurally_valid(false)
    );
    assert!(harness.editor().spec().sql().is_empty());
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn a_user_edit_while_the_model_responds_prevents_a_stale_tool_from_overwriting_it() {
    let fetch = FetchFixture::new("pending");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    for _ in 0..200 {
        if fetch
            .calls()
            .iter()
            .any(|call| call["url"] == "/api/models/complete")
        {
            break;
        }
        harness.pump(&mut dom).await;
    }
    assert!(!fetch.calls().is_empty(), "model request was dispatched");
    dom.in_scope(ScopeId::ROOT, || {
        harness.editor().set_name("User edited this".into())
    });
    releaseWorkflowResponse();
    harness.finished(&mut dom).await;
    assert_eq!(harness.editor().spec().name(), "User edited this");
    assert_eq!(
        fetch.calls().len(),
        2,
        "stale tools must not even start schema planning"
    );
    assert_eq!(
        harness.assistant().conversation().session().phase(),
        WorkflowAgentPhase::Failed
    );
    assert!(
        harness
            .assistant()
            .conversation()
            .provider_messages()
            .is_structurally_valid(false)
    );
}
