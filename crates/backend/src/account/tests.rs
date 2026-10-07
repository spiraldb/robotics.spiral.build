use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use spiral_robotics_storage::{StoredSession, StudioStore};
use tower::ServiceExt as _;

use super::fake::FakeSpiral;
use super::*;
use crate::projects::tests::ProjectFixture;

fn store() -> SessionStore {
    SessionStore::new(StudioStore::in_memory())
}

#[tokio::test]
async fn a_browser_session_survives_a_restart_with_its_project() {
    let spiral = FakeSpiral::start().await;
    let store = store();
    let sessions = spiral.sessions(store.clone());
    let id = FakeSpiral::sign_in(&sessions).await;
    let session = sessions.find(&id).await.unwrap();
    let identity = session.identity().await.unwrap();
    assert_eq!(identity.email(), "operator@example.test");
    assert_eq!(identity.organization().id(), "org_1");
    session.choose_project("arms").await.unwrap();
    assert!(Arc::ptr_eq(&session, &sessions.find(&id).await.unwrap()));

    let restarted = spiral.sessions(store);
    let restored = restarted.find(&id).await.unwrap();
    assert!(!Arc::ptr_eq(&session, &restored));
    assert_eq!(restored.identity().await.unwrap(), identity);
    assert_eq!(restored.project_id().await.unwrap(), "arms");
    // The saved access token was still fresh, so nothing was refreshed.
    assert!(spiral.requests().refreshes.is_empty());
    assert!(matches!(
        restarted.find("not-a-session").await,
        Err(OperationError::Unauthenticated(_))
    ));
}

#[tokio::test]
async fn restoring_refreshes_a_lapsed_session_and_forgets_an_ended_one() {
    let spiral = FakeSpiral::start().await;
    let store = store();
    let lapsed = SessionTokens::for_test("org_1", Duration::ZERO, "saved");
    store.save("lapsed", &lapsed.stored("arms")).await.unwrap();
    let revoked = SessionTokens::for_test("org_1", Duration::ZERO, "revoked");
    store.save("revoked", &revoked.stored("")).await.unwrap();
    let sessions = spiral.sessions(store.clone());

    let session = sessions.find("lapsed").await.unwrap();
    assert_eq!(session.project_id().await.unwrap(), "arms");
    assert_eq!(spiral.requests().refreshes, ["saved"]);
    let saved = store.load("lapsed").await.unwrap().unwrap();
    assert_eq!(saved.refresh_token(), "refresh-1");
    assert_eq!(saved.project_id(), "arms");

    assert!(matches!(
        sessions.find("revoked").await,
        Err(OperationError::Unauthenticated(_))
    ));
    assert!(store.load("revoked").await.unwrap().is_none());
}

#[tokio::test]
async fn a_session_asks_spiral_which_projects_it_may_open() {
    let spiral = FakeSpiral::start().await;
    let sessions = spiral.sessions(store());
    let session = sessions
        .find(&FakeSpiral::sign_in(&sessions).await)
        .await
        .unwrap();
    session.admit("arms").await.unwrap();
    assert!(matches!(
        session.admit("forbidden_arms").await,
        Err(OperationError::NotFound(_))
    ));
    let projects = session.projects().await.unwrap();
    assert_eq!(
        projects.iter().map(SpiralProject::id).collect::<Vec<_>>(),
        ["project_a", "project_b"]
    );
}

#[tokio::test]
async fn signing_out_revokes_the_refresh_token_and_ends_the_session() {
    let spiral = FakeSpiral::start().await;
    let store = store();
    let sessions = spiral.sessions(store.clone());
    let id = FakeSpiral::sign_in(&sessions).await;
    let session = sessions.find(&id).await.unwrap();
    session.choose_project("arms").await.unwrap();

    sessions.sign_out(&session).await.unwrap();
    assert_eq!(spiral.requests().revoked, ["refresh-1"]);
    assert!(session.ended());
    assert!(store.load(&id).await.unwrap().is_none());
    assert!(matches!(
        session.identity().await,
        Err(OperationError::Unauthenticated(_))
    ));
    assert!(matches!(
        sessions.find(&id).await,
        Err(OperationError::Unauthenticated(_))
    ));
}

/// The browser's whole path: start at `/sign-in`, confirm on Spiral's page,
/// return to `/callback`, and reach the API with the session cookie.
#[tokio::test]
async fn a_browser_signs_in_on_spirals_page_and_reaches_the_api_with_its_cookie() {
    let fixture = ProjectFixture::new().await;
    let studio = fixture.context();
    let app = Router::new()
        .nest(
            "/api",
            crate::http::routes::StudioApi::router(studio.clone()),
        )
        .merge(sign_in::BrowserSignIn::routes().with_state(studio));
    let send = |request: Request<Body>| app.clone().oneshot(request);

    let identity = Request::get("/api/account").body(Body::empty()).unwrap();
    assert_eq!(
        send(identity).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let started = send(
        Request::get("/sign-in?return_to=%2Fcuration")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(started.status(), StatusCode::SEE_OTHER);
    let spiral_page =
        url::Url::parse(started.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: HashMap<_, _> = spiral_page.query_pairs().into_owned().collect();
    assert_eq!(spiral_page.path(), "/oauth/authorize");
    assert_eq!(query["client_id"], "client_robotics_test");
    assert_eq!(
        query["resource"],
        spiral_page.origin().ascii_serialization()
    );
    assert_eq!(query["response_type"], "code");
    assert_eq!(
        query["scope"],
        "openid email projects:read catalog:read catalog:write tables:read tables:write"
    );
    assert_eq!(query["redirect_uri"], "http://localhost:3000/callback");
    assert_eq!(query["code_challenge_method"], "S256");
    let attempt = started.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(attempt.starts_with("studio_sign_in="));

    let callback = |state: &str| {
        Request::get(format!("/callback?code=granted&state={state}"))
            .header(header::COOKIE, &attempt)
            .body(Body::empty())
            .unwrap()
    };
    let forged = send(callback("forged")).await.unwrap();
    assert_eq!(forged.status(), StatusCode::BAD_REQUEST);

    let finished = send(callback(&query["state"])).await.unwrap();
    assert_eq!(finished.status(), StatusCode::SEE_OTHER);
    assert_eq!(finished.headers()[header::LOCATION], "/curation");
    let cookies: Vec<_> = finished
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    let session = cookies
        .iter()
        .find(|cookie| cookie.starts_with("studio_oauth_session="))
        .unwrap();
    assert!(session.contains("HttpOnly") && session.contains("SameSite=Lax"));
    let session = session.split(';').next().unwrap();
    // The code was redeemed with the verifier behind the page's challenge.
    let verifier = attempt.split('.').nth(1).unwrap();
    assert!(
        fixture
            .spiral()
            .requests()
            .redeemed
            .contains(&("granted".to_owned(), verifier.to_owned()))
    );

    let identity = Request::get("/api/account")
        .header(header::COOKIE, session)
        .body(Body::empty())
        .unwrap();
    let response = send(identity).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn saved_sessions_keep_their_tokens_out_of_debug_output() {
    let stored = StoredSession::new("access".into(), "refresh-secret".into(), String::new());
    assert!(!format!("{stored:?}").contains("refresh-secret"));
}
