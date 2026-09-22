//! Drive one whole pass in the browser: frames come back from a mocked
//! episode query, are composed into sheets on a real canvas, become image
//! content parts of one chat completion per episode, and the answers reach the
//! Build document and its save endpoint. Only the engine is mocked; frame
//! composition, prompt construction, provider transport, validation, and
//! persistence are the real ones.
use super::episodes::AnnotationEpisode;
use super::journal::BuildJournal;
use super::native::NativeSheets;
use super::pipeline::{AnnotationPass, AnnotationRecorder, ContactSheetSource, PassSettings};
use super::provider::ProviderAnnotator;
use super::sheet::ContactSheet;
use crate::activities::data::DatasetClient;
use crate::activities::workflow::build::client::BuildClient;
use crate::activities::workflow::build::document::{
    BUILD_ENCODING_VERSION, BuildNode, BuildNodeKind, DATASET_NODE_ID, SubtaskAnnotation,
    SubtaskRow,
};
use crate::activities::workflow::build::state::BuildController;
use crate::client::{
    model::ModelClient,
    project::{ProjectClient, ProjectSessionState},
    queries::QueryRefresh,
};
use dioxus::prelude::*;
use serde_json::Value;
use spiral_robotics_common::build::BuildDocument;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::project::ProjectSessionInfo;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let previousFetch, requests, completions;
export function installAnnotationFetch() {
    previousFetch = globalThis.fetch;
    requests = [];
    completions = 0;
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const url = new URL(request.url);
        const path = url.pathname;
        if (path === '/api/credentials') {
            return Response.json({llm:{base_url:'https://openrouter.ai/api/v1',api_key:'test-key',default_model:'openai/gpt-5.6-terra'},spiral:null,hugging_face:null});
        }
        const body = await request.json();
        requests.push({path, upstream: url.searchParams.get('url'), body});
        if (path === '/api/proxy') {
            completions += 1;
            const start = completions === 1 ? 0 : 4;
            const answer = JSON.stringify({subtasks:[{start_time:start, end_time:start + 3, description:'grasp the cloth'}]});
            const frames =
                'data: ' + JSON.stringify({choices:[{delta:{content:answer}}]}) + '\n\n' +
                'data: ' + JSON.stringify({choices:[{delta:{}, finish_reason:'stop'}]}) + '\n\n' +
                'data: [DONE]\n\n';
            return new Response(frames, {status:200, headers:{'content-type':'text/event-stream'}});
        }
        if (path === '/api/build-documents/save') {
            return Response.json({...body, revision: body.revision + 1});
        }
        if (path === '/api/queries/episode') {
            const statement = body.statement;
            if (statement.includes('count(*) AS packets')) {
                return Response.json({
                    columns: [
                        {name:'packets', data_type:'i64', nullable:false},
                        {name:'first_ns', data_type:'u64', nullable:true},
                        {name:'last_ns', data_type:'u64', nullable:true},
                    ],
                    rows: [{packets: 600, first_ns: 0, last_ns: 20000000000}],
                    truncated: false,
                    elapsed_ms: 1,
                });
            }
            const selection = statement.match(/frame_indices\(\[([^\]]*)\]\)/);
            if (!selection) { throw new Error('Unexpected episode query ' + statement); }
            const indices = selection[1].split(', ');
            return Response.json({
                columns: [
                    {name:'request_index', data_type:'i64', nullable:false},
                    {name:'time_ns', data_type:'i64', nullable:true},
                    {name:'image', data_type:'Image', nullable:true, semantic_type:'spql.image.Image'},
                ],
                rows: indices.map((_, index) => ({
                    request_index: index,
                    time_ns: index * 500000000,
                    image: 'data:image/jpeg;base64,/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYyLjI4LjEwMgD/2wBDAAgoKC8oLzc3Nzc3N0E8QUNDQ0FBQUFDQ0NISEhVVVVISEhDQ0hIUFBVVVxfXFdXVVdfX2RkZHh4c3OMjJGsrM//xABNAAEBAAAAAAAAAAAAAAAAAAAABgEBAQEAAAAAAAAAAAAAAAAAAAYHEAEAAAAAAAAAAAAAAAAAAAAAEQEAAAAAAAAAAAAAAAAAAAAA/8AAEQgAEgAgAwEiAAIRAAMRAP/aAAwDAQACEQMRAD8AiwEm38AAAAAB/9k=',
                })),
                truncated: false,
                elapsed_ms: 1,
            });
        }
        throw new Error('Unexpected request ' + path);
    };
}
export function restoreAnnotationFetch() { globalThis.fetch = previousFetch; }
export function annotationRequests() { return JSON.stringify(requests); }
"#)]
extern "C" {
    fn installAnnotationFetch();
    fn restoreAnnotationFetch();
    fn annotationRequests() -> String;
}

