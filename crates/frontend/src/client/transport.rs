//! Browser HTTP transport. All request state is owned by the originating call.
use serde::{Deserialize, Serialize};
use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};
/// Stable command code and message received as an HTTP JSON body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct CommandError(CommandFailure);

impl CommandError {
    pub(crate) fn new(code: CommandFailureCode, message: impl Into<String>) -> Self {
        Self(CommandFailure::new(code, message))
    }

    pub(crate) fn code(&self) -> CommandFailureCode {
        self.0.code()
    }

    pub(crate) fn retryable(&self) -> bool {
        !matches!(
            self.code(),
            CommandFailureCode::IncompatibleWorkflow
                | CommandFailureCode::InvalidInput
                | CommandFailureCode::NotFound
                | CommandFailureCode::StaleProject
        )
    }

    pub(crate) fn recovery_guidance(&self) -> &'static str {
        match self.code() {
            CommandFailureCode::ProjectTransition => {
                "Wait for the project change to finish, then retry."
            }
            CommandFailureCode::IncompatibleWorkflow => {
                "This Studio build cannot open the saved Workflow. Update Studio before retrying."
            }
            CommandFailureCode::StaleProject => {
                "The project changed. Reload this view to continue."
            }
            CommandFailureCode::InvalidInput => "Check the supplied values and try again.",
            CommandFailureCode::NotFound => "Reload this view to check what is still available.",
            CommandFailureCode::Conflict => "Finish the conflicting operation, then retry.",
            CommandFailureCode::ProjectUnavailable => {
                "Reconnect the project in Settings, then retry."
            }
            CommandFailureCode::External
            | CommandFailureCode::Transport
            | CommandFailureCode::Internal => {
                "Retry the request. If it keeps failing, check the backend connection."
            }
        }
    }

    pub(crate) fn stale_project() -> Self {
        Self::new(
            CommandFailureCode::StaleProject,
            "The project changed before this request completed. Refresh the current project and try again.",
        )
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}
impl From<CommandError> for String {
    fn from(error: CommandError) -> Self {
        error.to_string()
    }
}

/// Owns a request before awaiting it, including project identity and serialized data.
pub(crate) struct HttpRequest {
    method: &'static str,
    url: Result<String, CommandError>,
    session: Option<String>,
    body: Result<Option<RequestBody>, CommandError>,
}

enum RequestBody {
    Json(String),
    Upload(dioxus::html::FileData),
}

impl HttpRequest {
    pub(crate) fn new(method: &'static str, url: impl Into<String>) -> Self {
        Self {
            method,
            url: Ok(url.into()),
            session: None,
            body: Ok(None),
        }
    }

    pub(crate) fn for_session(mut self, session: String) -> Self {
        self.session = Some(session);
        self
    }

    pub(crate) fn json(mut self, value: &impl Serialize) -> Self {
        self.body = serde_json::to_string(value)
            .map(|body| Some(RequestBody::Json(body)))
            .map_err(|error| {
                CommandError::new(CommandFailureCode::InvalidInput, error.to_string())
            });
        self
    }

    pub(crate) fn query(mut self, value: &impl Serialize) -> Self {
        self.url = self.url.and_then(|url| {
            serde_urlencoded::to_string(value)
                .map(|query| format!("{url}?{query}"))
                .map_err(|error| {
                    CommandError::new(CommandFailureCode::InvalidInput, error.to_string())
                })
        });
        self
    }

    pub(crate) fn upload(mut self, document: dioxus::html::FileData) -> Self {
        self.body = Ok(Some(RequestBody::Upload(document)));
        self
    }

    pub(crate) async fn send<T: serde::de::DeserializeOwned>(self) -> Result<T, CommandError> {
        let (response, _abort) = self.response("application/json").await?;
        let status = response.status();
        let body = response.text().await.map_err(CommandError::transport)?;
        CommandError::decode_response(status, &body)
    }

    pub(crate) async fn bytes(self) -> Result<Vec<u8>, CommandError> {
        let (response, _abort) = self.response("application/vnd.apache.arrow.stream").await?;
        if !(200..300).contains(&response.status()) {
            let status = response.status();
            let body = response.text().await.map_err(CommandError::transport)?;
            return CommandError::decode_response(status, &body);
        }
        if response.headers().get("Content-Type").as_deref()
            != Some("application/vnd.apache.arrow.stream")
        {
            return Err(CommandError::transport("Expected Arrow IPC response"));
        }
        response.binary().await.map_err(CommandError::transport)
    }

