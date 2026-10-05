//! Backend-owned HTTP client for the training worker.
//!
//! The worker is a separate Python/PyTorch service. This adapter owns only
//! request encoding and response decoding, keeping that tensor framework
//! outside the Studio process.

use std::time::Duration;

use serde::de::DeserializeOwned;
use spiral_robotics_training::{
    TrainingBackendError, TrainingBackendInfo, TrainingJobHistory, TrainingJobRequest,
    TrainingJobState,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(7);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
pub(crate) struct HttpTrainingBackend {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpTrainingBackend {
    pub(crate) fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::blocking::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("HTTP training client configuration is valid"),
        }
    }

    fn url(&self, path: &str) -> Result<String, TrainingBackendError> {
        let endpoint = self.endpoint.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            return Err(TrainingBackendError::new(
                "training runtime endpoint is empty",
            ));
        }
        Ok(format!("{endpoint}{path}"))
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, TrainingBackendError> {
        let response = self
            .client
            .get(self.url(path)?)
            .timeout(SHORT_REQUEST_TIMEOUT)
            .send()
            .map_err(|error| TrainingBackendError::new(format!("requesting {path}: {error}")))?;
        response.decode_training_json(path)
    }

    /// Run one blocking request against the worker at `endpoint` off the async
    /// executor.
    pub(crate) async fn request<T: Send + 'static>(
        endpoint: String,
        call: impl FnOnce(&Self) -> Result<T, TrainingBackendError> + Send + 'static,
    ) -> Result<T, String> {
        tokio::task::spawn_blocking(move || call(&Self::new(endpoint)))
            .await
            .map_err(|error| format!("training runtime request stopped: {error}"))?
            .map_err(|error| error.to_string())
    }

    pub(crate) fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
        self.get_json("/v1/health")
    }

    pub(crate) fn create_job(
        &self,
        request: &TrainingJobRequest,
    ) -> Result<TrainingJobState, TrainingBackendError> {
        let response = self
            .client
            .post(self.url("/v1/jobs")?)
            .json(request)
            .send()
            .map_err(|error| {
                TrainingBackendError::new(format!("creating training job: {error}"))
            })?;
        response.decode_training_json("creating training job")
    }

    /// Every job the worker still holds.
    pub(crate) fn jobs(&self) -> Result<Vec<TrainingJobState>, TrainingBackendError> {
        self.get_json("/v1/jobs")
    }

    pub(crate) fn job(&self, job_id: &str) -> Result<TrainingJobHistory, TrainingBackendError> {
        self.get_json(&format!("/v1/jobs/{job_id}"))
    }

    pub(crate) fn stop_job(&self, job_id: &str) -> Result<TrainingJobState, TrainingBackendError> {
        let path = format!("/v1/jobs/{job_id}/stop");
        let response = self
            .client
            .post(self.url(&path)?)
            .timeout(SHORT_REQUEST_TIMEOUT)
            .send()
            .map_err(|error| {
                TrainingBackendError::new(format!("stopping training job: {error}"))
            })?;
        response.decode_training_json("stopping training job")
    }
}

trait TrainingHttpResponse {
    fn require_training_success(
        self,
        context: &str,
    ) -> Result<reqwest::blocking::Response, TrainingBackendError>;

    fn decode_training_json<T: DeserializeOwned>(
        self,
        context: &str,
    ) -> Result<T, TrainingBackendError>;
}

impl TrainingHttpResponse for reqwest::blocking::Response {
    fn require_training_success(
        self,
        context: &str,
    ) -> Result<reqwest::blocking::Response, TrainingBackendError> {
        if self.status().is_success() {
            return Ok(self);
        }
        let status = self.status();
        let message = self.text().unwrap_or_default();
        Err(TrainingBackendError::new(format!(
            "{context} failed with HTTP {status}: {message}"
        )))
    }

    fn decode_training_json<T: DeserializeOwned>(
        self,
        context: &str,
    ) -> Result<T, TrainingBackendError> {
        self.require_training_success(context)?
            .json()
            .map_err(|error| {
                TrainingBackendError::new(format!("decoding {context} response: {error}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_runtime_endpoints_are_rejected_before_network_io() {
        assert!(HttpTrainingBackend::new(" ").health().is_err());
    }
}
