//! Exercise the browser loop, stream evidence, and validation repair together.
use super::*;
use crate::client::{project::ProjectClient, queries::QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::{
    datasets::{
        HuggingFaceDatasetFile, HuggingFaceInspectionContents, HuggingFaceInspectionSource,
    },
    project::ProjectKey,
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
        if (path === '/api/credentials') return Response.json({llm:{base_url:'https://provider.test/v1',api_key:'test-key',default_model:'openai/gpt-5.6-terra'},spiral:null,hugging_face:{token:'hf-test'}});
        let metadata = null;
        if (path === '/api/proxy') {
            metadata = {url:new URL(request.url).searchParams.get('url'),method:request.method,authorization:request.headers.get('authorization')};
            if (metadata.url.includes('huggingface.co/')) {
                robotRequests.push({path:'/api/proxy',project:request.headers.get('X-Spiral-Project'),metadata,body:null});
                return new Response('<robot name="demo"><link name="base"/></robot>', {headers:{'Content-Type':'text/plain'}});
            }
        }
        const body = await request.clone().json();
        const recordedPath = path === '/api/proxy' ? metadata.url : path;
        robotRequests.push({path:recordedPath, transport:path, project: request.headers.get('X-Spiral-Project'), metadata, body});
        if (path === '/api/queries') return Response.json({
            columns:[{name:'id',data_type:'Utf8',nullable:false}],rows:[{id:'episode-1'}],truncated:false,elapsed_ms:0
        });
        if (path === '/api/datasets/episode-topics') return Response.json([
            {name:'/joints',schemas:['sensor_msgs/JointState'],members:1,samples:3,revision:{variant:'joints'}}
        ]);
        if (path === '/api/queries/episode') return Response.json({
            columns:[{name:'value',data_type:'Struct',nullable:false}],rows:[{value:{position:[0.25]}}],truncated:false,elapsed_ms:0
        });
        if (path === '/api/datasets/robot/save') {
            saveCount += 1;
            return Response.json(null);
        }
        if (path !== '/api/proxy') throw new Error('Unexpected request ' + path);
        completionCount += 1;
        const urdf = '<robot name="demo"><link name="base"><visual><geometry><box size="0.1 0.1 0.1"/></geometry></visual></link></robot>';
        const calls = [
            ['read-description','read_description',{url:'https://huggingface.co/datasets/acme/demo/resolve/pinned-sha/README.md'}],
            ['inspect-stream','inspect_stream',{topic:'/joints',schema:'sensor_msgs/JointState'}],
            ['first-finish','finish',{robot:{urdf,bindings:[{joint:'elbow',topic:'/joints',schema:'sensor_msgs/JointState',field:['position'],selector:{kind:'index',index:0},scale:1,offset:0}],meshes:{}}}],
            ['repaired-finish','finish',{robot:{urdf,bindings:[],meshes:{}}}]
        ];
        const [id, name, args] = calls[completionCount - 1];
        const message = {role:'assistant',content:'Inspecting the imported robot.',tool_calls:[{id,type:'function',function:{name,arguments:JSON.stringify(args)}}]};
        const tool_calls = message.tool_calls.map((call,index) => ({...call,index}));
        const payload = new TextEncoder().encode('data: '+JSON.stringify({choices:[{delta:{content:message.content,tool_calls},finish_reason:'tool_calls'}]})+'\n\ndata: [DONE]\n\n');
        return new Response(new ReadableStream({start(controller) { controller.enqueue(payload); controller.close(); }}), {headers:{'Content-Type':'text/event-stream'}});
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
        let project = ProjectKey::new("https://api.spiraldb.dev", "test-project");
        use_context_provider(|| ProjectClient::new(project, refresh));
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
    assert!(requests.iter().all(|request| {
        if request["transport"] == "/api/proxy" || request["path"] == "/api/proxy" {
            request["project"].is_null()
        } else {
            request["project"] == "test-project"
        }
    }));
    let description = requests
        .iter()
        .find(|request| request["path"] == "/api/proxy")
        .unwrap();
    assert_eq!(description["metadata"]["method"], "GET");
    assert!(
        description["metadata"]["url"]
            .as_str()
            .unwrap()
            .contains("pinned-sha")
    );
    let model_requests = requests
        .iter()
        .filter(|request| request["path"] == "https://provider.test/v1/chat/completions")
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
    let samples = requests
        .iter()
        .find(|request| request["path"] == "/api/queries/episode")
        .unwrap();
    assert_eq!(samples["body"]["episode"]["episode_id"], "episode-1");
    assert!(
        samples["body"]["statement"]
            .as_str()
            .unwrap()
            .contains("spql.mcap.read_topic('dataset_demo', '/joints', 'joints')")
    );
    let saves = requests
        .iter()
        .filter(|request| request["path"] == "/api/datasets/robot/save")
        .collect::<Vec<_>>();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["body"]["dataset"]["path"], json!(["dataset_demo"]));
    assert_eq!(saves[0]["body"]["robot"]["bindings"], json!([]));
}
