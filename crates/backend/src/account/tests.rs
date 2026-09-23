use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde_json::json;
use tokio::task::JoinHandle;

use super::*;

#[derive(Default)]
struct FakeState {
    pending_polls: usize,
    /// The organization each refresh asked for.
    refreshes: Vec<Option<String>>,
    issued: usize,
}

/// WorkOS and Spiral's API as the account sees them.
struct FakeSpiral {
    base: String,
    state: Arc<StdMutex<FakeState>>,
    task: JoinHandle<()>,
    directory: tempfile::TempDir,
}

impl FakeSpiral {
    async fn start(pending_polls: usize) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(StdMutex::new(FakeState {
            pending_polls,
            ..FakeState::default()
        }));
        let app = Router::new()
            .route("/user_management/authorize/device", post(Self::authorize))
            .route("/user_management/authenticate", post(Self::authenticate))
            .route("/v1/organizations/me", get(Self::me))
            .route("/v1/projects", get(Self::projects))
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            state,
            task,
            directory: tempfile::tempdir().unwrap(),
        }
    }

    fn login(&self) -> LoginFile {
        LoginFile::at(self.directory.path().join("auth.json"))
    }

    async fn account(&self) -> Arc<SpiralAccount> {
        let services = AccountServices {
            login: self.login(),
            workos: WorkOs::at(format!("{}/user_management", self.base)).unwrap(),
            control_plane: SpiralControlPlane::new().unwrap(),
        };
        SpiralAccount::restore_with(services, &self.base).await.0
    }

    async fn authorize() -> Json<serde_json::Value> {
        Json(json!({
            "device_code": "device", "user_code": "ABCD-EFGH",
            "verification_uri_complete": "https://signin.test/device?code=ABCD-EFGH",
            "expires_in": 60, "interval": 1
        }))
    }

    async fn authenticate(
        State(state): State<Arc<StdMutex<FakeState>>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Response {
        let mut state = state.lock().unwrap();
        let organization = match form["grant_type"].as_str() {
            workos::DEVICE_CODE_GRANT if state.pending_polls > 0 => {
                state.pending_polls -= 1;
                let pending = json!({"error": "authorization_pending"});
                return (StatusCode::BAD_REQUEST, Json(pending)).into_response();
            }
            "refresh_token" if form["refresh_token"] == "revoked" => {
                let ended = json!({"error": "invalid_grant", "error_description": "revoked"});
                return (StatusCode::BAD_REQUEST, Json(ended)).into_response();
            }
            "refresh_token" => {
                let organization = form.get("organization_id").cloned();
                state.refreshes.push(organization.clone());
                organization.unwrap_or_else(|| "org_1".to_owned())
            }
            _ => "org_1".to_owned(),
        };
        state.issued += 1;
        let refresh = format!("refresh-{}", state.issued);
        Json(LoginTokens::for_test(
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

    async fn projects() -> Json<serde_json::Value> {
        Json(json!({"items": [
            {"id": "project_b", "org_id": "org_1", "description": null},
            {"id": "project_a", "org_id": "org_1", "description": "Arms"}
        ], "next_page_token": null}))
    }
}

impl Drop for FakeSpiral {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn signed_in(account: &SpiralAccount) -> SpiralIdentity {
    for _ in 0..100 {
        if let SpiralAccountStatus::SignedIn(identity) = account.status().await {
            return identity;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("still {:?}", account.status().await);
}

#[tokio::test]
async fn sign_in_completes_in_the_background_and_shares_the_session() {
    let spiral = FakeSpiral::start(1).await;
    let account = spiral.account().await;
    assert_eq!(
        account.status().await,
        SpiralAccountStatus::SignedOut { problem: None }
    );
    let SpiralAccountStatus::SigningIn(pending) =
        account.sign_in(spiral.base.clone()).await.unwrap()
    else {
        panic!("sign-in did not start");
    };
    assert_eq!(pending.user_code(), "ABCD-EFGH");
    let identity = signed_in(&account).await;
    assert_eq!(identity.email(), "operator@example.test");
    assert_eq!(identity.organization().id(), "org_1");
    assert_eq!(
        spiral.login().read().unwrap().unwrap().access_token(),
        account.access_token().await.unwrap()
    );
    let projects = account.projects(&spiral.base).await.unwrap();
    assert_eq!(
        projects.iter().map(SpiralProject::id).collect::<Vec<_>>(),
        ["project_a", "project_b"]
    );

    let SpiralAccountStatus::SignedIn(switched) = account
        .switch_organization(&spiral.base, "org_2")
        .await
        .unwrap()
    else {
        panic!("switching left the session");
    };
    assert_eq!(switched.organization().id(), "org_2");
    assert_eq!(
        spiral.state.lock().unwrap().refreshes,
        [Some("org_2".to_owned())]
    );
    assert_eq!(
        spiral
            .login()
            .read()
            .unwrap()
            .unwrap()
            .organization_id()
            .as_deref(),
        Some("org_2")
    );

    account.sign_out().await.unwrap();
    assert!(spiral.login().read().unwrap().is_none());
    assert!(account.access_token().await.is_err());
}

#[tokio::test]
async fn restoring_refreshes_a_lapsed_saved_session_and_reports_an_ended_one() {
    let spiral = FakeSpiral::start(0).await;
    let lapsed = LoginTokens::for_test("org_1", Duration::ZERO, "saved");
    spiral.login().write(&lapsed).unwrap();
    let account = spiral.account().await;
    assert_eq!(signed_in(&account).await.organization().id(), "org_1");
    assert_eq!(spiral.state.lock().unwrap().refreshes, [None]);
    assert_eq!(
        spiral.login().read().unwrap().unwrap().refresh_token(),
        "refresh-1"
    );

    let revoked = LoginTokens::for_test("org_1", Duration::ZERO, "revoked");
    spiral.login().write(&revoked).unwrap();
    let SpiralAccountStatus::SignedOut {
        problem: Some(problem),
    } = spiral.account().await.status().await
    else {
        panic!("a revoked session stayed signed in");
    };
    assert!(problem.contains("Sign in again"));
}
