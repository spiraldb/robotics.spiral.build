use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{AdbcDatabase, ProjectStore, SpiralConnectionSettings};

#[derive(Default)]
struct CatalogState {
    reads: usize,
    version: usize,
    tokens: Vec<String>,
    deny_all: bool,
}

struct CatalogServer {
    endpoint: String,
    state: Arc<Mutex<CatalogState>>,
    task: JoinHandle<()>,
}

impl CatalogServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(CatalogState::default()));
        let app = Router::new()
            .fallback(Self::respond)
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint,
            state,
            task,
        }
    }

    async fn respond(State(state): State<Arc<Mutex<CatalogState>>>, request: Request) -> Response {
        let mut state = state.lock().unwrap();
        state.reads += 1;
        let token = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        state.tokens.push(token);
        if state.deny_all {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if request.uri().path().ends_with("/branches/main") {
            return Json(json!({"headCommitId":state.version.to_string()})).into_response();
        }
        Json(json!({"namespace":[{
            "parent":{"kind":"projectRoot"}, "name":"dataset",
            "resource":{"descriptor":{"resourceId":"resource","kind":"dataset"},"versionId":state.version.to_string()}
        }],"nextPageToken":null})).into_response()
    }

    fn store(&self, project: &str, access: crate::SpiralAccess) -> ProjectStore {
        let settings = SpiralConnectionSettings::new(self.endpoint.clone(), self.endpoint.clone())
            .unwrap()
            .with_project(project)
            .unwrap();
        let connection = crate::SpiralConnection::new(settings, access);
        let database = Arc::new(AdbcDatabase::new(connection.clone()));
        ProjectStore::new(connection, database).unwrap()
    }
}

impl Drop for CatalogServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn catalog_reads_observe_new_versions_and_isolate_projects() {
    let server = CatalogServer::start().await;
    let first = server.store("first", crate::SpiralAccess::fixed("token".into()));
    let catalog = first.dataset_catalog();
    let (a, b) = tokio::join!(catalog.resources(), catalog.resources());
    assert_eq!(a.unwrap()[0].dataset().unwrap().version_id(), "0");
    assert!(b.is_ok());
    server.state.lock().unwrap().version = 1;
    assert_eq!(
        catalog.resources().await.unwrap()[0]
            .dataset()
            .unwrap()
            .version_id(),
        "1"
    );
    assert_eq!(server.state.lock().unwrap().reads, 6);
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .tokens
            .iter()
            .all(|token| token == "Bearer token")
    );
}

#[tokio::test]
async fn catalog_reads_as_the_currently_signed_in_user() {
    let server = CatalogServer::start().await;
    let (publisher, access) = crate::SpiralAccessPublisher::new();
    let store = server.store("first", access);
    let catalog = store.dataset_catalog();
    let signed_out = catalog.resources().await.unwrap_err();
    assert!(signed_out.contains("Sign in to Spiral"));
    assert_eq!(server.state.lock().unwrap().reads, 0);
    publisher.publish(Some("first".into()));
    catalog.resources().await.unwrap();
    publisher.publish(Some("refreshed".into()));
    catalog.resources().await.unwrap();
    let tokens = server.state.lock().unwrap().tokens.clone();
    assert_eq!(tokens.first().map(String::as_str), Some("Bearer first"));
    assert_eq!(tokens.last().map(String::as_str), Some("Bearer refreshed"));
}

#[tokio::test]
async fn unauthorized_responses_fail_without_retrying() {
    let server = CatalogServer::start().await;
    server.state.lock().unwrap().deny_all = true;
    let store = server.store("first", crate::SpiralAccess::fixed("token".into()));
    let catalog = store.dataset_catalog();
    let response = catalog
        .request::<Value>(catalog.client.get(format!("{}/denied", server.endpoint)))
        .await;
    assert!(response.unwrap_err().contains("401"));
    assert_eq!(server.state.lock().unwrap().reads, 1);
}
