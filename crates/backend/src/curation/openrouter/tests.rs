use super::*;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode as Status, Uri};
use axum::response::{IntoResponse as _, Response};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// The key a stand-in OpenRouter accepts.
const TEST_KEY: &str = "test-openrouter-key";

/// What a stand-in OpenRouter answers a request: its route, its body, and
/// how many requests came before it.
type Responder = dyn Fn(&str, &Value, usize) -> Response + Send + Sync;

/// A stand-in OpenRouter: it refuses any key but [`TEST_KEY`] as OpenRouter
/// does, answers every other request as its responder says, and keeps the
/// route and body of each request it answered.
#[derive(Clone)]
pub(in crate::curation) struct MockOpenRouter {
    respond: Arc<Responder>,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
}

impl MockOpenRouter {
    pub(in crate::curation) fn new(
        respond: impl Fn(&str, &Value, usize) -> Response + Send + Sync + 'static,
    ) -> Self {
        Self {
            respond: Arc::new(respond),
            requests: Arc::default(),
        }
    }

    /// Serve on a free local port, and reach it with the key it accepts,
    /// retrying within milliseconds.
    pub(in crate::curation) async fn serve(&self) -> OpenRouter {
        let router = axum::Router::new()
            .fallback(axum::routing::post(Self::answer))
            .with_state(self.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        OpenRouter::reaching(&format!("http://{address}"), TEST_KEY, Self::quick_retry()).unwrap()
    }

    /// Every request it answered, in order: its route and its body.
    pub(in crate::curation) fn requests(&self) -> Vec<(String, Value)> {
        self.requests.lock().unwrap().clone()
    }

    /// A chat completion answering `content`.
    pub(in crate::curation) fn completion(content: &str) -> Response {
        axum::Json(json!({"choices": [{"message": {"content": content}}]})).into_response()
    }

    /// A Decisions response answering `answers`.
    pub(in crate::curation) fn decided(answers: Value) -> Response {
        axum::Json(json!({"answers": answers})).into_response()
    }

    fn quick_retry() -> RetryPolicy {
        RetryPolicy {
            attempts: 3,
            first_pause: Duration::from_millis(1),
            longest_pause: Duration::from_millis(5),
        }
    }

    async fn answer(
        State(mock): State<Self>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let bearer = format!("Bearer {TEST_KEY}");
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some(&bearer)
        {
            let refused = json!({"error": {"code": 401, "message": "No auth credentials found"}});
            return (Status::UNAUTHORIZED, axum::Json(refused)).into_response();
        }
        let body = serde_json::from_slice::<Value>(&body).unwrap();
        let earlier = {
            let mut requests = mock.requests.lock().unwrap();
            requests.push((uri.path().to_owned(), body.clone()));
            requests.len() - 1
        };
        (mock.respond)(uri.path(), &body, earlier)
    }
}

#[test]
fn openrouter_is_reached_with_the_deployments_key() {
    let missing = OpenRouter::of(&ProviderKeys::default(), "Break down subtasks");
    let reason = missing.err().unwrap();
    assert!(reason.starts_with("Break down subtasks needs OpenRouter"));
    assert!(reason.contains("OPENROUTER_API_KEY"));
    let keys = ProviderKeys::from_variables(|name| {
        (name == "OPENROUTER_API_KEY").then(|| "sk-or-test".to_owned())
    });
    let openrouter = OpenRouter::of(&keys, "Break down subtasks").unwrap();
    assert_eq!(openrouter.endpoint, "https://openrouter.ai/api");
}

#[test]
fn a_pause_doubles_up_to_its_longest_unless_openrouter_names_one() {
    let retry = RetryPolicy::OPENROUTER;
    assert_eq!(retry.pause(1, None), Duration::from_secs(1));
    assert_eq!(retry.pause(3, None), Duration::from_secs(4));
    assert_eq!(retry.pause(9, None), Duration::from_secs(30));
    let named = Some(Duration::from_millis(250));
    assert_eq!(retry.pause(3, named), Duration::from_millis(250));
}

#[tokio::test]
async fn a_completion_is_the_first_choices_text_from_the_chat_route() {
    let mock = MockOpenRouter::new(|_, _, _| MockOpenRouter::completion("the subtasks"));
    let openrouter = mock.serve().await;
    let answer = openrouter
        .chat_completion(&json!({"model": "google/gemini-3.8-flash"}))
        .await;
    assert_eq!(answer, Ok("the subtasks".to_owned()));
    let requests = mock.requests();
    assert_eq!(requests[0].0, "/v1/chat/completions");
    assert_eq!(requests[0].1["model"], "google/gemini-3.8-flash");
    let decided = MockOpenRouter::new(|_, _, _| MockOpenRouter::decided(json!({})));
    let openrouter = decided.serve().await;
    let answer = openrouter
        .decisions(&json!({"model": "jev"}))
        .await
        .unwrap();
    assert_eq!(answer, json!({"answers": {}}));
    assert_eq!(decided.requests()[0].0, "/alpha/decisions");
}

#[tokio::test]
async fn what_may_pass_is_tried_again_and_a_refused_key_fails_every_request() {
    // Rate limited once, then answered.
    let limited = MockOpenRouter::new(|_, _, earlier| match earlier {
        0 => (Status::TOO_MANY_REQUESTS, [("retry-after", "0")]).into_response(),
        _ => MockOpenRouter::completion("answered"),
    });
    let answer = limited.serve().await.chat_completion(&json!({})).await;
    assert_eq!(answer, Ok("answered".to_owned()));
    assert_eq!(limited.requests().len(), 2);
    // Unavailable on every attempt: only this request fails.
    let unavailable = MockOpenRouter::new(|_, _, _| Status::SERVICE_UNAVAILABLE.into_response());
    let Err(ProviderFailure::Request(reason)) =
        unavailable.serve().await.decisions(&json!({})).await
    else {
        panic!("a request still failing fails alone")
    };
    assert!(reason.ends_with("after 3 attempts"), "{reason}");
    assert_eq!(unavailable.requests().len(), 3);
    // An account out of credit, reported inside a 200 body, fails them all.
    let unpaid = MockOpenRouter::new(|_, _, _| {
        axum::Json(json!({"error": {"code": 402, "message": "Insufficient credits"}}))
            .into_response()
    });
    assert_eq!(
        unpaid.serve().await.chat_completion(&json!({})).await,
        Err(ProviderFailure::Account(
            "OpenRouter returned HTTP 402: Insufficient credits".into()
        ))
    );
    // A refused key too, and a bad request only itself.
    let mock =
        MockOpenRouter::new(|_, _, _| (Status::BAD_REQUEST, "no such model").into_response());
    let refused = OpenRouter::reaching(
        &mock.serve().await.endpoint,
        "another-key",
        MockOpenRouter::quick_retry(),
    )
    .unwrap();
    assert_eq!(
        refused.chat_completion(&json!({})).await,
        Err(ProviderFailure::Account(
            "OpenRouter returned HTTP 401: No auth credentials found".into()
        ))
    );
    let openrouter = mock.serve().await;
    assert_eq!(
        openrouter.chat_completion(&json!({})).await,
        Err(ProviderFailure::Request(
            "OpenRouter returned HTTP 400: no such model".into()
        ))
    );
    let silent = MockOpenRouter::new(|_, _, _| axum::Json(json!({"choices": []})).into_response());
    assert_eq!(
        silent.serve().await.chat_completion(&json!({})).await,
        Err(ProviderFailure::Request(
            "the model answered with no message text".into()
        ))
    );
}
