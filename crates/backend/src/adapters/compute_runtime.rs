//! Backend-owned HTTP control plane and Arrow IPC data-plane client.
//!
//! The compute runtime is a separate Python/PyTorch service. This adapter owns
//! only request encoding and response decoding, keeping that tensor framework
//! outside the Studio process.

use std::sync::Arc;
use std::time::Duration;

use arrow_array::{ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, UInt8Array};
use arrow_ipc::CompressionType;
use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow_schema::{DataType, Field};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, de::DeserializeOwned};
use spiral_robotics_inference::{
    ActionChunk, ActionChunkShape, ImageFormat, InferenceInput, InferenceStats, ModelArtifact,
    Policy, PolicyLoader, PolicySpecification,
};
use spiral_robotics_training::{
    TrainingBackendError, TrainingBackendInfo, TrainingJobHistory, TrainingJobRequest,
    TrainingJobState,
};

const ARROW_STREAM_CONTENT_TYPE: &str = "application/vnd.apache.arrow.stream";
const POLICY_ARTIFACT_CONTENT_TYPE: &str = "application/vnd.spiral.policy-artifact";
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

/// Materializes a policy on the selected remote compute runtime.
#[derive(Clone, Debug)]
pub(crate) struct HttpPolicyLoader {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpPolicyLoader {
    pub(crate) fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::blocking::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("HTTP policy client configuration is valid"),
        }
    }

    fn url(&self, path: &str) -> spiral_robotics_inference::InferenceResult<String> {
        let endpoint = self.endpoint.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            return Err(spiral_robotics_inference::InferenceError::Inference(
                "policy runtime endpoint is empty".to_owned(),
            ));
        }
        Ok(format!("{endpoint}{path}"))
    }
}

impl PolicyLoader for HttpPolicyLoader {
    fn load(
        &self,
        artifact: ModelArtifact,
    ) -> spiral_robotics_inference::InferenceResult<Box<dyn Policy>> {
        let body = artifact.http_envelope()?;
        let response = self
            .client
            .post(self.url("/v1/policies")?)
            .header(CONTENT_TYPE, POLICY_ARTIFACT_CONTENT_TYPE)
            .body(body)
            .send()
            .map_err(|error| {
                spiral_robotics_inference::InferenceError::Inference(format!(
                    "creating policy session: {error}"
                ))
            })?;
        let response: CreatePolicyResponse =
            response.decode_inference_json("creating policy session")?;
        Ok(Box::new(HttpPolicy {
            backend: self.clone(),
            session_id: response.session_id,
            specification: response.specification,
        }))
    }
}

struct HttpPolicy {
    backend: HttpPolicyLoader,
    session_id: String,
    specification: PolicySpecification,
}

impl HttpPolicy {
    fn path(&self, suffix: &str) -> String {
        format!("/v1/policies/{}{suffix}", self.session_id)
    }
}

impl Policy for HttpPolicy {
    fn specification(&self) -> &PolicySpecification {
        &self.specification
    }

    fn warm_up(&mut self) -> spiral_robotics_inference::InferenceResult<InferenceStats> {
        let path = self.path("/warm-up");
        let response = self
            .backend
            .client
            .post(self.backend.url(&path)?)
            .send()
            .map_err(|error| {
                spiral_robotics_inference::InferenceError::Inference(format!(
                    "warming up policy: {error}"
                ))
            })?;
        let response: InferenceStatsResponse =
            response.decode_inference_json("warming up policy")?;
        response.into_stats()
    }

    fn infer(
        &mut self,
        input: InferenceInput<'_>,
    ) -> spiral_robotics_inference::InferenceResult<ActionChunk> {
        let body = input.http_envelope(&self.specification)?;
        let path = self.path("/infer");
        let response = self
            .backend
            .client
            .post(self.backend.url(&path)?)
            .header(CONTENT_TYPE, ARROW_STREAM_CONTENT_TYPE)
            .body(body)
            .send()
            .map_err(|error| {
                spiral_robotics_inference::InferenceError::Inference(format!(
                    "running policy inference: {error}"
                ))
            })?;
        let response: InferenceResponse =
            response.decode_inference_json("running policy inference")?;
        response.into_chunk()
    }
}

impl Drop for HttpPolicy {
    fn drop(&mut self) {
        if let Ok(url) = self
            .backend
            .url(&format!("/v1/policies/{}", self.session_id))
        {
            let _ = self
                .backend
                .client
                .delete(url)
                .timeout(SHORT_REQUEST_TIMEOUT)
                .send();
        }
    }
}

