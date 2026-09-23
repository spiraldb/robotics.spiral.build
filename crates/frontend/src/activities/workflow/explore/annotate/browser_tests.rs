//! Drive one whole pass in the browser: frames come back from a mocked
//! episode query, are composed into sheets on a real canvas, become image
//! content parts of one chat completion per episode, and the answers reach the
//! exploration and its save endpoint. Only the engine is mocked; frame
//! composition, prompt construction, provider transport, validation, and
//! persistence are the real ones.
use super::episodes::AnnotationEpisode;
use super::journal::ExplorationJournal;
use super::native::NativeSheets;
use super::pipeline::{
    AnnotationPass, AnnotationRecorder, ContactSheetSource, PassSettings, SampledEpisode,
};
use super::provider::ProviderAnnotator;
use crate::activities::data::DatasetClient;
use crate::activities::workflow::explore::document::{
    DATASET_NODE_ID, EXPLORATION_ENCODING_VERSION, ExplorationNode, ExplorationNodeKind,
    SubtaskAnnotation, SubtaskRow,
};
use crate::activities::workflow::explore::poster::{
    PosterCut, PosterFrame, PosterFrames, ShownPoster,
};
use crate::activities::workflow::explore::review::{EpisodeOrder, SubtaskContext};
use crate::activities::workflow::explore::state::ExplorationController;
use crate::client::documents::DocumentClient;
use crate::client::{project::ProjectClient, queries::QueryRefresh};
use crate::components::document_session::StudioDocument as _;
use dioxus::prelude::*;
use serde_json::Value;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::exploration::ExplorationDocument;
use spiral_robotics_common::project::ProjectKey;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

/// The saving half of the exploration document, which the workspace reaches
/// through its controller and this test reaches directly.
type ExplorationDocuments = DocumentClient<ExplorationDocument>;