    /// Decode complete NDJSON records; byte buffering preserves split UTF-8 characters.
    pub(crate) async fn stream_json<T: serde::de::DeserializeOwned>(
        self,
        mut on_event: impl FnMut(T) -> Result<(), CommandError>,
    ) -> Result<(), CommandError> {
        use futures_util::StreamExt as _;
        let (response, _abort) = self.response("application/x-ndjson").await?;
        if !(200..300).contains(&response.status()) {
            let status = response.status();
            return CommandError::decode_response(
                status,
                &response.text().await.map_err(CommandError::transport)?,
            );
        }
        let body = response
            .body()
            .ok_or_else(|| CommandError::transport("Missing model response body"))?;
        let mut stream = wasm_streams::ReadableStream::from_raw(body).into_stream();
        let mut pending = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                CommandError::transport(format!("Model stream failed: {error:?}"))
            })?;
            pending.extend(js_sys::Uint8Array::new(&chunk).to_vec());
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line = pending.drain(..=end).collect::<Vec<_>>();
                on_event(serde_json::from_slice(&line).map_err(CommandError::transport)?)?;
            }
            if pending.len() > 2 * 1024 * 1024 {
                return Err(CommandError::transport("Model event exceeds 2 MiB"));
            }
        }
        if !pending.is_empty() {
            return Err(CommandError::transport("Incomplete model event"));
        }
        Ok(())
    }

    async fn response(
        self,
        accept: &str,
    ) -> Result<(gloo_net::http::Response, RequestCancellation), CommandError> {
        use gloo_net::http::{Method, RequestBuilder};
        let abort = RequestCancellation::new()?;
        let mut request = RequestBuilder::new(&self.url?)
            .method(self.method.parse::<Method>().expect("declared HTTP method"))
            .header("Accept", accept)
            .abort_signal(Some(&abort.0.signal()));
        if let Some(session) = self.session {
            request = request.header("X-Project-Session", &session);
        }
        let request = match self.body? {
            Some(RequestBody::Json(body)) => request
                .header("Content-Type", "application/json")
                .body(body),
            Some(RequestBody::Upload(document)) => {
                // Hand the browser its native File. It streams the upload without
                // allocating another project-sized buffer in WebAssembly.
                let file = document
                    .inner()
                    .downcast_ref::<web_sys::File>()
                    .ok_or_else(|| {
                        CommandError::new(
                            CommandFailureCode::InvalidInput,
                            "The selected project is not a browser file.",
                        )
                    })?;
                request
                    .header("Content-Type", "application/vnd.spiral-robotics.project")
                    .body(file.clone())
            }
            None => request.build(),
        }
        .map_err(CommandError::transport)?;
        let response = request.send().await.map_err(CommandError::transport)?;
        Ok((response, abort))
    }
}

impl CommandError {
    fn transport(error: impl std::fmt::Display) -> Self {
        Self::new(CommandFailureCode::Transport, error.to_string())
    }

    fn decode_response<T: serde::de::DeserializeOwned>(status: u16, body: &str) -> Result<T, Self> {
        if (200..300).contains(&status) {
            serde_json::from_str(body).map_err(|error| {
                Self::new(
                    CommandFailureCode::Transport,
                    format!("Invalid backend response: {error}"),
                )
            })
        } else {
            Err(serde_json::from_str::<Self>(body).unwrap_or_else(|_| {
                Self::new(
                    CommandFailureCode::Transport,
                    format!("The backend returned HTTP {status}."),
                )
            }))
        }
    }
}

struct RequestCancellation(web_sys::AbortController);
impl RequestCancellation {
    fn new() -> Result<Self, CommandError> {
        web_sys::AbortController::new().map(Self).map_err(|_| {
            CommandError::new(
                CommandFailureCode::Transport,
                "Could not initialize request cancellation.",
            )
        })
    }
}
impl Drop for RequestCancellation {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn failures_use_the_wire_code_and_never_expose_proxy_html() {
        let failure = CommandError::decode_response::<()>(
            409,
            r#"{"code":"stale_project","message":"Project replaced"}"#,
        )
        .unwrap_err();
        assert_eq!(failure.code(), CommandFailureCode::StaleProject);
        let failure =
            CommandError::decode_response::<()>(502, "<html>proxy secret</html>").unwrap_err();
        assert_eq!(failure.code(), CommandFailureCode::Transport);
        assert!(!failure.to_string().contains("secret"));
        assert!(CommandError::decode_response::<()>(200, "null").is_ok());
        assert!(CommandError::decode_response::<()>(200, "broken").is_err());
    }
}
