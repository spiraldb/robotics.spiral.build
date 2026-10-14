//! OpenRouter, which Break down and the judgments of its subtasks ask their
//! models through, with the deployment's key as a bearer: a chat completion
//! for each episode Break down reads, and Decisions API requests for the
//! judgments.
//!
//! A response means what OpenRouter reports, which also puts some failures
//! inside a 200 body, as an `error` whose code is the status it stands for.
//! A refused key (401) or an account out of credit (402) would fail every
//! episode alike, so it ends the run. A status that may pass (408, 429, 500,
//! 502, 503, 504), a timeout, or no connection is tried again after a pause,
//! the one OpenRouter names in `Retry-After` when it names one; anything
//! else, or a request still failing after its last attempt, fails only what
//! it asked about.
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;
use spiral_robotics_storage::ProviderKeys;
use std::time::Duration;

/// The API root both routes hang off.
const ENDPOINT: &str = "https://openrouter.ai/api";
const CHAT_COMPLETIONS: &str = "/v1/chat/completions";
const DECISIONS: &str = "/alpha/decisions";
/// A multimodal answer over several contact sheets can take tens of seconds.
const CHAT_TIMEOUT: Duration = Duration::from_secs(90);
/// jev evaluates a request's questions in parallel; one request is usually
/// sub-second.
const DECISIONS_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of OpenRouter's error message a failure repeats.
const REASON_CHARS: usize = 512;

/// Why a request produced no answer.
#[derive(Debug, PartialEq)]
pub(super) enum ProviderFailure {
    /// The key can make no request, so every episode would fail alike.
    Account(String),
    /// This request failed; others may still be answered.
    Request(String),
}

/// How often a request that may pass is sent, and how long to wait between
/// attempts.
#[derive(Clone, Copy, Debug)]
struct RetryPolicy {
    attempts: u32,
    first_pause: Duration,
    longest_pause: Duration,
}

impl RetryPolicy {
    /// Four attempts, the first pause a second and each later one twice as
    /// long, up to half a minute.
    const OPENROUTER: Self = Self {
        attempts: 4,
        first_pause: Duration::from_secs(1),
        longest_pause: Duration::from_secs(30),
    };

    /// The pause after attempt `attempt`, unless OpenRouter named its own.
    fn pause(self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        retry_after
            .unwrap_or_else(|| self.first_pause.saturating_mul(1 << (attempt - 1).min(16)))
            .min(self.longest_pause)
    }
}

/// OpenRouter as the provider keys reach it.
pub(super) struct OpenRouter {
    http: reqwest::Client,
    endpoint: String,
    key: String,
    retry: RetryPolicy,
}

impl OpenRouter {
    /// OpenRouter with the deployment's key, which `node` needs; a missing
    /// one is said plainly.
    pub(super) fn of(keys: &ProviderKeys, node: &str) -> Result<Self, String> {
        let key = keys.llm_api_key().ok_or_else(|| {
            format!(
                "{node} needs OpenRouter, which this Studio was started without: set OPENROUTER_API_KEY."
            )
        })?;
        Self::reaching(ENDPOINT, key, RetryPolicy::OPENROUTER)
    }

    fn reaching(endpoint: &str, key: &str, retry: RetryPolicy) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            retry,
        })
    }

    /// A non-streaming chat completion: the first choice's text.
    pub(super) async fn chat_completion(&self, body: &Value) -> Result<String, ProviderFailure> {
        let answer = self.post(CHAT_COMPLETIONS, body, CHAT_TIMEOUT).await?;
        answer
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ProviderFailure::Request("the model answered with no message text".to_owned())
            })
    }

    /// One Decisions request: jev's answers, keyed by question id.
    pub(super) async fn decisions(&self, body: &Value) -> Result<Value, ProviderFailure> {
        self.post(DECISIONS, body, DECISIONS_TIMEOUT).await
    }

    async fn post(
        &self,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, ProviderFailure> {
        let mut attempt = 1;
        loop {
            match self.send(path, body, timeout).await {
                Attempt::Answered(answer) => return Ok(answer),
                Attempt::Failed(failure) => return Err(failure),
                Attempt::Retriable { reason, .. } if attempt == self.retry.attempts => {
                    return Err(ProviderFailure::Request(format!(
                        "{reason}, after {attempt} attempts"
                    )));
                }
                Attempt::Retriable { retry_after, .. } => {
                    tokio::time::sleep(self.retry.pause(attempt, retry_after)).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn send(&self, path: &str, body: &Value, timeout: Duration) -> Attempt {
        let sent = self
            .http
            .post(format!("{}{path}", self.endpoint))
            .bearer_auth(&self.key)
            .timeout(timeout)
            .json(body)
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) if error.is_timeout() || error.is_connect() || error.is_request() => {
                return Attempt::Retriable {
                    reason: format!("OpenRouter request failed: {error}"),
                    retry_after: None,
                };
            }
            Err(error) => {
                return Attempt::Failed(ProviderFailure::Request(format!(
                    "OpenRouter request failed: {error}"
                )));
            }
        };
        let status = response.status();
        let retry_after = Self::retry_after(response.headers());
        match response.text().await {
            Ok(text) => Attempt::from_response(status, retry_after, &text),
            Err(error) => Attempt::Retriable {
                reason: format!("reading the OpenRouter response failed: {error}"),
                retry_after,
            },
        }
    }

    fn retry_after(headers: &HeaderMap) -> Option<Duration> {
        headers
            .get(RETRY_AFTER)?
            .to_str()
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(Duration::from_secs_f64)
    }
}

/// What one send produced.
enum Attempt {
    Answered(Value),
    Retriable {
        reason: String,
        retry_after: Option<Duration>,
    },
    Failed(ProviderFailure),
}

impl Attempt {
    fn from_response(status: StatusCode, retry_after: Option<Duration>, text: &str) -> Self {
        let answer = serde_json::from_str::<Value>(text).ok();
        let error = answer.as_ref().and_then(|answer| answer.get("error"));
        if status.is_success() && error.is_none() {
            return match answer {
                Some(answer) => Self::Answered(answer),
                None => Self::Failed(ProviderFailure::Request(format!(
                    "OpenRouter answered with invalid JSON: {}",
                    Self::excerpt(text)
                ))),
            };
        }
        let status = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_u64)
            .and_then(|code| u16::try_from(code).ok())
            .and_then(|code| StatusCode::from_u16(code).ok())
            .filter(|_| status.is_success())
            .unwrap_or(status);
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(text);
        let reason = format!(
            "OpenRouter returned HTTP {}: {}",
            status.as_u16(),
            Self::excerpt(message)
        );
        match status.as_u16() {
            401 | 402 => Self::Failed(ProviderFailure::Account(reason)),
            408 | 429 | 500 | 502 | 503 | 504 => Self::Retriable {
                reason,
                retry_after,
            },
            _ => Self::Failed(ProviderFailure::Request(reason)),
        }
    }

    fn excerpt(text: &str) -> String {
        text.trim().chars().take(REASON_CHARS).collect()
    }
}

#[cfg(test)]
pub(super) mod tests;
