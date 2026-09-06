//! Bounded OpenAI-compatible Chat Completions transport.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spiral_robotics_storage::workflow_assistant::{ProviderMessage, ProviderToolCall};

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-5.6-terra";
const MAX_LOCAL_ENV_BYTES: u64 = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_STREAM_TOOL_CALLS: usize = 24;
const MAX_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub(crate) struct ProviderConfig {
    base_url: Url,
    api_key: String,
    model: String,
}

impl ProviderConfig {
    pub(crate) fn from_env() -> Result<Option<Self>, String> {
        Self::from_lookup_and_local(
            |name| std::env::var(name).ok(),
            &Self::repo_root_local_env_path(),
        )
    }

    fn from_lookup_and_local(
        mut lookup: impl FnMut(&str) -> Option<String>,
        local_path: &Path,
    ) -> Result<Option<Self>, String> {
        let mut values = ProviderValues {
            api_key: lookup("LLM_API_KEY").non_empty(),
            model: lookup("LLM_MODEL").non_empty(),
            base_url: lookup("LLM_BASE_URL").non_empty(),
        };
        if values.has_missing_value() {
            values.fill_missing(ProviderValues::load(local_path)?);
        }
        let Some(api_key) = values.api_key else {
            return Ok(None);
        };
        let model = values
            .model
            .unwrap_or_else(|| DEFAULT_OPENROUTER_MODEL.to_owned());
        let base = values
            .base_url
            .unwrap_or_else(|| OPENROUTER_BASE_URL.to_owned());
        let base_url = Url::parse(&base)
            .map_err(|error| format!("Assistant provider endpoint is invalid: {error}"))?;
        let local = matches!(base_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base_url.scheme() != "https" && !(base_url.scheme() == "http" && local) {
            return Err(
                "Assistant provider endpoint must use HTTPS (HTTP is allowed only for localhost)."
                    .to_owned(),
            );
        }
        Ok(Some(Self {
            base_url,
            api_key,
            model,
        }))
    }

    fn completions_url(&self) -> Result<Url, String> {
        let mut base = self.base_url.as_str().trim_end_matches('/').to_owned();
        base.push_str("/chat/completions");
        Url::parse(&base).map_err(|error| format!("provider endpoint is invalid: {error}"))
    }

    fn repo_root_local_env_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(".env.local")
    }

    fn public_error(&self, context: &str, detail: &str) -> String {
        // Never include request headers, request bodies, or configured secrets.
        format!("LLM {context}: {detail}").replace(&self.api_key, "[redacted]")
    }
}

#[derive(Default)]
struct ProviderValues {
    api_key: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
}

impl ProviderValues {
    fn has_missing_value(&self) -> bool {
        self.api_key.is_none() || self.model.is_none() || self.base_url.is_none()
    }

    fn fill_missing(&mut self, fallback: Self) {
        if self.api_key.is_none() {
            self.api_key = fallback.api_key;
        }
        if self.model.is_none() {
            self.model = fallback.model;
        }
        if self.base_url.is_none() {
            self.base_url = fallback.base_url;
        }
    }

    fn load(path: &Path) -> Result<Self, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ProviderValues::default());
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect the local Assistant credential file: {error}"
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("The local Assistant credential path must be a regular file.".to_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(
                    "The local Assistant credential file must be readable only by its owner."
                        .to_owned(),
                );
            }
        }
        if metadata.len() > MAX_LOCAL_ENV_BYTES {
            return Err("The local Assistant credential file exceeds its size bound.".to_owned());
        }
        let encoded = fs::read(path).map_err(|error| {
            format!("Could not read the local Assistant credential file: {error}")
        })?;
        if encoded.len() as u64 > MAX_LOCAL_ENV_BYTES {
            return Err("The local Assistant credential file exceeds its size bound.".to_owned());
        }
        let contents = std::str::from_utf8(&encoded)
            .map_err(|_| "The local Assistant credential file is not UTF-8.".to_owned())?;
        Self::parse(contents)
    }

    fn parse(contents: &str) -> Result<Self, String> {
        let mut values = Self::default();
        for (index, line) in contents.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
            let Some((name, raw_value)) = line.split_once('=') else {
                continue;
            };
            let target = match name.trim() {
                "LLM_API_KEY" => &mut values.api_key,
                "LLM_MODEL" => &mut values.model,
                "LLM_BASE_URL" => &mut values.base_url,
                _ => continue,
            };
            *target =
                Some(LocalProviderValue::parse(raw_value, index + 1)?.into_string()).non_empty();
        }
        Ok(values)
    }
}

