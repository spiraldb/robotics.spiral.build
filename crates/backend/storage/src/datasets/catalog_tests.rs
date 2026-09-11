use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use super::*;
use crate::{AdbcDatabase, ProjectStore};

#[derive(Default)]
struct CatalogState {
    tokens: usize,
    reads: usize,
    version: usize,
    rejected: Option<String>,
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
        if request.uri().path() == "/v1/oauth2/token" {
            state.tokens += 1;
            return Json(
                json!({"access_token":format!("token-{}", state.tokens), "expires_in":3600}),
            )
            .into_response();
        }
        state.reads += 1;
        let token = request
            .headers()
            .get(AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();
        if state.deny_all || state.rejected.as_deref() == Some(token) {
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

    fn store(&self, project: &str) -> ProjectStore {
        let settings = SpiralConnectionSettings::new(self.endpoint.clone(), self.endpoint.clone())
            .unwrap()
            .with_project(project)
            .unwrap()
            .with_credentials(Some(SpiralCredentials::new(
                project.into(),
                "secret".into(),
            )));
        let database = Arc::new(AdbcDatabase::new(settings.clone()));
        ProjectStore::new(settings, "studio".into(), database).unwrap()
    }
}

impl Drop for CatalogServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn concurrent_catalog_reads_share_auth_but_observe_new_versions_and_isolate_projects() {
    let server = CatalogServer::start().await;
    let first = server.store("first");
    let catalog = first.dataset_catalog();
    let (a, b, c) = tokio::join!(
        catalog.resources(),
        catalog.resources(),
        catalog.resources()
    );
    assert_eq!(a.unwrap()[0].dataset().unwrap().version_id(), "0");
    assert!(b.is_ok() && c.is_ok());
    assert_eq!(server.state.lock().unwrap().tokens, 1);
    server.state.lock().unwrap().version = 1;
    assert_eq!(
        catalog.resources().await.unwrap()[0]
            .dataset()
            .unwrap()
            .version_id(),
        "1"
    );
    assert_eq!(server.state.lock().unwrap().tokens, 1);
    assert_eq!(server.state.lock().unwrap().reads, 8);
    server
        .store("second")
        .dataset_catalog()
        .resources()
        .await
        .unwrap();
    assert_eq!(server.state.lock().unwrap().tokens, 2);
}

#[tokio::test]
async fn catalog_renews_expiring_and_rejected_tokens() {
    let server = CatalogServer::start().await;
    let store = server.store("first");
    let catalog = store.dataset_catalog();
    catalog.resources().await.unwrap();
    catalog.token.lock().await.as_mut().unwrap().refresh_at = Some(Instant::now());
    catalog.resources().await.unwrap();
    assert_eq!(server.state.lock().unwrap().tokens, 2);
    server.state.lock().unwrap().rejected = Some("Bearer token-2".into());
    let (a, b) = tokio::join!(catalog.resources(), catalog.resources());
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(server.state.lock().unwrap().tokens, 3);
    catalog.resources().await.unwrap();
    assert_eq!(server.state.lock().unwrap().tokens, 3);
}

#[tokio::test]
async fn persistent_unauthorized_response_is_retried_only_once() {
    let server = CatalogServer::start().await;
    server.state.lock().unwrap().deny_all = true;
    let store = server.store("first");
    let catalog = store.dataset_catalog();
    let response = catalog
        .request::<Value>(catalog.client.get(format!("{}/denied", server.endpoint)))
        .await;
    assert!(response.unwrap_err().contains("401"));
    let state = server.state.lock().unwrap();
    assert_eq!((state.tokens, state.reads), (2, 2));
}

#[test]
fn token_lifetimes_support_oauth_jwt_and_opaque_tokens() {
    let token = AccessToken {
        access_token: "opaque".into(),
        expires_in: Some(60),
    };
    assert_eq!(token.refresh_after(), Some(Duration::from_secs(30)));
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({"exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()+3600.0})
            .to_string(),
    );
    let token = AccessToken {
        access_token: format!("header.{payload}.signature"),
        expires_in: None,
    };
    assert!(token.refresh_after().unwrap() > Duration::from_secs(3500));
    let token = AccessToken {
        access_token: "opaque".into(),
        expires_in: None,
    };
    assert_eq!(token.refresh_after(), None);
}
