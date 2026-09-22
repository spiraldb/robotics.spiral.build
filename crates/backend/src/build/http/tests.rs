use super::*;
use axum::{body::Body, response::IntoResponse};

fn submitted(content_type: Option<&str>, body: Body) -> Request {
    let mut request = Request::builder()
        .method("POST")
        .uri("/build-documents/save");
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    request.body(body).unwrap()
}

async fn read(request: Request, limit: usize) -> Result<BuildDocument, StatusCode> {
    SubmittedBuildDocument::read(request, limit)
        .await
        .map(|SubmittedBuildDocument(document)| document)
        .map_err(|failure| failure.into_response().status())
}

#[tokio::test]
async fn a_document_is_accepted_and_malformed_bodies_keep_their_own_failure() {
    let document = BuildDocument::new("folding", "Cloth folding", 1, r#"{"nodes":[]}"#);
    let body = serde_json::to_string(&document).unwrap();
    assert_eq!(
        read(
            submitted(Some("application/json"), Body::from(body.clone())),
            MAX_SAVE_BODY_BYTES
        )
        .await
        .unwrap(),
        document
    );
    for (content_type, body, expected) in [
        (None, body.clone(), StatusCode::UNSUPPORTED_MEDIA_TYPE),
        (
            Some("text/plain"),
            body.clone(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            Some("application/json"),
            "{".into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            Some("application/json"),
            r#"{"id":"folding"}"#.into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            read(
                submitted(content_type, Body::from(body)),
                MAX_SAVE_BODY_BYTES
            )
            .await
            .unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn the_request_bound_holds_against_the_advertised_and_the_streamed_length() {
    let advertised = Request::builder()
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, "9")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        read(advertised, 8).await.unwrap_err(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    // A chunked body that never advertises a length is cut off just as firmly.
    let chunks = futures_util::stream::iter([
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"1234")),
        Ok(bytes::Bytes::from_static(b"5678")),
    ]);
    assert_eq!(
        read(
            submitted(Some("application/json"), Body::from_stream(chunks)),
            7
        )
        .await
        .unwrap_err(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