#[wasm_bindgen(inline_js = r#"
const SAMPLE_FRAME = 'data:image/jpeg;base64,/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYyLjI4LjEwMgD/2wBDAAgoKC8oLzc3Nzc3N0E8QUNDQ0FBQUFDQ0NISEhVVVVISEhDQ0hIUFBVVVxfXFdXVVdfX2RkZHh4c3OMjJGsrM//xABNAAEBAAAAAAAAAAAAAAAAAAAABgEBAQEAAAAAAAAAAAAAAAAAAAYHEAEAAAAAAAAAAAAAAAAAAAAAEQEAAAAAAAAAAAAAAAAAAAAA/8AAEQgAEgAgAwEiAAIRAAMRAP/aAAwDAQACEQMRAD8AiwEm38AAAAAB/9k=';
let previousFetch, requests, pendingCompletions, maximumCompletionsInFlight;
export function installAnnotationFetch() {
    previousFetch = globalThis.fetch;
    requests = [];
    pendingCompletions = [];
    maximumCompletionsInFlight = 0;
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const url = new URL(request.url);
        const path = url.pathname;
        if (path === '/api/providers') {
            return Response.json({llm:{base_url:'https://openrouter.ai/api/v1',default_model:'openai/gpt-5.6-terra'}});
        }
        const body = await request.json();
        requests.push({path, upstream: url.searchParams.get('url'), body});
        if (path === '/api/proxy') {
            await new Promise(resolve => {
                pendingCompletions.push(resolve);
                maximumCompletionsInFlight = Math.max(
                    maximumCompletionsInFlight,
                    pendingCompletions.length,
                );
                if (pendingCompletions.length === 2) {
                    for (const release of pendingCompletions.splice(0)) release();
                }
            });
            const instruction = body.messages[1].content;
            const start = instruction.includes('20 seconds') ? 0 : 4;
            const answer = JSON.stringify({subtasks:[{start_time:start, end_time:start + 3, description:'grasp the cloth'}]});
            const frames =
                'data: ' + JSON.stringify({choices:[{delta:{content:answer}}]}) + '\n\n' +
                'data: ' + JSON.stringify({choices:[{delta:{}, finish_reason:'stop'}]}) + '\n\n' +
                'data: [DONE]\n\n';
            return new Response(frames, {status:200, headers:{'content-type':'text/event-stream'}});
        }
        if (path === '/api/explorations/save') {
            return Response.json({...body, revision: body.revision + 1});
        }
        if (path === '/api/queries/episode') {
            const statement = body.statement;
            if (statement.includes('spql.video.frame_slice()')) {
                // One frame in the middle of every window the poster query asks for.
                const windows = [...statement.matchAll(/BETWEEN (-?\d+) AND (-?\d+)/g)];
                return Response.json({
                    columns: [
                        {name:'request_index', data_type:'i64', nullable:false},
                        {name:'time_ns', data_type:'i64', nullable:true},
                        {name:'image', data_type:'Image', nullable:true, semantic_type:'spql.image.Image'},
                    ],
                    rows: windows.map(([, low, high], index) => ({
                        request_index: index,
                        time_ns: (Number(low) + Number(high)) / 2,
                        image: SAMPLE_FRAME,
                    })),
                    truncated: false,
                    elapsed_ms: 1,
                });
            }
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
                    image: SAMPLE_FRAME,
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
export function maximumAnnotationRequestsInFlight() { return maximumCompletionsInFlight; }
"#)]
extern "C" {
    fn installAnnotationFetch();
    fn restoreAnnotationFetch();
    fn annotationRequests() -> String;
    fn maximumAnnotationRequestsInFlight() -> u32;
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

/// The controller's own edits, saved through the exploration client. The
/// workspace's journal saves through the controller, which needs the project's
/// notification centre; the document that reaches the backend is the same.
#[derive(Clone)]
struct TestJournal {
    node: String,
    controller: ExplorationController,
    documents: ExplorationDocuments,
}
impl AnnotationRecorder for TestJournal {
    fn record(&self, episode: &EpisodeId, outcome: Result<Vec<SubtaskRow>, String>) {
        ExplorationJournal::write(self.controller, &self.node, episode, outcome);
    }
    async fn persist(&self) {
        let payload = self
            .controller
            .with_payload(|payload| payload.encode())
            .expect("the annotated payload encodes");
        let document = ExplorationDocument::new(
            self.controller.id(),
            self.controller.name(),
            EXPLORATION_ENCODING_VERSION,
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
    controller: Rc<RefCell<Option<ExplorationController>>>,
    clients: Rc<RefCell<Option<(ExplorationDocuments, DatasetClient)>>>,
    finished: Rc<RefCell<bool>>,
}

impl AnnotationHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let project = ProjectKey::new("https://api.spiraldb.dev", "test-project");
        use_context_provider(|| ProjectClient::new(project, refresh));
        *self.controller.borrow_mut() = Some(ExplorationController::use_controller());
        *self.clients.borrow_mut() = Some((
            DocumentClient::current(ExplorationDocument::COLLECTION),
            DatasetClient::current(),
        ));
        rsx! {}
    }
    fn controller(&self) -> ExplorationController {
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
                        ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
                    )
                })
                .expect("the dataset node accepts an annotation child")
        })
    }
    fn start(&self, dom: &VirtualDom, node: String) {
        let (documents, datasets) = self.clients.borrow().clone().expect("the harness rendered");
        let controller = self.controller();
        let finished = self.finished.clone();
        dom.in_scope(ScopeId::ROOT, || {
            spawn(async move {
                AnnotationPass::new(
                    PassSettings::new(node.clone(), "google/gemini-2.5-flash", 0.5),
                    NativeSheets::new(datasets, "/top-camera", 0.5),
                    ProviderAnnotator,
                    TestJournal {
                        node,
                        controller,
                        documents,
                    },
                )
                .run(vec![
                    AnnotationEpisode::fixture(1, 20_000_000_000),
                    AnnotationEpisode::fixture(2, 21_000_000_000),
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
async fn annotate_sends_parallel_image_requests_and_persists_in_episode_order() {
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
                    row.episode().clone(),
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
            (
                EpisodeId::fixture(1),
                0.0,
                3.0,
                "grasp the cloth".to_owned()
            ),
            (
                EpisodeId::fixture(2),
                4.0,
                7.0,
                "grasp the cloth".to_owned()
            ),
        ]
    );
    // Every row carries the still it will be shown as, cut in this browser
    // from a frame the pass had already decoded for its contact sheets.
    let posters = controller.with_payload(|payload| {
        payload
            .subtasks_for(&node)
            .map(|row| row.poster().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert_eq!(posters.len(), 2);
    for poster in &posters {
        let poster = poster
            .as_deref()
            .expect("an annotated row carries a poster");
        assert!(poster.starts_with("data:image/jpeg;base64,"));
        assert!(poster.len() > 512, "a poster is a real picture");
    }
    // And nothing was asked of the engine beyond the sampling itself: one
    // timing query and one frame query per episode, no poster query at all.
    let frame_queries = fetch
        .requests()
        .into_iter()
        .filter(|request| request["path"] == "/api/queries/episode")
        .count();
    assert_eq!(frame_queries, 4);

    let completed =
        controller.with_payload(
            |payload| match payload.node(&node).map(ExplorationNode::kind) {
                Some(ExplorationNodeKind::BreakDownSubtasks(annotation)) => (
                    annotation.completed_episodes().to_vec(),
                    annotation.failed_episodes().len(),
                ),
                _ => panic!("the graph keeps its annotation node"),
            },
        );
    assert_eq!(
        completed,
        (vec![EpisodeId::fixture(1), EpisodeId::fixture(2)], 0)
    );

    let requests = fetch.requests();
    let completions = requests
        .iter()
        .filter(|request| request["path"] == "/api/proxy")
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 2);
    assert_eq!(maximumAnnotationRequestsInFlight(), 2);
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
        .filter(|request| request["path"] == "/api/explorations/save")
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
    assert_eq!(
        first["subtasks"][0]["episode"],
        EpisodeId::fixture(1).as_str()
    );
    let last: Value = serde_json::from_str(
        saves[1]["body"]["payload"]
            .as_str()
            .expect("the payload is JSON text"),
    )
    .expect("the payload decodes");
    assert_eq!(last["subtasks"].as_array().expect("rows").len(), 2);
    assert_eq!(
        last["nodes"][1]["kind"]["completed_episodes"][1],
        EpisodeId::fixture(2).as_str()
    );
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn native_frames_are_composed_into_sheets_in_the_browser() {
    let fetch = AnnotationFetch::install();
    let harness = AnnotationHarness::default();
    let mut dom = VirtualDom::new_with_props(AnnotationHarness::render, harness.clone());
    dom.rebuild_in_place();
    let (_, datasets) = harness
        .clients
        .borrow()
        .clone()
        .expect("the harness rendered");
    let sheets = Rc::new(RefCell::new(None::<Result<SampledEpisode, String>>));
    let written = sheets.clone();
    dom.in_scope(ScopeId::ROOT, || {
        spawn(async move {
            // Twelve seconds at half a second: twenty-four frames, so the last
            // four spill onto a second sheet.
            let composed = NativeSheets::new(datasets, "/top-camera", 0.5)
                .sheets(&AnnotationEpisode::fixture(1, 12_000_000_000))
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
    let sampled = sheets
        .borrow_mut()
        .take()
        .expect("the native source finished")
        .expect("the mocked engine decoded every sampled frame");
    let composed = sampled.sheets();
    assert_eq!(composed.len(), 2);
    assert_eq!(
        composed[0].caption(0, 2),
        "Contact sheet 1 of 2, covering 0.0 s to 9.5 s of this episode."
    );
    assert_eq!(
        composed[1].caption(1, 2),
        "Contact sheet 2 of 2, covering 10.0 s to 11.5 s of this episode."
    );
    for sheet in composed {
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

/// The statements one run sent to the episode query endpoint.
fn episode_queries(fetch: &AnnotationFetch) -> Vec<String> {
    fetch
        .requests()
        .into_iter()
        .filter(|request| request["path"] == "/api/queries/episode")
        .map(|request| request["body"]["statement"].as_str().unwrap().to_owned())
        .collect()
}

/// One index over two episodes: one measured with its posters, one measured
/// before posters were stored.
#[derive(Clone, Default)]
struct PosterHarness {
    rows: Rc<RefCell<Vec<SubtaskRow>>>,
    frames: Rc<RefCell<Option<PosterFrames>>>,
    shown: Rc<RefCell<Vec<ShownPoster>>>,
}

impl PosterHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let project = ProjectKey::new("https://api.spiraldb.dev", "test-project");
        use_context_provider(|| ProjectClient::new(project, refresh));
        // The controller provides the exploration's one poster cache.
        ExplorationController::use_controller();
        let rows = self.rows.borrow().clone();
        let context = SubtaskContext::new(
            Some(DatasetSelection::new(vec!["folding".into()]).expect("a dataset name")),
            Some("/top-camera".to_owned()),
            EpisodeOrder::of(rows.iter().map(SubtaskRow::episode)),
        );
        let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
        *self.frames.borrow_mut() = Some(frames);
        *self.shown.borrow_mut() = rows.iter().map(|row| frames.shown(row)).collect();
        rsx! {
            ol {
                for row in rows {
                    li { key: "{row.episode()}-{row.start_time()}",
                        PosterFrame { row: row.clone(), frames }
                    }
                }
            }
        }
    }
    fn frames(&self) -> PosterFrames {
        self.frames.borrow().expect("the index mounted")
    }
    fn picture(&self, row: usize) -> Option<String> {
        self.shown.borrow()[row].picture().map(str::to_owned)
    }
    async fn settle(&self, dom: &mut VirtualDom, row: usize) -> String {
        for _ in 0..200 {
            dom.render_immediate_to_vec();
            if let Some(picture) = self.picture(row) {
                return picture;
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("the backfilled poster never arrived");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn a_row_annotated_with_a_poster_shows_it_without_any_frame_query() {
    let fetch = AnnotationFetch::install();
    let stored = "data:image/jpeg;base64,AAAA".to_owned();
    let harness = PosterHarness::default();
    let (measured, older) = (EpisodeId::fixture(1), EpisodeId::fixture(2));
    *harness.rows.borrow_mut() = vec![
        SubtaskRow::new("break_down_subtasks-1", measured.clone(), 0.0, 4.0, "grasp")
            .unwrap()
            .with_poster(stored.clone()),
        SubtaskRow::new("break_down_subtasks-1", older.clone(), 2.0, 6.0, "fold").unwrap(),
    ];
    let mut dom = VirtualDom::new_with_props(PosterHarness::render, harness.clone());
    dom.rebuild_in_place();
    dom.render_immediate_to_vec();

    // The measured row shows the picture it was annotated with; the older one
    // has none yet.
    assert_eq!(harness.picture(0), Some(stored));
    assert_eq!(harness.picture(1), None);

    // Scrolling the measured episode into view asks for nothing at all: its
    // rows brought their pictures with them.
    let frames = harness.frames();
    dom.in_scope(ScopeId::ROOT, || frames.request(&measured));
    for _ in 0..5 {
        dom.render_immediate_to_vec();
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
    assert!(episode_queries(&fetch).is_empty());

    // The older episode is backfilled in one query, selected by recorded time:
    // nothing estimates the frame rate first, and the picture it produces is
    // encoded exactly as an annotated one would have been.
    dom.in_scope(ScopeId::ROOT, || frames.request(&older));
    let backfilled = harness.settle(&mut dom, 1).await;
    assert!(backfilled.starts_with("data:image/jpeg;base64,"));
    let queries = episode_queries(&fetch);
    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("spql.video.frame_slice()"));
    assert!(queries[0].contains("BETWEEN 3980000000 AND 4020000000"));
    assert!(!queries[0].contains("count(*)"));

    // Asking again — another view of the same exploration — is free.
    dom.in_scope(ScopeId::ROOT, || frames.request(&older));
    for _ in 0..5 {
        dom.render_immediate_to_vec();
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
    assert_eq!(episode_queries(&fetch).len(), 1);
}