struct AnnotationFetch;
impl AnnotationFetch {
    fn install() -> Self {
        installAnnotationFetch();
        Self
    }
    fn requests(&self) -> Vec<Value> {
        serde_json::from_str(&annotationRequests()).expect("the mock records JSON requests")
    }
}
impl Drop for AnnotationFetch {
    fn drop(&mut self) {
        restoreAnnotationFetch();
    }
}

/// The controller's own edits, saved through the Build client. The workspace's
/// journal saves through the controller, which needs the project's
/// notification centre; the document that reaches the backend is the same.
#[derive(Clone)]
struct TestJournal {
    node: String,
    controller: BuildController,
    documents: BuildClient,
}
impl AnnotationRecorder for TestJournal {
    fn record(&self, episode: &str, outcome: Result<Vec<SubtaskRow>, String>) {
        BuildJournal::write(self.controller, &self.node, episode, outcome);
    }
    async fn persist(&self) {
        let payload = self
            .controller
            .with_payload(|payload| payload.encode())
            .expect("the annotated payload encodes");
        let document = BuildDocument::new(
            self.controller.id(),
            self.controller.name(),
            BUILD_ENCODING_VERSION,
            payload,
        );
        self.documents
            .save(&document)
            .await
            .expect("the save endpoint accepts the document");
    }
    fn running(&self) -> bool {
        true
    }
}

#[derive(Clone, Default)]
struct AnnotationHarness {
    controller: Rc<RefCell<Option<BuildController>>>,
    clients: Rc<RefCell<Option<(ModelClient, BuildClient, DatasetClient)>>>,
    finished: Rc<RefCell<bool>>,
}

