//! Exercise the browser loop, native tool transport, and validation repair together.
use super::*;
use crate::client::{
    project::{ProjectClient, ProjectSessionState},
    queries::QueryRefresh,
};
use dioxus::prelude::*;
use spiral_robotics_common::{
    datasets::{
        HuggingFaceDatasetFile, HuggingFaceInspectionContents, HuggingFaceInspectionSource,
    },
    project::ProjectSessionInfo,
};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let previousFetch, robotRequests, completionCount, saveCount;
export function installRobotDiscoveryFetch() {
    previousFetch = globalThis.fetch;
    robotRequests = []; completionCount = 0; saveCount = 0;
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const path = new URL(request.url).pathname;
        const body = await request.clone().json();
        robotRequests.push({path, session: request.headers.get('X-Project-Session'), body});
        if (path === '/api/datasets/robot-discovery/context') return Response.json({
            episode:{dataset:{path:['dataset_demo']},episode_id:'episode-1'},
            topics:[{name:'/joints',schemas:['sensor_msgs/JointState'],members:1,samples:3,revision:null}]
        });
        if (path === '/api/datasets/robot-discovery/read-description') {
            return Response.json('<robot name="demo"><link name="base"/></robot>');
        }
        if (path === '/api/datasets/robot-discovery/inspect-stream') {
            return Response.json({schema:[{name:'position'}],samples:[{value:{position:[0.25]}}]});
        }
        if (path === '/api/datasets/robot-discovery/save') {
            saveCount += 1;
            if (saveCount === 1) return Response.json(
                {code:'external',message:'Bindings must match the model independent joints'},
                {status:502}
            );
            return Response.json(null);
        }
        if (path !== '/api/models/complete') throw new Error('Unexpected request ' + path);
        completionCount += 1;
        const calls = [
            ['read-description','read_description',{url:'https://huggingface.co/datasets/acme/demo/resolve/pinned-sha/README.md'}],
            ['inspect-stream','inspect_stream',{topic:'/joints',schema:'sensor_msgs/JointState'}],
            ['first-finish','finish',{robot:{urdf:'<robot name="demo"><link name="base"/></robot>',bindings:[],meshes:{}}}],
            ['repaired-finish','finish',{robot:{urdf:'<robot name="demo"><link name="base"/></robot>',bindings:[],meshes:{}}}]
        ];
        const [id, name, args] = calls[completionCount - 1];
        const message = {role:'assistant',content:'Inspecting the imported robot.',tool_calls:[{id,type:'function',function:{name,arguments:JSON.stringify(args)}}]};
        const payload = new TextEncoder().encode(JSON.stringify({event:'complete',data:message})+'\n');
        return new Response(new ReadableStream({start(controller) { controller.enqueue(payload); controller.close(); }}), {headers:{'Content-Type':'application/x-ndjson'}});
    };
}
export function restoreRobotDiscoveryFetch() { globalThis.fetch = previousFetch; }
export function capturedRobotDiscoveryRequests() { return JSON.stringify(robotRequests); }
"#)]
extern "C" {
    fn installRobotDiscoveryFetch();
    fn restoreRobotDiscoveryFetch();
    fn capturedRobotDiscoveryRequests() -> String;
}

struct FetchFixture;
impl FetchFixture {
    fn install() -> Self {
        installRobotDiscoveryFetch();
        Self
    }
    fn requests(&self) -> Vec<Value> {
        serde_json::from_str(&capturedRobotDiscoveryRequests()).unwrap()
    }
}
impl Drop for FetchFixture {
    fn drop(&mut self) {
        restoreRobotDiscoveryFetch();
    }
}

#[derive(Clone, Default)]
struct DiscoveryHarness {
    result: Rc<RefCell<Option<Result<(), String>>>>,
    client: Rc<RefCell<Option<DatasetClient>>>,
}
impl DiscoveryHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let session = ProjectSessionInfo::new("robot-session", "test-project");
        let current = session.clone();
        let state = use_signal(move || ProjectSessionState::Unavailable(Some(current)));
        use_context_provider(|| ProjectClient::new(session, state.into(), refresh));
        *self.client.borrow_mut() = Some(DatasetClient::current());
        rsx! {}
    }
    fn start(&self, dom: &VirtualDom, inspection: HuggingFaceDatasetInspection) {
        let result = self.result.clone();
        let client = self.client.borrow().as_ref().unwrap().clone();
        dom.in_scope(ScopeId::ROOT, || {
            spawn(async move {
                let agent = RobotDiscoveryAgent::new(client);
                *result.borrow_mut() = Some(agent.discover(&inspection).await);
            });
        });
    }
    async fn finish(&self, dom: &mut VirtualDom) {
        for _ in 0..400 {
            dom.render_immediate_to_vec();
            if let Some(result) = self.result.borrow_mut().take() {
                return result.expect("browser robot discovery should repair and finish");
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("browser robot discovery did not finish");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn browser_dispatches_native_tools_and_repairs_a_rejected_result() {
    let fetch = FetchFixture::install();
    let harness = DiscoveryHarness::default();
    let mut dom = VirtualDom::new_with_props(DiscoveryHarness::render, harness.clone());
    dom.rebuild_in_place();
    let inspection = HuggingFaceDatasetInspection::new(
        HuggingFaceInspectionSource::new("demo", "acme/demo").with_revisions("main", "pinned-sha"),
        HuggingFaceInspectionContents::new(
            vec![HuggingFaceDatasetFile::new("episodes/episode.mcap", 42)],
            42,
            vec!["dataset_demo".into()],
        ),
    );
    harness.start(&dom, inspection);
    harness.finish(&mut dom).await;

    let requests = fetch.requests();
    assert!(
        requests
            .iter()
            .all(|request| request["session"] == "robot-session")
    );
    let model_requests = requests
        .iter()
        .filter(|request| request["path"] == "/api/models/complete")
        .collect::<Vec<_>>();
    assert_eq!(model_requests.len(), 4);
    assert_eq!(model_requests[0]["body"]["model"], "gpt-5.6-terra");
    assert!(
        model_requests[0]["body"]["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("pinned-sha")
    );
    assert!(
        model_requests[3]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Bindings must match")))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["path"] == "/api/datasets/robot-discovery/save")
            .count(),
        2
    );
}