trait NonEmptyString {
    fn non_empty(self) -> Option<String>;
}

impl NonEmptyString for Option<String> {
    fn non_empty(self) -> Option<String> {
        self.map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }
}

struct LocalProviderValue(String);

impl LocalProviderValue {
    fn parse(raw: &str, line: usize) -> Result<Self, String> {
        let raw = raw.trim();
        let Some(quote @ ('\'' | '"')) = raw.chars().next() else {
            let comment = raw.char_indices().find_map(|(index, character)| {
                (character == '#'
                    && (index == 0
                        || raw[..index]
                            .chars()
                            .next_back()
                            .is_some_and(char::is_whitespace)))
                .then_some(index)
            });
            return Ok(Self(raw[..comment.unwrap_or(raw.len())].trim().to_owned()));
        };
        let mut value = String::new();
        let mut escaped = false;
        let mut closed_at = None;
        for (index, character) in raw.char_indices().skip(1) {
            if quote == '"' && escaped {
                value.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                escaped = false;
            } else if quote == '"' && character == '\\' {
                escaped = true;
            } else if character == quote {
                closed_at = Some(index + character.len_utf8());
                break;
            } else {
                value.push(character);
            }
        }
        let Some(closed_at) = closed_at else {
            return Err(format!(
                "The local Assistant credential file has an unterminated quoted value on line {line}."
            ));
        };
        let trailing = raw[closed_at..].trim();
        if !trailing.is_empty() && !trailing.starts_with('#') {
            return Err(format!(
                "The local Assistant credential file has trailing text on line {line}."
            ));
        }
        Ok(Self(value))
    }

    fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderResponse {
    content: String,
    tool_calls: Vec<ProviderToolCall>,
}

impl ProviderResponse {
    #[cfg(test)]
    pub(crate) fn test_response(
        content: impl Into<String>,
        tool_calls: Vec<ProviderToolCall>,
    ) -> Self {
        Self {
            content: content.into(),
            tool_calls,
        }
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }
    pub(crate) fn tool_calls(&self) -> &[ProviderToolCall] {
        &self.tool_calls
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ProviderMessage],
    tools: &'a [Value],
    tool_choice: &'static str,
    stream: bool,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolDelta>,
}

#[derive(Deserialize)]
struct StreamToolDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunctionDelta>,
}

#[derive(Deserialize)]
struct StreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Clone)]
pub(crate) struct OpenAiProvider {
    config: ProviderConfig,
    client: Client,
}

pub(crate) struct ProviderCompletion<'a> {
    messages: &'a [ProviderMessage],
    tools: &'a [Value],
    cancel: CancellationToken,
}

impl<'a> ProviderCompletion<'a> {
    pub(crate) fn new(
        messages: &'a [ProviderMessage],
        tools: &'a [Value],
        cancel: CancellationToken,
    ) -> Self {
        Self {
            messages,
            tools,
            cancel,
        }
    }
}

