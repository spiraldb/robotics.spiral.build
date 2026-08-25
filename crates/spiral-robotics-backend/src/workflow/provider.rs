//! Bounded OpenAI-compatible Chat Completions transport.

use std::collections::VecDeque;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        Self::from_lookup_and_local(|name| std::env::var(name).ok(), &repo_root_local_env_path())
    }

    fn from_lookup_and_local(
        mut lookup: impl FnMut(&str) -> Option<String>,
        local_path: &Path,
    ) -> Result<Option<Self>, String> {
        let mut values = ProviderValues {
            api_key: non_empty(lookup("LLM_API_KEY")),
            model: non_empty(lookup("LLM_MODEL")),
            base_url: non_empty(lookup("LLM_BASE_URL")),
        };
        if values.has_missing_value() {
            values.fill_missing(load_local_provider_values(local_path)?);
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
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn repo_root_local_env_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".env.local")
}

fn load_local_provider_values(path: &Path) -> Result<ProviderValues, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(ProviderValues::default()),
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
    let encoded = fs::read(path)
        .map_err(|error| format!("Could not read the local Assistant credential file: {error}"))?;
    if encoded.len() as u64 > MAX_LOCAL_ENV_BYTES {
        return Err("The local Assistant credential file exceeds its size bound.".to_owned());
    }
    let contents = std::str::from_utf8(&encoded)
        .map_err(|_| "The local Assistant credential file is not UTF-8.".to_owned())?;
    parse_local_provider_values(contents)
}

fn parse_local_provider_values(contents: &str) -> Result<ProviderValues, String> {
    let mut values = ProviderValues::default();
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
        *target = non_empty(Some(parse_local_value(raw_value, index + 1)?));
    }
    Ok(values)
}

fn parse_local_value(raw: &str, line: usize) -> Result<String, String> {
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
        return Ok(raw[..comment.unwrap_or(raw.len())].trim().to_owned());
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
    Ok(value)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderMessage {
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ProviderToolCall>,
}

impl ProviderMessage {
    pub(crate) fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub(crate) fn tool(call_id: String, content: String) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(content),
            tool_call_id: Some(call_id),
            tool_calls: Vec::new(),
        }
    }

    pub(crate) fn assistant(content: Option<String>, tool_calls: Vec<ProviderToolCall>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content,
            tool_call_id: None,
            tool_calls,
        }
    }

    pub(crate) fn role(&self) -> &str {
        &self.role
    }
    #[cfg(test)]
    pub(crate) fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }
    pub(crate) fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }
    pub(crate) fn tool_calls(&self) -> &[ProviderToolCall] {
        &self.tool_calls
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: ProviderFunctionCall,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderResponse {
    content: String,
    tool_calls: Vec<ProviderToolCall>,
}

impl ProviderToolCall {
    #[cfg(test)]
    pub(crate) fn test_function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_owned(),
            function: ProviderFunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) const fn function(&self) -> &ProviderFunctionCall {
        &self.function
    }
}

impl ProviderFunctionCall {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn arguments(&self) -> &str {
        &self.arguments
    }
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
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
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
    cancel: Arc<AtomicBool>,
}

impl<'a> ProviderCompletion<'a> {
    pub(crate) fn new(
        messages: &'a [ProviderMessage],
        tools: &'a [Value],
        cancel: Arc<AtomicBool>,
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
            if cancel.load(Ordering::SeqCst) {
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
                response = send => response.map_err(|error| provider_error("request failed", &error.to_string()))?,
                () = wait_cancelled(cancel.clone()) => return Err("Agent turn cancelled.".to_owned()),
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
                    () = wait_cancelled(cancel.clone()) => return Err("Agent turn cancelled.".to_owned()),
                }
                continue;
            }
            if !response.status().is_success() {
                let status = response.status();
                let body = bounded_body(response).await.unwrap_or_default();
                let detail = public_error_body(&body).replace(&self.config.api_key, "[redacted]");
                return Err(provider_error(
                    &format!("provider returned {status}"),
                    &detail,
                ));
            }
            return parse_stream(response, cancel, &mut on_text_delta).await;
        }
        Err("The LLM provider remained overloaded after bounded retries.".to_owned())
    }
}

