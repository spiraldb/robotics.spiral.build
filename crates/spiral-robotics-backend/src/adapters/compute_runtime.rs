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
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingJobCursor,
    TrainingJobRequest, TrainingJobSnapshot,
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
            return Err(backend_error("training runtime endpoint is empty"));
        }
        Ok(format!("{endpoint}{path}"))
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, TrainingBackendError> {
        let response = self
            .client
            .get(self.url(path)?)
            .timeout(SHORT_REQUEST_TIMEOUT)
            .send()
            .map_err(|error| backend_error(format!("requesting {path}: {error}")))?;
        decode_json_response(response, path)
    }
}

impl TrainingBackend for HttpTrainingBackend {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
        self.get_json("/v1/health")
    }

    fn create_job(
        &self,
        request: TrainingJobRequest,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        let response = self
            .client
            .post(self.url("/v1/jobs")?)
            .json(&request)
            .send()
            .map_err(|error| backend_error(format!("creating training job: {error}")))?;
        decode_json_response(response, "creating training job")
    }

    fn job_snapshot(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.get_json(&job_path(job_id, "", cursor))
    }

    fn stop_job(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        let path = job_path(job_id, "/stop", cursor);
        let response = self
            .client
            .post(self.url(&path)?)
            .timeout(SHORT_REQUEST_TIMEOUT)
            .send()
            .map_err(|error| backend_error(format!("stopping training job: {error}")))?;
        decode_json_response(response, "stopping training job")
    }
}

fn job_path(job_id: &str, suffix: &str, cursor: TrainingJobCursor) -> String {
    format!(
        "/v1/jobs/{job_id}{suffix}?after_metric_step={}&after_checkpoint_step={}&after_log_offset={}",
        cursor.metric_step(),
        cursor.checkpoint_step(),
        cursor.log_offset()
    )
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
            return Err(inference_error("policy runtime endpoint is empty"));
        }
        Ok(format!("{endpoint}{path}"))
    }
}

impl PolicyLoader for HttpPolicyLoader {
    fn load(
        &self,
        artifact: ModelArtifact,
    ) -> spiral_robotics_inference::InferenceResult<Box<dyn Policy>> {
        let body = encode_policy_artifact(&artifact)?;
        let response = self
            .client
            .post(self.url("/v1/policies")?)
            .header(CONTENT_TYPE, POLICY_ARTIFACT_CONTENT_TYPE)
            .body(body)
            .send()
            .map_err(|error| inference_error(format!("creating policy session: {error}")))?;
        let response: CreatePolicyResponse =
            decode_inference_json_response(response, "creating policy session")?;
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
            .map_err(|error| inference_error(format!("warming up policy: {error}")))?;
        let response: InferenceStatsResponse =
            decode_inference_json_response(response, "warming up policy")?;
        response.into_stats()
    }

