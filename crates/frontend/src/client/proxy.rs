//! Browser client for Studio's provider-neutral HTTP relay.

use super::transport::{CommandError, HttpRequest};
use futures_util::StreamExt as _;

pub(crate) struct ProxyClient;

impl ProxyClient {
    pub(crate) async fn credentials()
    -> Result<spiral_robotics_common::credentials::StudioCredentials, CommandError> {
        HttpRequest::new("GET", "/api/credentials").send().await
    }

    pub(crate) fn request(method: &'static str, url: &str) -> HttpRequest {
        HttpRequest::new(method, "/api/proxy").query(&[("url", url)])
    }

    pub(crate) async fn text(
        request: HttpRequest,
        max_bytes: usize,
    ) -> Result<String, CommandError> {
        String::from_utf8(Self::bytes(request, max_bytes).await?)
            .map_err(|_| CommandError::transport("Upstream response is not UTF-8 text"))
    }

    pub(crate) async fn bytes(
        request: HttpRequest,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CommandError> {
        let (response, _cancel) = request.response("*/*").await?;
        let status = response.status();
        let body = response
            .body()
            .ok_or_else(|| CommandError::transport("Missing upstream response body"))?;
        let mut stream = wasm_streams::ReadableStream::from_raw(body).into_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                CommandError::transport(format!("Upstream response failed: {error:?}"))
            })?;
            let chunk = js_sys::Uint8Array::new(&chunk).to_vec();
            if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                return Err(CommandError::transport(
                    "Upstream response exceeded its size limit",
                ));
            }
            bytes.extend(chunk);
        }
        if !(200..300).contains(&status) {
            return Err(CommandError::transport(format!(
                "Upstream HTTP request returned {status}"
            )));
        }
        Ok(bytes)
    }
}