impl AnnotationHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let session = ProjectSessionInfo::new("annotate-session", "test-project");
        let current = session.clone();
        let state = use_signal(move || ProjectSessionState::Unavailable(Some(current)));
        use_context_provider(|| ProjectClient::new(session, state.into(), refresh));
        *self.controller.borrow_mut() = Some(BuildController::use_controller());
        *self.clients.borrow_mut() = Some((
            ModelClient::current(),
            BuildClient::current(),
            DatasetClient::current(),
        ));
        rsx! {}
    }
    fn controller(&self) -> BuildController {
        self.controller.borrow().expect("the harness rendered")
    }
    /// A graph over one dataset with one Break down subtasks node.
    fn annotation_node(&self, dom: &VirtualDom) -> String {
        dom.in_scope(ScopeId::ROOT, || {
            let controller = self.controller();
            controller.create(
                "Folding",
                Some(DatasetSelection::new(vec!["folding".into()]).expect("a dataset name")),
            );
            controller
                .edit(|payload| {
                    payload.add_child(
                        DATASET_NODE_ID,
                        BuildNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
                    )
                })
                .expect("the dataset node accepts an annotation child")
        })
    }
    fn start(&self, dom: &VirtualDom, node: String) {
        let (model, documents, datasets) =
            self.clients.borrow().clone().expect("the harness rendered");
        let controller = self.controller();
        let finished = self.finished.clone();
        dom.in_scope(ScopeId::ROOT, || {
            spawn(async move {
                AnnotationPass::new(
                    PassSettings::new(node.clone(), "google/gemini-2.5-flash", 0.5),
                    NativeSheets::new(datasets, "/top-camera", 0.5),
                    ProviderAnnotator::new(model),
                    TestJournal {
                        node,
                        controller,
                        documents,
                    },
                )
                .run(vec![
                    AnnotationEpisode::fixture("ep-1", 20_000_000_000),
                    AnnotationEpisode::fixture("ep-2", 20_000_000_000),
                ])
                .await;
                *finished.borrow_mut() = true;
            });
        });
    }
    async fn finish(&self, dom: &mut VirtualDom) {
        for _ in 0..400 {
            dom.render_immediate_to_vec();
            if *self.finished.borrow() {
                return;
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("the browser annotation pass did not finish");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn annotate_sends_sheets_as_images_and_persists_each_episode_before_the_next() {
    let fetch = AnnotationFetch::install();
    let harness = AnnotationHarness::default();
    let mut dom = VirtualDom::new_with_props(AnnotationHarness::render, harness.clone());
    dom.rebuild_in_place();
    let node = harness.annotation_node(&dom);
    harness.start(&dom, node.clone());
    harness.finish(&mut dom).await;

    let controller = harness.controller();
    let rows = controller.with_payload(|payload| {
        payload
            .subtasks_for(&node)
            .map(|row| {
                (
                    row.episode().to_owned(),
                    row.start_time(),
                    row.end_time(),
                    row.description().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        rows,
        [
            ("ep-1".to_owned(), 0.0, 3.0, "grasp the cloth".to_owned()),
            ("ep-2".to_owned(), 4.0, 7.0, "grasp the cloth".to_owned()),
        ]
    );
    let completed =
        controller.with_payload(|payload| match payload.node(&node).map(BuildNode::kind) {
            Some(BuildNodeKind::BreakDownSubtasks(annotation)) => (
                annotation.completed_episodes().to_vec(),
                annotation.failed_episodes().len(),
            ),
            _ => panic!("the graph keeps its annotation node"),
        });
    assert_eq!(completed, (vec!["ep-1".to_owned(), "ep-2".to_owned()], 0));

    let requests = fetch.requests();
    let completions = requests
        .iter()
        .filter(|request| request["path"] == "/api/proxy")
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 2);
    assert_eq!(
        completions[0]["upstream"],
        "https://openrouter.ai/api/v1/chat/completions"
    );
    let body = &completions[0]["body"];
    assert_eq!(body["model"], "google/gemini-2.5-flash");
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body["tools"].is_null());
    assert!(body["tool_choice"].is_null());
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "episode_subtasks"
    );
    let messages = body["messages"].as_array().expect("messages are a list");
    // One system prompt, one instruction, then one image message per sheet.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert!(
        messages[1]["content"]
            .as_str()
            .expect("the instruction is text")
            .contains("A sheet edge is not an action boundary.")
    );
    assert_eq!(messages[2]["content"][0]["type"], "text");
    assert!(
        messages[2]["content"][0]["text"]
            .as_str()
            .expect("a sheet caption")
            .starts_with("Contact sheet 1 of 2")
    );
    // The sheet is a picture this browser drew and encoded, not a passed-through
    // frame: the tiles, their timestamps, and the JPEG are all the real ones.
    assert!(
        messages[3]["content"][1]["image_url"]["url"]
            .as_str()
            .expect("a sheet image")
            .starts_with("data:image/jpeg;base64,")
    );

    let saves = requests
        .iter()
        .filter(|request| request["path"] == "/api/build-documents/save")
        .collect::<Vec<_>>();
    assert_eq!(saves.len(), 2);
    // The first save carries only the first episode: a reload keeps what has
    // finished rather than waiting for the whole dataset.
    let first: Value = serde_json::from_str(
        saves[0]["body"]["payload"]
            .as_str()
            .expect("the payload is JSON text"),
    )
    .expect("the payload decodes");
    assert_eq!(first["subtasks"].as_array().expect("rows").len(), 1);
    assert_eq!(first["subtasks"][0]["episode"], "ep-1");
    let last: Value = serde_json::from_str(
        saves[1]["body"]["payload"]
            .as_str()
            .expect("the payload is JSON text"),
    )
    .expect("the payload decodes");
    assert_eq!(last["subtasks"].as_array().expect("rows").len(), 2);
    assert_eq!(last["nodes"][1]["kind"]["completed_episodes"][1], "ep-2");
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn native_frames_are_composed_into_sheets_in_the_browser() {
    let fetch = AnnotationFetch::install();
    let harness = AnnotationHarness::default();
    let mut dom = VirtualDom::new_with_props(AnnotationHarness::render, harness.clone());
    dom.rebuild_in_place();
    let (_, _, datasets) = harness
        .clients
        .borrow()
        .clone()
        .expect("the harness rendered");
    let sheets = Rc::new(RefCell::new(None::<Result<Vec<ContactSheet>, String>>));
    let written = sheets.clone();
    dom.in_scope(ScopeId::ROOT, || {
        spawn(async move {
            // Twelve seconds at half a second: twenty-four frames, so the last
            // four spill onto a second sheet.
            let composed = NativeSheets::new(datasets, "/top-camera", 0.5)
                .sheets(&AnnotationEpisode::fixture("ep-1", 12_000_000_000))
                .await;
            *written.borrow_mut() = Some(composed);
        });
    });
    for _ in 0..400 {
        dom.render_immediate_to_vec();
        if sheets.borrow().is_some() {
            break;
        }
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
    let composed = sheets
        .borrow_mut()
        .take()
        .expect("the native source finished")
        .expect("the mocked engine decoded every sampled frame");
    assert_eq!(composed.len(), 2);
    assert_eq!(
        composed[0].caption(0, 2),
        "Contact sheet 1 of 2, covering 0.0 s to 9.5 s of this episode."
    );
    assert_eq!(
        composed[1].caption(1, 2),
        "Contact sheet 2 of 2, covering 10.0 s to 11.5 s of this episode."
    );
    for sheet in &composed {
        assert!(sheet.image().starts_with("data:image/jpeg;base64,"));
        assert!(sheet.bytes() > 512);
    }

    // One cheap timing query, then one query that decodes the sampled frames.
    let queries = fetch
        .requests()
        .into_iter()
        .filter(|request| request["path"] == "/api/queries/episode")
        .map(|request| {
            request["body"]["statement"]
                .as_str()
                .expect("a statement")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 2);
    assert!(queries[0].contains("count(*) AS packets"));
    assert!(queries[1].contains("spql.video.decode(f.frame) AS image"));
    assert!(queries[1].contains("frame_indices([0, 15, 30, 45,"));
}