impl OpenAiProvider {
    pub(crate) fn new(config: ProviderConfig) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|error| format!("could not configure the LLM client: {error}"))?;
        Ok(Self { config, client })
    }

    pub(crate) async fn complete<F>(
        &self,
        completion: ProviderCompletion<'_>,
        mut on_text_delta: F,
    ) -> Result<ProviderResponse, String>
    where
        F: FnMut(&str),
    {
        let ProviderCompletion {
            messages,
            tools,
            cancel,
        } = completion;
        let endpoint = self.config.completions_url()?;
        for attempt in 0..MAX_ATTEMPTS {
            if cancel.is_cancelled() {
                return Err("Agent turn cancelled.".to_owned());
            }
            let request = ChatRequest {
                model: &self.config.model,
                messages,
                tools,
                tool_choice: "auto",
                stream: true,
            };
            let send = self
                .client
                .post(endpoint.clone())
                .bearer_auth(&self.config.api_key)
                .json(&request)
                .send();
            let response = tokio::select! {
                response = send => response.map_err(|error| self.config.public_error("request failed", &error.to_string()))?,
                () = cancel.cancelled() => return Err("Agent turn cancelled.".to_owned()),
            };
            if matches!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::BAD_GATEWAY
            ) && attempt + 1 < MAX_ATTEMPTS
            {
                let delay = Duration::from_millis(250 * (attempt as u64 + 1));
                tokio::select! {
                    () = tokio::time::sleep(delay) => {},
                    () = cancel.cancelled() => return Err("Agent turn cancelled.".to_owned()),
                }
                continue;
            }
            if !response.status().is_success() {
                let status = response.status();
                let body = tokio::select! {
                    body = response.bounded_error_body() => body.unwrap_or_default(),
                    () = cancel.cancelled() => return Err("Agent turn cancelled.".to_owned()),
                };
                let detail = body.public_error_summary();
                return Err(self
                    .config
                    .public_error(&format!("provider returned {status}"), &detail));
            }
            return ProviderResponse::from_stream(response, cancel, &mut on_text_delta)
                .await
                .map_err(|error| self.config.public_error("stream failed", &error));
        }
        Err("The LLM provider remained overloaded after bounded retries.".to_owned())
    }
}