#[derive(Deserialize)]
struct CreatePolicyResponse {
    session_id: String,
    specification: PolicySpecification,
}

#[derive(Deserialize)]
struct InferenceStatsResponse {
    preprocessing_ms: f64,
    model_ms: f64,
    postprocessing_ms: f64,
    total_ms: f64,
}

impl InferenceStatsResponse {
    fn into_stats(self) -> spiral_robotics_inference::InferenceResult<InferenceStats> {
        Ok(InferenceStats::default()
            .with_preprocessing(self.preprocessing_ms.inference_duration()?)
            .with_model(self.model_ms.inference_duration()?)
            .with_postprocessing(self.postprocessing_ms.inference_duration()?)
            .with_total(self.total_ms.inference_duration()?))
    }
}

#[derive(Deserialize)]
struct InferenceResponse {
    values: Vec<f32>,
    steps: usize,
    action_dimensions: usize,
    stats: InferenceStatsResponse,
}

impl InferenceResponse {
    fn into_chunk(self) -> spiral_robotics_inference::InferenceResult<ActionChunk> {
        ActionChunk::try_new(
            self.values,
            ActionChunkShape::try_new(self.steps, self.action_dimensions)?,
            self.stats.into_stats()?,
        )
    }
}

trait PolicyArtifactHttp {
    fn http_envelope(&self) -> spiral_robotics_inference::InferenceResult<Vec<u8>>;
}

impl PolicyArtifactHttp for ModelArtifact {
    fn http_envelope(&self) -> spiral_robotics_inference::InferenceResult<Vec<u8>> {
        let config_len = u64::try_from(self.config().len()).map_err(|_| {
            spiral_robotics_inference::InferenceError::Inference(
                "model config is too large for the HTTP envelope".to_owned(),
            )
        })?;
        let capacity = 8_usize
            .checked_add(self.config().len())
            .and_then(|size| size.checked_add(self.weights().len()))
            .ok_or_else(|| {
                spiral_robotics_inference::InferenceError::Inference(
                    "policy artifact size overflowed".to_owned(),
                )
            })?;
        let mut body = Vec::with_capacity(capacity);
        body.extend_from_slice(&config_len.to_le_bytes());
        body.extend_from_slice(self.config());
        body.extend_from_slice(self.weights());
        Ok(body)
    }
}

trait InferenceInputHttp {
    fn http_envelope(
        self,
        specification: &PolicySpecification,
    ) -> spiral_robotics_inference::InferenceResult<Vec<u8>>;
}

impl InferenceInputHttp for InferenceInput<'_> {
    fn http_envelope(
        self,
        specification: &PolicySpecification,
    ) -> spiral_robotics_inference::InferenceResult<Vec<u8>> {
        if self.state().len() != specification.state_dimensions() {
            return Err(spiral_robotics_inference::InferenceError::Input(format!(
                "policy state has {} values, expected {}",
                self.state().len(),
                specification.state_dimensions()
            )));
        }
        if self.images().len() != specification.image_inputs().len() {
            return Err(spiral_robotics_inference::InferenceError::Input(format!(
                "policy input has {} images, expected {}",
                self.images().len(),
                specification.image_inputs().len()
            )));
        }
        let mut arrays = Vec::<(String, ArrayRef)>::with_capacity(self.images().len() + 1);
        arrays.push((
            "state".to_owned(),
            FixedListLayout::new(DataType::Float32, 1, "state")
                .build(Arc::new(Float32Array::from(self.state().to_vec())))
                .map_err(|error| {
                    spiral_robotics_inference::InferenceError::Inference(error.to_string())
                })?,
        ));
        for (index, (image, expected)) in self
            .images()
            .iter()
            .zip(specification.image_inputs())
            .enumerate()
        {
            let dimensions = expected.dimensions();
            let packed_stride = dimensions
                .width()
                .checked_mul(expected.channels())
                .ok_or_else(|| {
                    spiral_robotics_inference::InferenceError::Inference(
                        "policy image row stride overflowed".to_owned(),
                    )
                })?;
            let expected_len = packed_stride
                .checked_mul(dimensions.height())
                .ok_or_else(|| {
                    spiral_robotics_inference::InferenceError::Inference(
                        "policy image size overflowed".to_owned(),
                    )
                })?;
            if image.name() != expected.name()
                || image.dimensions() != dimensions
                || image.format() != ImageFormat::Rgb8
                || expected.channels() != 3
                || image.row_stride() != packed_stride
                || image.data().len() != expected_len
            {
                return Err(spiral_robotics_inference::InferenceError::Input(format!(
                    "image {:?} does not match policy input {:?} ({}x{} RGB8)",
                    image.name(),
                    expected.name(),
                    dimensions.width(),
                    dimensions.height()
                )));
            }
            arrays.push((
                format!("image_{index}"),
                FixedListLayout::new(DataType::UInt8, 1, &format!("image_{index}"))
                    .build(Arc::new(UInt8Array::from(image.data().to_vec())))
                    .map_err(|error| {
                        spiral_robotics_inference::InferenceError::Inference(error.to_string())
                    })?,
            ));
        }
        let record = RecordBatch::try_from_iter(
            arrays
                .iter()
                .map(|(name, array)| (name.as_str(), array.clone())),
        )
        .map_err(|error| {
            spiral_robotics_inference::InferenceError::Inference(format!(
                "building Arrow inference input: {error}"
            ))
        })?;
        record
            .arrow_stream()
            .map_err(spiral_robotics_inference::InferenceError::Inference)
    }
}

