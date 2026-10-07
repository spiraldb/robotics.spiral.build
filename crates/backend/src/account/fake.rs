//! Spiral OAuth and API as Studio's sessions see them, for tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde_json::json;
use spiral_robotics_storage::SessionStore;
use tokio::task::JoinHandle;

use super::control_plane::SpiralControlPlane;
use super::oauth::SpiralOAuth;
use super::sign_in::{PublicAddress, SignInAttempt};
use super::tokens::SessionTokens;
use super::{SessionServices, SpiralSessions};

/// What the fake was asked.
#[derive(Default)]
pub(super) struct FakeRequests {
    /// Refresh tokens redeemed by the application.
    pub(super) refreshes: Vec<String>,
    pub(super) revoked: Vec<String>,
    /// Each code redeemed, with the verifier that proved it.
    pub(super) redeemed: Vec<(String, String)>,
    issued: usize,
}

pub(crate) struct FakeSpiral {
    base: String,
    requests: Arc<StdMutex<FakeRequests>>,
    task: JoinHandle<()>,
}

impl FakeSpiral {
    /// A project ID this prefix starts is one Spiral refuses to show.
    pub(crate) const FORBIDDEN: &str = "forbidden";

    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(StdMutex::new(FakeRequests::default()));
        let app = Router::new()
            .route("/oauth/token", post(Self::authenticate))
            .route("/oauth/revoke", post(Self::revoke))
            .route("/userinfo", get(Self::me))
            .route("/v1/projects", get(Self::projects))
            .route("/v1/projects/{project}", get(Self::project))
            .with_state(requests.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            requests,
            task,
        }
    }

    pub(super) fn requests(&self) -> std::sync::MutexGuard<'_, FakeRequests> {
        self.requests.lock().unwrap()
    }

    /// Sessions that sign in through this fake from `http://localhost:3000`,
    /// saved in `store`.
    pub(crate) fn sessions(&self, store: SessionStore) -> SpiralSessions {
        let services = SessionServices {
            oauth: SpiralOAuth::at(
                self.base.clone(),
                "client_robotics_test".to_owned(),
                self.base.clone(),
                "http://localhost:3000/callback".to_owned(),
            )
            .unwrap(),
            control_plane: SpiralControlPlane::new(self.base.clone()).unwrap(),
            store,
        };
        SpiralSessions::with_services(
            services,
            PublicAddress::parse("http://localhost:3000").unwrap(),
        )
    }

    /// A browser that confirmed Spiral's sign-in page. Returns its session ID.
    pub(crate) async fn sign_in(sessions: &SpiralSessions) -> String {
        let attempt = SignInAttempt::new("/").unwrap();
        sessions.complete("code", &attempt).await.unwrap()
    }

    async fn authenticate(
        State(requests): State<Arc<StdMutex<FakeRequests>>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Response {
        let mut requests = requests.lock().unwrap();
        assert_eq!(form["client_id"], "client_robotics_test");
        assert!(form["resource"].starts_with("http://127.0.0.1:"));
        assert!(!form.contains_key("organization_id"));
        match form["grant_type"].as_str() {
            "authorization_code" => {
                requests
                    .redeemed
                    .push((form["code"].clone(), form["code_verifier"].clone()));
                assert_eq!(form["redirect_uri"], "http://localhost:3000/callback");
            }
            "refresh_token" if form["refresh_token"] == "revoked" => {
                let ended = json!({"error": "invalid_grant", "error_description": "revoked"});
                return (StatusCode::BAD_REQUEST, Json(ended)).into_response();
            }
            "refresh_token" => {
                requests.refreshes.push(form["refresh_token"].clone());
            }
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
        requests.issued += 1;
        let refresh = format!("refresh-{}", requests.issued);
        Json(SessionTokens::for_test(
            "org_1",
            Duration::from_secs(300),
            &refresh,
        ))
        .into_response()
    }

    async fn revoke(
        State(requests): State<Arc<StdMutex<FakeRequests>>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> StatusCode {
        assert_eq!(form["client_id"], "client_robotics_test");
        assert_eq!(form["token_type_hint"], "refresh_token");
        requests.lock().unwrap().revoked.push(form["token"].clone());
        StatusCode::OK
    }

    async fn me(headers: HeaderMap) -> Json<serde_json::Value> {
        let token = headers["authorization"].to_str().unwrap();
        let payload = token.split('.').nth(1).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .unwrap(),
        )
        .unwrap();
        Json(json!({
            "sub": "user_1", "org_id": claims["org_id"],
            "email": "operator@example.test"
        }))
    }

    async fn projects() -> Json<serde_json::Value> {
        Json(json!({"items": [
            {"id": "project_b", "org_id": "org_1", "description": null},
            {"id": "project_a", "org_id": "org_1", "description": "Arms"}
        ], "next_page_token": null}))
    }

    async fn project(Path(project): Path<String>) -> Response {
        if project.starts_with(Self::FORBIDDEN) {
            return StatusCode::FORBIDDEN.into_response();
        }
        Json(json!({"id": project, "org_id": "org_1", "description": null})).into_response()
    }
}

impl Drop for FakeSpiral {
    fn drop(&mut self) {
        self.task.abort();
    }
}
