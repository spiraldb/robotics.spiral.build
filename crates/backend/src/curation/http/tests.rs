use super::*;
use crate::projects::tests::{ENDPOINT, ProjectFixture};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use spiral_robotics_common::project::ProjectKey;
use tower::ServiceExt;

async fn save_status(fixture: &ProjectFixture, body: String) -> StatusCode {
    command_status(fixture, "/curations/save", body).await
}

async fn command_status(fixture: &ProjectFixture, uri: &str, body: String) -> StatusCode {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(ProjectKey::ENDPOINT_HEADER, ENDPOINT)
        .header(ProjectKey::PROJECT_HEADER, "curation_project")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    CurationEndpoints::commands()
        .with_state(StudioContext::new(
            fixture.projects.clone(),
            crate::account::SpiralAccount::offline(),
        ))
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_save_may_exceed_the_default_json_bound_but_not_its_own() {
    let fixture = ProjectFixture::new();
    let document =
        CurationDocument::new("folding", "Cloth folding", 1, "x".repeat(4 * 1024 * 1024));
    assert_eq!(
        save_status(&fixture, serde_json::to_string(&document).unwrap()).await,
        StatusCode::OK
    );
    assert_eq!(
        save_status(&fixture, " ".repeat(MAX_SAVE_BODY_BYTES + 1)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fixture.projects.hardware().shutdown().await.unwrap();
}

#[tokio::test]
async fn publishing_is_a_command_that_refuses_a_name_that_is_not_an_identifier() {
    let fixture = ProjectFixture::new();
    for (uri, body) in [
        (
            "/curations/publish",
            serde_json::to_string(&PublishRequest::new("2folds", "SELECT 1")).unwrap(),
        ),
        (
            "/curations/unpublish",
            serde_json::to_string(&UnpublishRequest::new("my folds")).unwrap(),
        ),
    ] {
        assert_eq!(
            command_status(&fixture, uri, body).await,
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    fixture.projects.hardware().shutdown().await.unwrap();
}
