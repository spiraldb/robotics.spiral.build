//! WorkOS and Spiral's API as Studio's sessions see them, for tests.

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
use super::sign_in::{PublicAddress, SignInAttempt};
use super::tokens::SessionTokens;
use super::workos::WorkOs;
use super::{SessionServices, SpiralSessions};

/// What the fake was asked.
#[derive(Default)]
pub(super) struct FakeRequests {
    /// The organization each refresh asked for.
    pub(super) refreshes: Vec<Option<String>>,
    /// Each code redeemed, with the verifier that proved it.
    pub(super) redeemed: Vec<(String, String)>,
    issued: usize,
    /// Projects created through the fake, which it lists after its own.
    created_projects: Vec<String>,
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
            .route("/user_management/authenticate", post(Self::authenticate))
            .route("/v1/organizations/me", get(Self::me))
            .route("/v1/organizations", get(Self::organizations))
            .route(
                "/v1/projects",
                get(Self::projects).post(Self::create_project),
            )
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
            workos: WorkOs::at(format!("{}/user_management", self.base)).unwrap(),
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
        let organization = match form["grant_type"].as_str() {
            "authorization_code" => {
                requests
                    .redeemed
                    .push((form["code"].clone(), form["code_verifier"].clone()));
                "org_1".to_owned()
            }
            "refresh_token" if form["refresh_token"] == "revoked" => {
                let ended = json!({"error": "invalid_grant", "error_description": "revoked"});
                return (StatusCode::BAD_REQUEST, Json(ended)).into_response();
            }
            "refresh_token" => {
                let organization = form.get("organization_id").cloned();
                requests.refreshes.push(organization.clone());
                organization.unwrap_or_else(|| "org_1".to_owned())
            }
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
        requests.issued += 1;
        let refresh = format!("refresh-{}", requests.issued);
        Json(SessionTokens::for_test(
            &organization,
            Duration::from_secs(300),
            &refresh,
        ))
        .into_response()
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
            "user_id": "user_1", "org_id": claims["org_id"], "org_name": "Robots Inc",
            "role": "member", "email": "operator@example.test", "first_name": "",
            "last_name": "", "email_verified": true
        }))
    }

    async fn organizations() -> Json<serde_json::Value> {
        Json(json!({"items": [
            {"org": {"id": "org_1", "name": "Robots Inc"}},
            {"org": {"id": "org_2", "name": null}}
        ], "next_page_token": null}))
    }

    async fn projects(
        State(requests): State<Arc<StdMutex<FakeRequests>>>,
    ) -> Json<serde_json::Value> {
        let mut items = vec![
            json!({"id": "project_b", "org_id": "org_1", "description": null}),
            json!({"id": "project_a", "org_id": "org_1", "description": "Arms"}),
        ];
        for id in &requests.lock().unwrap().created_projects {
            items.push(json!({"id": id, "org_id": "org_1", "description": null}));
        }
        Json(json!({"items": items, "next_page_token": null}))
    }

    /// A project named by the requested prefix, or a made-up one, and
    /// numbered like Spiral's.
    async fn create_project(
        State(requests): State<Arc<StdMutex<FakeRequests>>>,
        Json(request): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let mut requests = requests.lock().unwrap();
        let prefix = request["id_prefix"].as_str().unwrap_or("brave-otter");
        let id = format!("{prefix}-{:06}", requests.created_projects.len() + 1);
        requests.created_projects.push(id.clone());
        Json(json!({"id": id, "org_id": "org_1", "description": request["description"]}))
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
