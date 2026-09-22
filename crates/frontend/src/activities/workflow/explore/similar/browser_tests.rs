//! The search path in the browser: the saved rows, the Decisions request
//! built from them, and the ranking its answers produce.
use super::decisions::DecisionRun;
use super::questions::SearchQuestions;
use super::ranking::{RankedRow, SearchRanking};
use crate::activities::workflow::explore::document::{
    DATASET_NODE_ID, ExplorationNodeKind, ExplorationPayload, SubtaskAnnotation, SubtaskRow,
};
use crate::client::completions::ModelClient;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryRefresh;
use dioxus::prelude::*;
use serde_json::Value;
use spiral_robotics_common::project::ProjectKey;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let previousFetch, requests;
export function installSearchFetch() {
    previousFetch = globalThis.fetch;
    requests = [];
    // r3 is answered by nothing, so the ranking must still list it.
    const nouls = {r0: 0.21, r1: 0.94, r2: 0.55};
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const path = new URL(request.url).pathname;
        if (path === '/api/credentials') return Response.json({llm:{base_url:'https://openrouter.ai/api/v1',api_key:'test-key',default_model:'openai/gpt-5.6-terra'},spiral:null,hugging_face:null});
        if (path !== '/api/proxy') throw new Error('Unexpected request ' + path);
        const body = await request.json();
        requests.push({url:new URL(request.url).searchParams.get('url'), body});
        const answers = {};
        for (const id of Object.keys(body.questions)) {
            if (id in nouls) answers[id] = {type:'noul', noul: nouls[id]};
        }
        return Response.json({answers, usage:{input_tokens: 11}});
    };
}
export function restoreSearchFetch() { globalThis.fetch = previousFetch; }
export function searchRequests() { return JSON.stringify(requests); }
"#)]
extern "C" {
    fn installSearchFetch();
    fn restoreSearchFetch();
    fn searchRequests() -> String;
}

struct SearchFetch;
impl SearchFetch {
    fn install() -> Self {
        installSearchFetch();
        Self
    }
    fn requests(&self) -> Vec<Value> {
        serde_json::from_str(&searchRequests()).unwrap()
    }
}
impl Drop for SearchFetch {
    fn drop(&mut self) {
        restoreSearchFetch();
    }
}

/// An exploration whose annotation node has already saved four rows.
fn annotated() -> (ExplorationPayload, Vec<SubtaskRow>) {
    let mut payload = ExplorationPayload::new(None);
    let node = payload
        .add_child(
            DATASET_NODE_ID,
            ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    let rows = [
        (0.0, 2.0, "pick up the cloth"),
        (2.0, 5.0, "fold the cloth in half"),
        (5.0, 7.0, "smooth the fold flat"),
        (7.0, 9.0, "place the cloth on the stack"),
    ]
    .into_iter()
    .map(|(start, end, description)| {
        SubtaskRow::new(node.clone(), "episode-a", start, end, description).unwrap()
    })
    .collect::<Vec<_>>();
    payload
        .replace_episode_subtasks(&node, "episode-a", rows)
        .unwrap();
    let rows = payload.subtasks_for(&node).cloned().collect();
    (payload, rows)
}

#[derive(Clone, Default)]
struct SearchHarness {
    ranking: Rc<RefCell<Option<Result<SearchRanking, String>>>>,
    model: Rc<RefCell<Option<ModelClient>>>,
}
impl SearchHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let project = ProjectKey::new("https://api.spiraldb.dev", "test-project");
        let current = project.clone();
        let state = use_signal(move || Some(current));
        use_context_provider(|| ProjectClient::new(project, state.into(), refresh));
        *self.model.borrow_mut() = Some(ModelClient::current());
        rsx! {}
    }
    fn start(&self, dom: &VirtualDom, query: &str, rows: Vec<SubtaskRow>) {
        let descriptions = rows
            .iter()
            .map(|row| row.description().to_owned())
            .collect::<Vec<_>>();
        let batches = SearchQuestions::new(query, None)
            .unwrap()
            .batches(&descriptions.iter().map(String::as_str).collect::<Vec<_>>());
        let ranking = self.ranking.clone();
        let model = self.model.borrow().as_ref().unwrap().clone();
        dom.in_scope(ScopeId::ROOT, || {
            spawn(async move {
                let mut ranked = SearchRanking::of(descriptions.len());
                let result = DecisionRun::new(model, batches)
                    .run(|batch, answers| ranked.absorb(batch, &answers))
                    .await;
                *ranking.borrow_mut() = Some(result.map(|()| ranked));
            });
        });
    }
    async fn finish(&self, dom: &mut VirtualDom) -> SearchRanking {
        for _ in 0..200 {
            dom.render_immediate_to_vec();
            if let Some(ranking) = self.ranking.borrow_mut().take() {
                return ranking.expect("the browser search should succeed");
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("the browser search did not finish");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn browser_ranks_saved_subtasks_against_one_query() {
    let fetch = SearchFetch::install();
    let (_payload, rows) = annotated();
    let harness = SearchHarness::default();
    let mut dom = VirtualDom::new_with_props(SearchHarness::render, harness.clone());
    dom.rebuild_in_place();
    harness.start(&dom, "make the first fold", rows.clone());
    let ranking = harness.finish(&mut dom).await;

    assert_eq!(
        ranking
            .ranked()
            .iter()
            .map(RankedRow::row)
            .collect::<Vec<_>>(),
        [1, 2, 0, 3],
        "best first, with the unanswered row last"
    );
    assert_eq!(ranking.ranked()[0].probability(), 0.94);
    assert!(!ranking.ranked()[3].answered());
    assert_eq!(ranking.unanswered(), 1);

    let requests = fetch.requests();
    assert_eq!(requests.len(), 1, "four rows ride in one request");
    assert_eq!(
        requests[0]["url"],
        "https://openrouter.ai/api/alpha/decisions"
    );
    let body = &requests[0]["body"];
    assert_eq!(body["model"], "typesafe/jev-1.13");
    assert_eq!(body["state"]["query"], "make the first fold");
    assert_eq!(body["state"]["candidates"]["r1"], rows[1].description());
    assert_eq!(body["questions"]["r1"]["type"], "noul");
    assert_eq!(
        body["questions"]["r1"]["instructions"],
        "Does `candidates.r1` describe the same manipulation operation as `query`?"
    );
    assert_eq!(
        body["questions"]["r1"]["criteria"]["false"],
        "a different operation, a neighboring step, or only the same object or scene."
    );
}