trait ArrowRecordHttp {
    fn arrow_stream(&self) -> Result<Vec<u8>, String>;
}

impl ArrowRecordHttp for RecordBatch {
    fn arrow_stream(&self) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        let options = IpcWriteOptions::default()
            .try_with_compression(Some(CompressionType::ZSTD))
            .map_err(|error| format!("configuring Arrow IPC compression: {error}"))?;
        let mut writer = StreamWriter::try_new_with_options(&mut body, &self.schema(), options)
            .map_err(|error| format!("starting Arrow IPC stream: {error}"))?;
        writer
            .write(self)
            .map_err(|error| format!("writing Arrow IPC stream: {error}"))?;
        writer
            .finish()
            .map_err(|error| format!("finishing Arrow IPC stream: {error}"))?;
        drop(writer);
        Ok(body)
    }
}

struct FixedListLayout<'a> {
    data_type: DataType,
    batch_size: usize,
    name: &'a str,
}

impl<'a> FixedListLayout<'a> {
    fn new(data_type: DataType, batch_size: usize, name: &'a str) -> Self {
        Self {
            data_type,
            batch_size,
            name,
        }
    }

    fn build(self, values: ArrayRef) -> Result<ArrayRef, String> {
        if self.batch_size == 0 || !values.len().is_multiple_of(self.batch_size) {
            return Err(format!(
                "{} has {} values for batch size {}",
                self.name,
                values.len(),
                self.batch_size
            ));
        }
        let width = values.len() / self.batch_size;
        let width =
            i32::try_from(width).map_err(|_| format!("{} row is too wide for Arrow", self.name))?;
        FixedSizeListArray::try_new(
            Arc::new(Field::new("item", self.data_type, false)),
            width,
            values,
            None,
        )
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| format!("building {} Arrow tensor: {error}", self.name))
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

trait InferenceHttpResponse {
    fn require_inference_success(
        self,
        context: &str,
    ) -> spiral_robotics_inference::InferenceResult<reqwest::blocking::Response>;

    fn decode_inference_json<T: DeserializeOwned>(
        self,
        context: &str,
    ) -> spiral_robotics_inference::InferenceResult<T>;
}

impl InferenceHttpResponse for reqwest::blocking::Response {
    fn require_inference_success(
        self,
        context: &str,
    ) -> spiral_robotics_inference::InferenceResult<reqwest::blocking::Response> {
        if self.status().is_success() {
            return Ok(self);
        }
        let status = self.status();
        let message = self.text().unwrap_or_default();
        Err(spiral_robotics_inference::InferenceError::Inference(
            format!("{context} failed with HTTP {status}: {message}"),
        ))
    }

    fn decode_inference_json<T: DeserializeOwned>(
        self,
        context: &str,
    ) -> spiral_robotics_inference::InferenceResult<T> {
        self.require_inference_success(context)?
            .json()
            .map_err(|error| {
                spiral_robotics_inference::InferenceError::Inference(format!(
                    "decoding {context} response: {error}"
                ))
            })
    }
}

trait InferenceDuration {
    fn inference_duration(self) -> spiral_robotics_inference::InferenceResult<Duration>;
}

impl InferenceDuration for f64 {
    fn inference_duration(self) -> spiral_robotics_inference::InferenceResult<Duration> {
        Duration::try_from_secs_f64(self / 1_000.0).map_err(|error| {
            spiral_robotics_inference::InferenceError::Inference(format!(
                "invalid inference duration: {error}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_runtime_endpoints_are_rejected_before_network_io() {
        assert!(HttpTrainingBackend::new(" ").health().is_err());
        assert!(HttpPolicyLoader::new("").url("/v1/policies").is_err());
    }
}