    fn infer(
        &mut self,
        input: InferenceInput<'_>,
    ) -> spiral_robotics_inference::InferenceResult<ActionChunk> {
        let body = encode_inference_input(input, &self.specification)?;
        let path = self.path("/infer");
        let response = self
            .backend
            .client
            .post(self.backend.url(&path)?)
            .header(CONTENT_TYPE, ARROW_STREAM_CONTENT_TYPE)
            .body(body)
            .send()
            .map_err(|error| inference_error(format!("running policy inference: {error}")))?;
        let response: InferenceResponse =
            decode_inference_json_response(response, "running policy inference")?;
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
            .with_preprocessing(duration_from_millis(self.preprocessing_ms)?)
            .with_model(duration_from_millis(self.model_ms)?)
            .with_postprocessing(duration_from_millis(self.postprocessing_ms)?)
            .with_total(duration_from_millis(self.total_ms)?))
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

fn encode_policy_artifact(
    artifact: &ModelArtifact,
) -> spiral_robotics_inference::InferenceResult<Vec<u8>> {
    let config_len = u64::try_from(artifact.config().len())
        .map_err(|_| inference_error("model config is too large for the HTTP envelope"))?;
    let capacity = 8_usize
        .checked_add(artifact.config().len())
        .and_then(|size| size.checked_add(artifact.weights().len()))
        .ok_or_else(|| inference_error("policy artifact size overflowed"))?;
    let mut body = Vec::with_capacity(capacity);
    body.extend_from_slice(&config_len.to_le_bytes());
    body.extend_from_slice(artifact.config());
    body.extend_from_slice(artifact.weights());
    Ok(body)
}

fn encode_inference_input(
    input: InferenceInput<'_>,
    specification: &PolicySpecification,
) -> spiral_robotics_inference::InferenceResult<Vec<u8>> {
    if input.state().len() != specification.state_dimensions() {
        return Err(spiral_robotics_inference::InferenceError::Input(format!(
            "policy state has {} values, expected {}",
            input.state().len(),
            specification.state_dimensions()
        )));
    }
    if input.images().len() != specification.image_inputs().len() {
        return Err(spiral_robotics_inference::InferenceError::Input(format!(
            "policy input has {} images, expected {}",
            input.images().len(),
            specification.image_inputs().len()
        )));
    }
    let mut arrays = Vec::<(String, ArrayRef)>::with_capacity(input.images().len() + 1);
    arrays.push((
        "state".to_owned(),
        fixed_f32(input.state().to_vec(), 1, "state")
            .map_err(|error| inference_error(error.to_string()))?,
    ));
    for (index, (image, expected)) in input
        .images()
        .iter()
        .zip(specification.image_inputs())
        .enumerate()
    {
        let dimensions = expected.dimensions();
        let packed_stride = dimensions
            .width()
            .checked_mul(expected.channels())
            .ok_or_else(|| inference_error("policy image row stride overflowed"))?;
        let expected_len = packed_stride
            .checked_mul(dimensions.height())
            .ok_or_else(|| inference_error("policy image size overflowed"))?;
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
            fixed_u8(image.data().to_vec(), 1, &format!("image_{index}"))
                .map_err(|error| inference_error(error.to_string()))?,
        ));
    }
    let record = RecordBatch::try_from_iter(
        arrays
            .iter()
            .map(|(name, array)| (name.as_str(), array.clone())),
    )
    .map_err(|error| inference_error(format!("building Arrow inference input: {error}")))?;
    encode_arrow_record(&record).map_err(inference_error)
}

fn encode_arrow_record(record: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let options = IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::ZSTD))
        .map_err(|error| format!("configuring Arrow IPC compression: {error}"))?;
    let mut writer = StreamWriter::try_new_with_options(&mut body, &record.schema(), options)
        .map_err(|error| format!("starting Arrow IPC stream: {error}"))?;
    writer
        .write(record)
        .map_err(|error| format!("writing Arrow IPC stream: {error}"))?;
    writer
        .finish()
        .map_err(|error| format!("finishing Arrow IPC stream: {error}"))?;
    drop(writer);
    Ok(body)
}

fn fixed_f32(values: Vec<f32>, batch_size: usize, name: &str) -> Result<ArrayRef, String> {
    FixedListLayout::new(DataType::Float32, batch_size, name)
        .build(Arc::new(Float32Array::from(values)))
}

fn fixed_u8(values: Vec<u8>, batch_size: usize, name: &str) -> Result<ArrayRef, String> {
    FixedListLayout::new(DataType::UInt8, batch_size, name)
        .build(Arc::new(UInt8Array::from(values)))
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

fn decode_json_response<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
    context: &str,
) -> Result<T, TrainingBackendError> {
    require_success(response, context)?
        .json()
        .map_err(|error| backend_error(format!("decoding {context} response: {error}")))
}

fn decode_inference_json_response<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
    context: &str,
) -> spiral_robotics_inference::InferenceResult<T> {
    require_inference_success(response, context)?
        .json()
        .map_err(|error| inference_error(format!("decoding {context} response: {error}")))
}

fn require_inference_success(
    response: reqwest::blocking::Response,
    context: &str,
) -> spiral_robotics_inference::InferenceResult<reqwest::blocking::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response.text().unwrap_or_default();
    Err(inference_error(format!(
        "{context} failed with HTTP {status}: {message}"
    )))
}

fn require_success(
    response: reqwest::blocking::Response,
    context: &str,
) -> Result<reqwest::blocking::Response, TrainingBackendError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response.text().unwrap_or_default();
    Err(backend_error(format!(
        "{context} failed with HTTP {status}: {message}"
    )))
}

fn backend_error(message: impl Into<String>) -> TrainingBackendError {
    TrainingBackendError::new(message)
}

fn inference_error(message: impl Into<String>) -> spiral_robotics_inference::InferenceError {
    spiral_robotics_inference::InferenceError::Inference(message.into())
}

fn duration_from_millis(milliseconds: f64) -> spiral_robotics_inference::InferenceResult<Duration> {
    Duration::try_from_secs_f64(milliseconds / 1_000.0)
        .map_err(|error| inference_error(format!("invalid inference duration: {error}")))
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
