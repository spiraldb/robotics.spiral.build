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
let originalFetch, calls, mode, releaseResponse, aborted, runs;
export function installWorkflowFetch(nextMode) {
    originalFetch = globalThis.fetch;
    calls = []; mode = nextMode; aborted = false; runs = [];
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
        if (url === '/api/workflow-runs' && request.method === 'POST') {
            const run = {workflow:body,snapshot:{workflow_id:body.id,workflow_revision:body.revision,run_id:'run-'+(runs.length+1),phase:'running',started_ms:runs.length+1,updated_ms:runs.length+1}};
            runs.push(run);
            return Response.json(run);
        }
        if (url === '/api/workflow-runs') return Response.json(runs);
        if (url.startsWith('/api/workflow-runs/')) {
            const run = runs.find(run => url.endsWith('/'+run.snapshot.run_id));
            if (!run) throw new Error('Unknown run');
            if (mode !== 'running') {
                run.snapshot.updated_ms = 100 + run.snapshot.started_ms;
                if (run.snapshot.run_id === 'run-1' && mode !== 'missing-presentation') {
                    run.snapshot.phase = 'failed'; run.snapshot.error = 'Native evaluation failed';
                } else {
                    run.snapshot.phase = 'completed';
                    run.snapshot.result = {columns:[{name:'answer',data_type:'Int32',nullable:false},{name:'frame',data_type:'Utf8',nullable:false,semantic_type:'spql.image.Image'}],rows:[{answer:43,frame:'data:image/png;base64,aW1hZ2U='}],truncated:true,elapsed_ms:1};
                }
            }
            return Response.json(run);
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
        let message = first
            ? {role:'assistant',content:'I’ll build it.',tool_calls:[{id:'graph-call',type:'function',function:{name:'set_workflow',arguments:JSON.stringify({workflow:graph})}}]}
            : {role:'assistant',content:'Ready to run.'};
        if (mode === 'iterate' || mode === 'running' || mode === 'reject-evidence') {
            const set = sql => ['set_workflow',{workflow:{name:'Evaluated result',sql}}];
            const steps = [set('SELECT 42 AS answer'), ['run_workflow',{}], ['inspect_run',{run_id:'run-1'}],
                set('SELECT 43 AS answer'), ['run_workflow',{}], ['inspect_run',{run_id:'run-2'}],
                ['inspect_images',{run_id:'run-2',column:'frame',rows:[0]}],
                ['assess_evidence',{run_id:'run-2',verdict:'pass',reason:'The image supports the proposed method.'}],
                set('SELECT count(*) AS count FROM evidence'), ['run_workflow',{}], ['inspect_run',{run_id:'run-3'}],
                ['show_results',{run_id:'run-2',kind:'result'}]];
            if (mode === 'reject-evidence') steps.splice(7, steps.length,
                ['assess_evidence',{run_id:'run-2',verdict:'fail',reason:'The images contradict the classifier.'}],
                set('SELECT 99 AS answer'), ['run_workflow',{}],
                ['show_results',{run_id:'run-2',kind:'result'}],
                ['assess_evidence',{run_id:'run-2',verdict:'pass',reason:'Trying to continue anyway.'}],
                ['show_results',{run_id:'run-2',kind:'diagnostic'}]);
            const index = calls.filter(call => call.url === '/api/models/complete').length - 1;
            const step = steps[index];
            message = step ? {role:'assistant',content:'Evaluating evidence.',tool_calls:[{id:'step-'+index,type:'function',function:{name:step[0],arguments:JSON.stringify(step[1])}}]}
                : {role:'assistant',content:'The result is 43. I inspected the frame; the displayed rows are a truncated preview.'};
            if (mode === 'reject-evidence' && index === 6) message.tool_calls.push({id:'premature-assessment',type:'function',function:{name:'assess_evidence',arguments:JSON.stringify({run_id:'run-2',verdict:'pass',reason:'Images not received yet.'})}});
        }
        if (mode === 'missing-presentation') {
            const steps = [['run_workflow',{}], ['inspect_run',{run_id:'run-1'}]];
            const index = calls.filter(call => call.url === '/api/models/complete').length - 1;
            const step = steps[index];
            message = step ? {role:'assistant',content:'Running.',tool_calls:[{id:'step-'+index,type:'function',function:{name:step[0],arguments:JSON.stringify(step[1])}}]}
                : {role:'assistant',content:'Finished without presenting a result.'};
        }
        if (mode === 'empty-id') {
            message = first
                ? {role:'assistant',content:'Recovering runs.',tool_calls:[{id:'recover',type:'function',function:{name:'inspect_run',arguments:JSON.stringify({run_id:''})}}]}
                : {role:'assistant',content:'No admitted runs.'};
        }
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
        for _ in 0..1000 {
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
    // The graph can reuse candidate validation instead of issuing another EXPLAIN.
    let workflow = harness.editor().spec();
    assert!(
        harness
            .editor()
            .plan_query(&workflow)
            .await
            .unwrap()
            .report()
            .is_some()
    );
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

#[wasm_bindgen_test::wasm_bindgen_test]
async fn empty_run_id_recovers_the_workflows_run_list() {
    let fetch = FetchFixture::new("empty-id");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    harness.finished(&mut dom).await;
    let calls = fetch.calls();
    assert!(
        calls
            .iter()
            .any(|call| call["url"] == "/api/workflow-runs" && call["body"].is_null())
    );
    assert!(!calls.iter().any(|call| {
        call["url"]
            .as_str()
            .unwrap()
            .starts_with("/api/workflow-runs/")
    }));
    assert_eq!(
        harness.assistant().conversation().session().phase(),
        WorkflowAgentPhase::Idle
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn agent_recovers_from_execution_failure_inspects_images_and_presents_the_producing_query() {
    let fetch = FetchFixture::new("iterate");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    harness.finished(&mut dom).await;
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
    let submissions = conversation
        .session()
        .messages()
        .iter()
        .flat_map(WorkflowAgentMessage::tools)
        .filter(|tool| tool.name() == "run_workflow")
        .collect::<Vec<_>>();
    assert_eq!(submissions.len(), 3);
    assert!(
        submissions
            .iter()
            .all(|tool| tool.summary() == "Query submitted")
    );
    assert_eq!(harness.editor().spec().sql(), "SELECT 43 AS answer");
    assert_eq!(harness.editor().snapshot().run_id(), Some("run-2"));
    assert!(!harness.editor().snapshot().result_stale());
    assert_eq!(harness.editor().workflow_runs().len(), 3);
    let calls = fetch.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["url"] == "/api/workflow-runs")
            .count(),
        3
    );
    assert!(
        !calls
            .iter()
            .any(|call| call["url"] == "/api/workflows/save")
    );
    let requests = calls
        .iter()
        .filter(|call| call["url"] == "/api/models/complete")
        .collect::<Vec<_>>();
    assert!(
        requests[3]["body"]["messages"]
            .to_string()
            .contains("Native evaluation failed")
    );
    let preview: Value = serde_json::from_str(
        requests[6]["body"]["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(preview["preview_truncated"], true);
    assert_eq!(preview["total_result_rows"], Value::Null);
    assert!(
        preview["result_notice"]
            .as_str()
            .unwrap()
            .contains("total matching row count")
    );
    assert!(
        requests[6]["body"]["messages"]
            .to_string()
            .contains("preview_truncated")
    );
    assert!(
        requests[7]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["content"].is_array())
    );
    assert!(
        !requests[8]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["content"].is_array())
    );
    assert!(
        !serde_json::to_string(&conversation)
            .unwrap()
            .contains("data:image")
    );
    assert_eq!(
        conversation.session().messages().last().unwrap().text(),
        "The result is 43. I inspected the frame; the displayed rows are a truncated preview."
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn rejected_pilot_blocks_submission_and_success_presentation_in_the_real_loop() {
    let fetch = FetchFixture::new("reject-evidence");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    harness.finished(&mut dom).await;
    let calls = fetch.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["url"] == "/api/workflow-runs" && call["body"].is_object())
            .count(),
        2,
        "the attempted full query must never reach the backend"
    );
    assert_eq!(harness.editor().snapshot().run_id(), Some("run-2"));
    assert_eq!(harness.editor().spec().sql(), "SELECT 43 AS answer");
    let conversation = harness.assistant().conversation();
    assert_eq!(
        conversation.session().phase(),
        WorkflowAgentPhase::Idle,
        "{:?}",
        conversation.session().error()
    );
    let tools = conversation
        .session()
        .messages()
        .iter()
        .flat_map(WorkflowAgentMessage::tools)
        .collect::<Vec<_>>();
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool.phase() == WorkflowAgentToolPhase::Failed)
            .count(),
        5,
        "native failure, premature assessment, blocked execution, blocked success presentation, and assessment overwrite"
    );
    assert_eq!(
        tools.last().unwrap().summary(),
        "Diagnostic evidence presented"
    );
    assert!(
        conversation
            .provider_messages()
            .is_structurally_valid(false)
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn an_execution_turn_cannot_finish_with_only_a_narrative() {
    let fetch = FetchFixture::new("missing-presentation");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    dom.in_scope(ScopeId::ROOT, || {
        let editor = harness.editor();
        let expected = editor.spec();
        editor
            .replace_authored_query(&expected, expected.clone().with_sql("SELECT 43 AS answer"))
            .unwrap();
    });
    harness.start(&dom);
    harness.finished(&mut dom).await;
    let conversation = harness.assistant().conversation();
    assert_eq!(conversation.session().phase(), WorkflowAgentPhase::Failed);
    assert!(
        conversation
            .session()
            .error()
            .unwrap()
            .contains("show_results")
    );
    assert!(
        conversation
            .provider_messages()
            .iter()
            .all(|message| message.role() != "system"),
        "a turn-local completion correction must not become an instruction in later turns"
    );
    assert_eq!(
        fetch
            .calls()
            .iter()
            .filter(|call| call["url"] == "/api/models/complete")
            .count(),
        4,
        "allow exactly one corrective model response, not an unbounded completion loop"
    );
    assert!(
        conversation
            .provider_messages()
            .is_structurally_valid(false)
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn stopping_during_result_inspection_preserves_the_admitted_run() {
    let fetch = FetchFixture::new("running");
    let harness = AgentHarness::default();
    let mut dom = VirtualDom::new_with_props(AgentHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom);
    for _ in 0..200 {
        harness.pump(&mut dom).await;
        if fetch
            .calls()
            .iter()
            .any(|call| call["url"] == "/api/workflow-runs/run-1")
        {
            break;
        }
    }
    assert_eq!(harness.editor().snapshot().run_id(), Some("run-1"));
    dom.in_scope(ScopeId::ROOT, || harness.assistant().cancel());
    harness.pump(&mut dom).await;
    assert!(harness.editor().snapshot().phase().active());
    let conversation = harness.assistant().conversation();
    let inspection = conversation
        .session()
        .messages()
        .iter()
        .flat_map(WorkflowAgentMessage::tools)
        .find(|tool| tool.name() == "inspect_run")
        .unwrap();
    assert_eq!(
        inspection.summary(),
        "Assistant stopped before this action completed"
    );
    assert!(
        inspection
            .error()
            .unwrap()
            .contains("query continues independently")
    );
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
    assert_eq!(
        fetch
            .calls()
            .iter()
            .filter(|call| call["url"] == "/api/workflow-runs")
            .count(),
        1
    );
}