async fn parse_stream<F>(
    response: reqwest::Response,
    cancel: Arc<AtomicBool>,
    on_text_delta: &mut F,
) -> Result<ProviderResponse, String>
where
    F: FnMut(&str),
{
    let mut stream = response.bytes_stream();
    // HTTP byte chunks are not UTF-8 character boundaries. Buffer complete
    // SSE lines as bytes, then decode each line after its newline arrives.
    let mut buffer = VecDeque::new();
    let mut searched = 0_usize;
    let mut received = 0_usize;
    let mut content = String::new();
    let mut calls = Vec::<PendingToolCall>::new();
    while let Some(next) = tokio::select! {
        next = stream.next() => next,
        () = wait_cancelled(cancel.clone()) => return Err("Agent turn cancelled.".to_owned()),
    } {
        let chunk = next.map_err(|error| provider_error("stream failed", &error.to_string()))?;
        received = received.saturating_add(chunk.len());
        if received > MAX_RESPONSE_BYTES {
            return Err("The LLM provider response exceeded the 2 MiB safety limit.".to_owned());
        }
        buffer.extend(chunk);
        while let Some(position) = buffer
            .iter()
            .enumerate()
            .skip(searched)
            .find_map(|(index, byte)| (*byte == b'\n').then_some(index))
        {
            // VecDeque removes the completed prefix without shifting a growing
            // partial SSE line. Together with `searched`, every received byte
            // is scanned and copied at most once, even for adversarial streams
            // with many short lines or multibyte characters split by chunks.
            let mut line = buffer.drain(..=position).collect::<Vec<_>>();
            line.pop(); // newline
            searched = 0;
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| "The LLM provider returned non-UTF-8 stream data.".to_owned())?;
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let chunk: StreamChunk = serde_json::from_str(data).map_err(|error| {
                format!("The LLM provider returned malformed stream data: {error}")
            })?;
            for choice in chunk.choices {
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
        searched = buffer.len();
    }
    if !buffer.is_empty() {
        let tail = buffer.into_iter().collect::<Vec<_>>();
        std::str::from_utf8(&tail)
            .map_err(|_| "The LLM provider returned non-UTF-8 stream data.".to_owned())?;
    }
    let tool_calls = calls
        .into_iter()
        .filter(|call| !call.id.is_empty() || !call.name.is_empty())
        .map(|call| ProviderToolCall {
            id: call.id,
            kind: "function".to_owned(),
            function: ProviderFunctionCall {
                name: call.name,
                arguments: call.arguments,
            },
        })
        .collect();
    Ok(ProviderResponse {
        content,
        tool_calls,
    })
}

async fn bounded_body(response: reqwest::Response) -> Result<String, String> {
    let mut stream = response.bytes_stream();
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

fn public_error_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(512).collect()
}

fn provider_error(context: &str, detail: &str) -> String {
    // Never include request headers, request bodies, or configured secrets.
    format!("LLM {context}: {detail}")
}

async fn wait_cancelled(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
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
    use std::sync::atomic::AtomicUsize;

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
        let error = provider_error("request failed", "connection refused");
        assert!(!error.contains("LLM_API_KEY"));
        assert_eq!(error, "LLM request failed: connection refused");
    }

    #[test]
    fn provider_messages_round_trip_tool_calls() {
        let message = ProviderMessage {
            role: "assistant".to_owned(),
            content: None,
            tool_call_id: None,
            tool_calls: vec![ProviderToolCall {
                id: "call-1".to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: "list_relations".to_owned(),
                    arguments: "{}".to_owned(),
                },
            }],
        };
        let decoded: ProviderMessage =
            serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
        assert_eq!(decoded.tool_calls[0].function.name, "list_relations");
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
                    Arc::new(AtomicBool::new(false)),
                ),
                |delta| deltas.push_str(delta),
            )
            .await
            .unwrap();
        assert_eq!(deltas, "I will ");
        assert_eq!(response.tool_calls[0].function.name, "list_relations");
        assert_eq!(response.tool_calls[0].function.arguments, "{}");
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
                ProviderCompletion::new(&[], &[], Arc::new(AtomicBool::new(false))),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].function.name, "describe_relations");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            "{\"relations\":[\"demo.frames\"]}"
        );
        assert_eq!(
            response.tool_calls[1].function.name,
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
                ProviderCompletion::new(&[], &[], Arc::new(AtomicBool::new(false))),
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
                ProviderCompletion::new(&[], &[], Arc::new(AtomicBool::new(false))),
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
                    Arc::new(AtomicBool::new(false)),
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
                    Arc::new(AtomicBool::new(false)),
                ),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("malformed stream data"));

        let cancel = Arc::new(AtomicBool::new(true));
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
                ProviderCompletion::new(&[], &[], Arc::new(AtomicBool::new(false))),
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
                ProviderCompletion::new(&[], &[], Arc::new(AtomicBool::new(false))),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("2 MiB safety limit"));
    }
}