impl ProviderResponse {
    async fn from_stream<F>(
        response: reqwest::Response,
        cancel: CancellationToken,
        on_text_delta: &mut F,
    ) -> Result<ProviderResponse, String>
    where
        F: FnMut(&str),
    {
        let mut received = 0_usize;
        let mut events = response
            .bytes_stream()
            .map(move |chunk| {
                let chunk = chunk.map_err(|error| format!("LLM stream failed: {error}"))?;
                received = received.saturating_add(chunk.len());
                if received > MAX_RESPONSE_BYTES {
                    return Err(
                        "The LLM provider response exceeded the 2 MiB safety limit.".to_owned()
                    );
                }
                Ok(chunk)
            })
            .eventsource();
        let mut content = String::new();
        let mut calls = Vec::<PendingToolCall>::new();
        let mut completed = false;
        while let Some(next) = tokio::select! {
            next = events.next() => next,
            () = cancel.cancelled() => return Err("Agent turn cancelled.".to_owned()),
        } {
            let event = next
                .map_err(|error| format!("The LLM provider returned invalid SSE data: {error}"))?;
            let data = event.data.trim();
            if data == "[DONE]" {
                completed = true;
                break;
            }
            let chunk: StreamChunk = serde_json::from_str(data).map_err(|error| {
                format!("The LLM provider returned malformed stream data: {error}")
            })?;
            if let Some(error) = chunk.error {
                let detail = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The provider reported a generation error.");
                return Err(detail.public_error_summary());
            }
            for choice in chunk.choices {
                if let Some(reason) = choice.finish_reason {
                    match reason.as_str() {
                        "stop" | "tool_calls" => completed = true,
                        _ => {
                            return Err(format!(
                                "The LLM provider did not complete the response ({reason})."
                            ));
                        }
                    }
                }
                if let Some(delta) = choice.delta.content {
                    content.push_str(&delta);
                    on_text_delta(&delta);
                }
                for delta in choice.delta.tool_calls {
                    if delta.index >= MAX_STREAM_TOOL_CALLS {
                        return Err(format!(
                            "The LLM provider returned a tool-call index outside the 0–{} safety bound.",
                            MAX_STREAM_TOOL_CALLS - 1
                        ));
                    }
                    if calls.len() <= delta.index {
                        calls.resize_with(delta.index + 1, PendingToolCall::default);
                    }
                    let call = &mut calls[delta.index];
                    if let Some(id) = delta.id {
                        call.id.push_str(&id);
                    }
                    if let Some(function) = delta.function {
                        if let Some(name) = function.name {
                            call.name.push_str(&name);
                        }
                        if let Some(arguments) = function.arguments {
                            call.arguments.push_str(&arguments);
                        }
                    }
                }
            }
        }
        if !completed {
            return Err("The LLM provider stream ended before completing its response.".to_owned());
        }
        let mut ids = std::collections::BTreeSet::new();
        let tool_calls = calls
            .into_iter()
            .map(|call| {
                if call.id.is_empty()
                    || call.name.is_empty()
                    || !ids.insert(call.id.clone())
                    || !serde_json::from_str::<Value>(&call.arguments)
                        .is_ok_and(|value| value.is_object())
                {
                    return Err(
                        "The LLM provider returned an incomplete or invalid tool call.".to_owned(),
                    );
                }
                Ok(ProviderToolCall::new(call.id, call.name, call.arguments))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if content.is_empty() && tool_calls.is_empty() {
            return Err("The LLM provider completed without a message or tool call.".to_owned());
        }
        Ok(Self {
            content,
            tool_calls,
        })
    }
}

trait ProviderHttpResponse {
    async fn bounded_error_body(self) -> Result<String, String>;
}

impl ProviderHttpResponse for reqwest::Response {
    async fn bounded_error_body(self) -> Result<String, String> {
        let mut stream = self.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("could not read provider error: {error}"))?;
            let remaining = (32_usize * 1024).saturating_sub(bytes.len());
            if remaining == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

trait ProviderErrorBody {
    fn public_error_summary(&self) -> String;
}

impl ProviderErrorBody for str {
    fn public_error_summary(&self) -> String {
        let compact = self.split_whitespace().collect::<Vec<_>>().join(" ");
        compact.chars().take(512).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::http::header::CONTENT_TYPE;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn write_private_local_env(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        file
    }

    struct StreamFixture;
    impl StreamFixture {
        async fn parse(body: &str) -> Result<ProviderResponse, String> {
            let response = axum::http::Response::builder()
                .body(body.to_owned())
                .unwrap()
                .into();
            ProviderResponse::from_stream(response, CancellationToken::new(), &mut |_| {}).await
        }
    }

    #[tokio::test]
    async fn multiline_sse_uses_event_boundaries_and_done_stops_the_stream() {
        let body = concat!(
            "data: {\"choices\":\ndata: [{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" ignored\"}}]}\n\n"
        );
        assert_eq!(StreamFixture::parse(body).await.unwrap().content(), "hello");
    }

    #[tokio::test]
    async fn incomplete_error_and_truncated_tool_streams_are_rejected() {
        for body in [
            "data: {\"choices\":[{\"delta\":{\"content\":\"unfinished\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"generation failed\"},\"choices\":[{\"delta\":{},\"finish_reason\":\"error\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call\",\"function\":{\"name\":\"run_query\",\"arguments\":\"{\"}}]}}]}\n\ndata: [DONE]\n\n",
        ] {
            assert!(StreamFixture::parse(body).await.is_err(), "{body}");
        }
    }

    #[tokio::test]
    async fn cancelling_an_idle_stream_wakes_the_pending_request() {
        let body = reqwest::Body::wrap_stream(futures_util::stream::pending::<
            Result<Bytes, std::io::Error>,
        >());
        let response = axum::http::Response::builder().body(body).unwrap().into();
        let cancel = CancellationToken::new();
        let request_cancel = cancel.clone();
        let mut request = tokio::spawn(async move {
            ProviderResponse::from_stream(response, request_cancel, &mut |_| {}).await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(5), &mut request)
                .await
                .is_err()
        );
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("cancelled"));
    }

    #[test]
    fn local_file_loads_credential_with_openrouter_defaults() {
        let secret = "local-secret-that-must-not-appear-in-diagnostics";
        let file = write_private_local_env(&format!(
            "# Studio-owned local provider credential\nexport LLM_API_KEY=\"{secret}\" # ignored comment\n"
        ));

        let config = ProviderConfig::from_lookup_and_local(|_| None, file.path())
            .unwrap()
            .expect("the local credential configures the provider");

        assert_eq!(config.api_key, secret);
        assert_eq!(config.model, DEFAULT_OPENROUTER_MODEL);
        assert_eq!(config.base_url.as_str(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn process_values_take_precedence_over_local_fallbacks() {
        let file = write_private_local_env(
            "LLM_API_KEY=local-secret\nLLM_MODEL=local-model\nLLM_BASE_URL=https://local.invalid/v1\n",
        );
        let process = BTreeMap::from([
            ("LLM_API_KEY", "process-secret"),
            ("LLM_MODEL", "process-model"),
            ("LLM_BASE_URL", "https://process.invalid/v1"),
        ]);

        let config = ProviderConfig::from_lookup_and_local(
            |name| process.get(name).map(ToString::to_string),
            file.path(),
        )
        .unwrap()
        .expect("process configuration is complete");

        assert_eq!(config.api_key, "process-secret");
        assert_eq!(config.model, "process-model");
        assert_eq!(config.base_url.as_str(), "https://process.invalid/v1");
    }

    #[test]
    fn local_loader_errors_never_include_secret_values() {
        let secret = "never-log-this-secret";
        let file = write_private_local_env(&format!("LLM_API_KEY=\"{secret}\n"));

        let error = match ProviderConfig::from_lookup_and_local(|_| None, file.path()) {
            Err(error) => error,
            Ok(_) => panic!("an unterminated credential must be rejected"),
        };

        assert!(!error.contains(secret));
        assert!(error.contains("unterminated quoted value"));
    }

    #[cfg(unix)]
    #[test]
    fn local_loader_rejects_credentials_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt;

        let secret = "world-readable-secret";
        let file = write_private_local_env(&format!("LLM_API_KEY={secret}\n"));
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))
            .unwrap();

        let error = match ProviderConfig::from_lookup_and_local(|_| None, file.path()) {
            Err(error) => error,
            Ok(_) => panic!("a permissive credential file must be rejected"),
        };

        assert!(!error.contains(secret));
        assert!(error.contains("readable only by its owner"));
    }

    #[test]
    fn error_format_never_contains_configuration() {
        let config = ProviderConfig {
            base_url: Url::parse(OPENROUTER_BASE_URL).unwrap(),
            api_key: "secret".to_owned(),
            model: DEFAULT_OPENROUTER_MODEL.to_owned(),
        };
        let error = config.public_error("request failed", "connection refused");
        assert!(!error.contains("LLM_API_KEY"));
        assert_eq!(error, "LLM request failed: connection refused");
    }

    #[test]
    fn provider_messages_round_trip_tool_calls() {
        let message = ProviderMessage::assistant(
            None,
            vec![ProviderToolCall::new("call-1", "list_relations", "{}")],
        );
        let decoded: ProviderMessage =
            serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
        assert_eq!(decoded.tool_calls()[0].function().name(), "list_relations");
    }

    async fn test_provider(app: Router) -> OpenAiProvider {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        OpenAiProvider::new(ProviderConfig {
            base_url: Url::parse(&format!("http://{address}")).unwrap(),
            api_key: "test-secret".to_owned(),
            model: "test-model".to_owned(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn streamed_text_and_tool_schema_round_trip() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"I will \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"list_relations\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let app = Router::new().route(
            "/chat/completions",
            post(move || async move { ([(CONTENT_TYPE, "text/event-stream")], body) }),
        );
        let provider = test_provider(app).await;
        let mut deltas = String::new();
        let response = provider
            .complete(
                ProviderCompletion::new(
                    &[ProviderMessage::text("user", "show relations")],
                    &[serde_json::json!({"type":"function"})],
                    CancellationToken::new(),
                ),
                |delta| deltas.push_str(delta),
            )
            .await
            .unwrap();
        assert_eq!(deltas, "I will ");
        assert_eq!(response.tool_calls[0].function().name(), "list_relations");
        assert_eq!(response.tool_calls[0].function().arguments(), "{}");
    }

    #[tokio::test]
    async fn fragmented_multi_tool_calls_preserve_index_and_arguments() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-a\",\"function\":{\"name\":\"describe_\",\"arguments\":\"{\\\"relations\\\":[\"}},{\"index\":1,\"id\":\"call-b\",\"function\":{\"name\":\"list_model_capabilities\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"relations\",\"arguments\":\"\\\"demo.frames\\\"]}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let app = Router::new().route(
            "/chat/completions",
            post(move || async move { ([(CONTENT_TYPE, "text/event-stream")], body) }),
        );
        let provider = test_provider(app).await;
        let response = provider
            .complete(
                ProviderCompletion::new(&[], &[], CancellationToken::new()),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(
            response.tool_calls[0].function().name(),
            "describe_relations"
        );
        assert_eq!(
            response.tool_calls[0].function().arguments(),
            "{\"relations\":[\"demo.frames\"]}"
        );
        assert_eq!(
            response.tool_calls[1].function().name(),
            "list_model_capabilities"
        );
    }

    #[tokio::test]
    async fn tool_call_index_is_bounded_before_sparse_allocation() {
        let body = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":{MAX_STREAM_TOOL_CALLS},\"id\":\"call-out-of-range\"}}]}}}}]}}\n\n"
        );
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let body = body.clone();
                async move { ([(CONTENT_TYPE, "text/event-stream")], body) }
            }),
        );
        let provider = test_provider(app).await;
        let error = provider
            .complete(
                ProviderCompletion::new(&[], &[], CancellationToken::new()),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("tool-call index"));
        assert!(error.contains("0–23"));
    }

    #[tokio::test]
    async fn utf8_split_across_http_chunks_is_decoded_after_reassembly() {
        let event = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"café robot\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let split = event.find('é').unwrap() + 1;
        let first = Bytes::copy_from_slice(&event.as_bytes()[..split]);
        let second = Bytes::copy_from_slice(&event.as_bytes()[split..]);
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let first = first.clone();
                let second = second.clone();
                async move {
                    let chunks = futures_util::stream::iter([
                        Ok::<Bytes, std::convert::Infallible>(first),
                        Ok::<Bytes, std::convert::Infallible>(second),
                    ]);
                    (
                        [(CONTENT_TYPE, "text/event-stream")],
                        Body::from_stream(chunks),
                    )
                }
            }),
        );
        let provider = test_provider(app).await;
        let mut deltas = String::new();
        let response = provider
            .complete(
                ProviderCompletion::new(&[], &[], CancellationToken::new()),
                |delta| deltas.push_str(delta),
            )
            .await
            .unwrap();
        assert_eq!(response.content, "café robot");
        assert_eq!(deltas, "café robot");
    }

    #[tokio::test]
    async fn overload_is_retried_with_a_bound() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let handler_attempts = attempts.clone();
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let attempts = handler_attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        return (StatusCode::TOO_MANY_REQUESTS, "overloaded").into_response();
                    }
                    (
                        [(CONTENT_TYPE, "text/event-stream")],
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ready\"}}]}\n\ndata: [DONE]\n\n",
                    )
                        .into_response()
                }
            }),
        );
        let provider = test_provider(app).await;
        let response = provider
            .complete(
                ProviderCompletion::new(
                    &[ProviderMessage::text("user", "hello")],
                    &[],
                    CancellationToken::new(),
                ),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(response.content, "ready");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn malformed_stream_and_cancel_are_explicit() {
        let app = Router::new().route(
            "/chat/completions",
            post(|| async {
                (
                    [(CONTENT_TYPE, "text/event-stream")],
                    "data: {not-json}\n\n",
                )
            }),
        );
        let provider = test_provider(app).await;
        let error = provider
            .complete(
                ProviderCompletion::new(
                    &[ProviderMessage::text("user", "hello")],
                    &[],
                    CancellationToken::new(),
                ),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("malformed stream data"));

        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = provider
            .complete(ProviderCompletion::new(&[], &[], cancel), |_| {})
            .await
            .unwrap_err();
        assert_eq!(error, "Agent turn cancelled.");
    }

    #[tokio::test]
    async fn provider_error_redacts_the_configured_secret() {
        let app = Router::new().route(
            "/chat/completions",
            post(|| async { (StatusCode::BAD_REQUEST, "bad token test-secret") }),
        );
        let provider = test_provider(app).await;
        let error = provider
            .complete(
                ProviderCompletion::new(&[], &[], CancellationToken::new()),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(!error.contains("test-secret"));
        assert!(error.contains("[redacted]"));
    }

    #[tokio::test]
    async fn oversized_stream_is_rejected() {
        let oversized = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "x".repeat(MAX_RESPONSE_BYTES)
        );
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let oversized = oversized.clone();
                async move { ([(CONTENT_TYPE, "text/event-stream")], oversized) }
            }),
        );
        let provider = test_provider(app).await;
        let error = provider
            .complete(
                ProviderCompletion::new(&[], &[], CancellationToken::new()),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("2 MiB safety limit"));
    }
}
