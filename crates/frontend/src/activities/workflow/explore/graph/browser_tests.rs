//! Mount the Explore canvas in the browser against a scripted document API, and
//! grow the graph the way the + on a node does.
use super::*;
use crate::client::{
    project::{ProjectClient, ProjectSessionState},
    queries::QueryRefresh,
};
use serde_json::Value;
use spiral_robotics_common::project::ProjectSessionInfo;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

use super::super::document::DATASET_NODE_ID;
use super::super::state::ExplorationController;
use super::node_card::ExploreNodeCard;

#[wasm_bindgen(inline_js = r#"
let previousFetch, requests;
export function installExplorationFetch() {
    previousFetch = globalThis.fetch;
    requests = [];
    globalThis.fetch = async (input, options) => {
        const request = input instanceof Request ? input : new Request(input, options);
        const path = new URL(request.url).pathname;
        const body = request.method === 'GET' ? null : await request.clone().json();
        requests.push({path, method: request.method, body});
        if (path === '/api/explorations') {
            return Response.json([{id:'exploration-1', name:'Cloth folding exploration', revision:3}]);
        }
        if (path === '/api/explorations/save') {
            return Response.json({...body, revision: body.revision + 1});
        }
        if (path.startsWith('/api/explorations/')) {
            return Response.json({id:'exploration-1', name:'Cloth folding exploration', revision:3,
                encoding:1, payload: JSON.stringify({nodes:[{id:'dataset',kind:{type:'dataset'}}],edges:[]})});
        }
        throw new Error('Unexpected request ' + path);
    };
}
export function restoreExplorationFetch() { globalThis.fetch = previousFetch; }
export function explorationFetchRequests() { return JSON.stringify(requests); }
"#)]
extern "C" {
    fn installExplorationFetch();
    fn restoreExplorationFetch();
    fn explorationFetchRequests() -> String;
}

struct ExplorationFetch;
impl ExplorationFetch {
    fn install() -> Self {
        installExplorationFetch();
        Self
    }
    fn requests(&self) -> Vec<Value> {
        serde_json::from_str(&explorationFetchRequests()).unwrap()
    }
}
impl Drop for ExplorationFetch {
    fn drop(&mut self) {
        restoreExplorationFetch();
    }
}

#[derive(Clone, Default)]
struct CanvasHarness(Rc<RefCell<Option<ExplorationController>>>);

impl CanvasHarness {
    fn render(self) -> Element {
        let refresh = QueryRefresh::use_provider();
        let session = ProjectSessionInfo::new("exploration-session", "test-project");
        let current = session.clone();
        let state = use_signal(move || ProjectSessionState::Unavailable(Some(current)));
        use_context_provider(|| ProjectClient::new(session, state.into(), refresh));
        let controller = ExplorationController::use_controller();
        *self.0.borrow_mut() = Some(controller);
        rsx! { ExploreGraphCanvas { controller } }
    }
    fn controller(&self) -> ExplorationController {
        self.0.borrow().expect("the canvas mounted its controller")
    }
    async fn settle(&self, dom: &mut VirtualDom) {
        for _ in 0..200 {
            dom.render_immediate_to_vec();
            if self.controller().command().is_none() && !self.controller().documents().is_empty() {
                return;
            }
            gloo_timers::future::TimeoutFuture::new(5).await;
        }
        panic!("the exploration list never arrived");
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn the_canvas_lists_saved_explorations_and_grows_the_graph_from_a_nodes_menu() {
    let fetch = ExplorationFetch::install();
    let harness = CanvasHarness::default();
    let mut dom = VirtualDom::new_with_props(CanvasHarness::render, harness.clone());
    dom.rebuild_in_place();
    let controller = harness.controller();
    dom.in_scope(ScopeId::ROOT, || controller.refresh_documents());
    harness.settle(&mut dom).await;

    assert_eq!(
        fetch
            .requests()
            .iter()
            .map(|request| request["path"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["/api/explorations"]
    );
    assert_eq!(
        controller.documents()[0].name(),
        "Cloth folding exploration"
    );

    // What the + on the dataset node offers, and what choosing the first entry does.
    let dataset = controller
        .with_payload(ExploreNodeCard::all)
        .into_iter()
        .find(|card| card.id() == DATASET_NODE_ID)
        .expect("every graph starts with its dataset node");
    let menu = dataset
        .menu()
        .cloned()
        .expect("the dataset accepts children");
    assert_eq!(menu.labels(), ["Break down subtasks"]);
    let added = dom
        .in_scope(ScopeId::ROOT, || menu.choose(controller, 0))
        .expect("the dataset accepts an annotation");
    dom.render_immediate_to_vec();

    assert_eq!(added, "break_down_subtasks-1");
    assert_eq!(controller.selected_node().as_deref(), Some(added.as_str()));
    controller.with_payload(|payload| {
        assert_eq!(payload.nodes().len(), 2);
        assert_eq!(payload.parent_of(&added), Some(DATASET_NODE_ID));
        assert_eq!(payload.validate(), Ok(()));
    });
    assert!(controller.unsaved_changes());
    let cards = controller.with_payload(ExploreNodeCard::all);
    let annotation = cards.iter().find(|card| card.id() == added).unwrap();
    assert_eq!(annotation.summary(), "0 episodes · 0 subtasks");
    assert_eq!(
        annotation
            .menu()
            .expect("an annotation accepts children")
            .labels(),
        ["Find similar subtasks", "Group similar subtasks"]
    );
    // An entry the graph no longer offers changes nothing.
    assert!(
        dom.in_scope(ScopeId::ROOT, || menu.choose(controller, 7))
            .is_err()
    );
    controller.with_payload(|payload| assert_eq!(payload.nodes().len(), 2));
    assert_eq!(fetch.requests().len(), 1, "growing the graph saves nothing");
}
