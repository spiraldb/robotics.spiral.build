//! Studio-side HTTP control plane and Arrow IPC data-plane client.
//!
//! The compute runtime is a separate Python/PyTorch service. This crate owns
//! only framework-neutral request encoding and response decoding, so linking
//! Studio can never pull a tensor framework into its process.

use std::sync::Arc;
use std::time::Duration;

use arrow_array::{ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, UInt8Array};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, de::DeserializeOwned};
use spiral_robotics_inference::{
    ActionChunk, ImageFormat, InferenceInput, InferenceStats, ModelArtifact, ModelIdentity, Policy,
    PolicyLoader, PolicySpecification,
};
use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingBatch,
    TrainingCheckpointArtifact, TrainingCheckpointRequest, TrainingLoss, TrainingSession,
    TrainingSessionInfo, TrainingSessionRequest, TrainingTelemetry,
};

const ARROW_STREAM_CONTENT_TYPE: &str = "application/vnd.apache.arrow.stream";
const POLICY_ARTIFACT_CONTENT_TYPE: &str = "application/vnd.spiral.policy-artifact";
const CHECKPOINT_FORMAT_HEADER: &str = "x-spiral-checkpoint-format";
const CHECKPOINT_EXTENSION_HEADER: &str = "x-spiral-checkpoint-extension";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(7);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
pub struct HttpTrainingBackend {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpTrainingBackend {
    pub fn new(endpoint: impl Into<String>) -> Self {
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

    fn create_session(
        &self,
        request: TrainingSessionRequest,
    ) -> Result<Box<dyn TrainingSession>, TrainingBackendError> {
        let response = self
            .client
            .post(self.url("/v1/sessions")?)
            .json(&request)
            .send()
            .map_err(|error| backend_error(format!("creating training session: {error}")))?;
        let response: CreateSessionResponse = decode_json_response(response, "creating session")?;
        Ok(Box::new(HttpTrainingSession {
            backend: self.clone(),
            session_id: response.session_id,
            info: response.info,
            last_learning_rate: 0.0,
            last_telemetry: None,
        }))
    }
}

/// Materializes a policy on the selected remote compute runtime.
#[derive(Clone, Debug)]
pub struct HttpPolicyLoader {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpPolicyLoader {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::blocking::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("HTTP policy client configuration is valid"),
        }
    }

    fn url(&self, path: &str) -> spiral_robotics_inference::Result<String> {
        let endpoint = self.endpoint.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            return Err(inference_error("policy runtime endpoint is empty"));
        }
        Ok(format!("{endpoint}{path}"))
    }
}

impl PolicyLoader for HttpPolicyLoader {
    fn load(&self, artifact: ModelArtifact) -> spiral_robotics_inference::Result<Box<dyn Policy>> {
        let identity = artifact.identity().clone();
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
            identity,
            specification: response.specification,
        }))
    }
}

struct HttpPolicy {
    backend: HttpPolicyLoader,
    session_id: String,
    identity: ModelIdentity,
    specification: PolicySpecification,
}

impl HttpPolicy {
    fn path(&self, suffix: &str) -> String {
        format!("/v1/policies/{}{suffix}", self.session_id)
    }
}

impl Policy for HttpPolicy {
    fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    fn specification(&self) -> &PolicySpecification {
        &self.specification
    }

    fn warm_up(&mut self) -> spiral_robotics_inference::Result<InferenceStats> {
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
    ) -> spiral_robotics_inference::Result<ActionChunk> {
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

struct HttpTrainingSession {
    backend: HttpTrainingBackend,
    session_id: String,
    info: TrainingSessionInfo,
    last_learning_rate: f64,
    last_telemetry: Option<TrainingTelemetry>,
}

impl HttpTrainingSession {
    fn path(&self, suffix: &str) -> String {
        format!("/v1/sessions/{}{suffix}", self.session_id)
    }

    fn step(
        &mut self,
        suffix: &str,
        batch: TrainingBatch<'_>,
    ) -> Result<TrainingLoss, TrainingBackendError> {
        let body = encode_training_batch(batch)?;
        let path = self.path(suffix);
        let response = self
            .backend
            .client
            .post(self.backend.url(&path)?)
            .header(CONTENT_TYPE, ARROW_STREAM_CONTENT_TYPE)
            .body(body)
            .send()
            .map_err(|error| backend_error(format!("sending optimizer batch: {error}")))?;
        let response: StepResponse = decode_json_response(response, "running optimizer step")?;
        self.last_learning_rate = response.learning_rate;
        self.last_telemetry = Some(response.telemetry);
        Ok(response.loss)
    }
}

impl TrainingSession for HttpTrainingSession {
    fn info(&self) -> &TrainingSessionInfo {
        &self.info
    }

    fn learning_rate(&self, _step: u64) -> f64 {
        self.last_learning_rate
    }

    fn telemetry(&self) -> Result<TrainingTelemetry, TrainingBackendError> {
        self.last_telemetry
            .map_or_else(|| self.backend.get_json(&self.path("/telemetry")), Ok)
    }

    fn train_step(
        &mut self,
        batch: TrainingBatch<'_>,
        step: u64,
    ) -> Result<TrainingLoss, TrainingBackendError> {
        self.step(&format!("/train?step={step}"), batch)
    }

    fn validation_step(
        &mut self,
        batch: TrainingBatch<'_>,
    ) -> Result<TrainingLoss, TrainingBackendError> {
        self.step("/validate", batch)
    }

    fn checkpoint(
        &self,
        request: TrainingCheckpointRequest,
    ) -> Result<TrainingCheckpointArtifact, TrainingBackendError> {
        let path = self.path("/checkpoint");
        let response = self
            .backend
            .client
            .post(self.backend.url(&path)?)
            .json(&request)
            .send()
            .map_err(|error| backend_error(format!("requesting checkpoint: {error}")))?;
        let response = require_success(response, "requesting checkpoint")?;
        let format = response_header(&response, CHECKPOINT_FORMAT_HEADER)?;
        let extension = response_header(&response, CHECKPOINT_EXTENSION_HEADER)?;
        let payload = response
            .bytes()
            .map_err(|error| backend_error(format!("reading checkpoint: {error}")))?
            .to_vec();
        Ok(TrainingCheckpointArtifact {
            format,
            extension,
            payload,
        })
    }
}

impl Drop for HttpTrainingSession {
    fn drop(&mut self) {
        if let Ok(url) = self
            .backend
            .url(&format!("/v1/sessions/{}", self.session_id))
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
struct CreateSessionResponse {
    session_id: String,
    info: TrainingSessionInfo,
}

#[derive(Deserialize)]
struct StepResponse {
    loss: TrainingLoss,
    learning_rate: f64,
    telemetry: TrainingTelemetry,
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
    fn into_stats(self) -> spiral_robotics_inference::Result<InferenceStats> {
        Ok(InferenceStats {
            preprocessing: duration_from_millis(self.preprocessing_ms)?,
            model: duration_from_millis(self.model_ms)?,
            postprocessing: duration_from_millis(self.postprocessing_ms)?,
            total: duration_from_millis(self.total_ms)?,
        })
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
    fn into_chunk(self) -> spiral_robotics_inference::Result<ActionChunk> {
        ActionChunk::try_new(
            self.values,
            self.steps,
            self.action_dimensions,
            self.stats.into_stats()?,
        )
    }
}

fn encode_policy_artifact(artifact: &ModelArtifact) -> spiral_robotics_inference::Result<Vec<u8>> {
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
) -> spiral_robotics_inference::Result<Vec<u8>> {
    if input.state.len() != specification.state_dimensions {
        return Err(spiral_robotics_inference::Error::Input(format!(
            "policy state has {} values, expected {}",
            input.state.len(),
            specification.state_dimensions
        )));
    }
    if input.images.len() != specification.image_inputs.len() {
        return Err(spiral_robotics_inference::Error::Input(format!(
            "policy input has {} images, expected {}",
            input.images.len(),
            specification.image_inputs.len()
        )));
    }
    let mut arrays = Vec::<(String, ArrayRef)>::with_capacity(input.images.len() + 1);
    arrays.push((
        "state".to_owned(),
        fixed_f32(input.state.to_vec(), 1, "state")
            .map_err(|error| inference_error(error.to_string()))?,
    ));
    for (index, (image, expected)) in input
        .images
        .iter()
        .zip(&specification.image_inputs)
        .enumerate()
    {
        let packed_stride = expected
            .width
            .checked_mul(expected.channels)
            .ok_or_else(|| inference_error("policy image row stride overflowed"))?;
        let expected_len = packed_stride
            .checked_mul(expected.height)
            .ok_or_else(|| inference_error("policy image size overflowed"))?;
        if image.name != expected.name
            || image.width != expected.width
            || image.height != expected.height
            || image.format != ImageFormat::Rgb8
            || expected.channels != 3
            || image.row_stride != packed_stride
            || image.data.len() != expected_len
        {
            return Err(spiral_robotics_inference::Error::Input(format!(
                "image {:?} does not match policy input {:?} ({}x{} RGB8)",
                image.name, expected.name, expected.width, expected.height
            )));
        }
        arrays.push((
            format!("image_{index}"),
            fixed_u8(image.data.to_vec(), 1, &format!("image_{index}"))
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

fn encode_training_batch(batch: TrainingBatch<'_>) -> Result<Vec<u8>, TrainingBackendError> {
    let TrainingBatch {
        batch_size,
        states,
        images,
        actions,
        action_is_pad,
    } = batch;
    let arrays = [
        (
            "states",
            fixed_f32(states.into_owned(), batch_size, "states")?,
        ),
        (
            "images",
            fixed_f32(images.into_owned(), batch_size, "images")?,
        ),
        (
            "actions",
            fixed_f32(actions.into_owned(), batch_size, "actions")?,
        ),
        (
            "action_is_pad",
            fixed_u8(action_is_pad.into_owned(), batch_size, "action_is_pad")?,
        ),
    ];
    let record = RecordBatch::try_from_iter(arrays)
        .map_err(|error| backend_error(format!("building Arrow training batch: {error}")))?;
    encode_arrow_record(&record).map_err(backend_error)
}

fn encode_arrow_record(record: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut writer = StreamWriter::try_new(&mut body, &record.schema())
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

fn fixed_f32(
    values: Vec<f32>,
    batch_size: usize,
    name: &str,
) -> Result<ArrayRef, TrainingBackendError> {
    fixed_list(
        Arc::new(Float32Array::from(values)),
        DataType::Float32,
        batch_size,
        name,
    )
}

fn fixed_u8(
    values: Vec<u8>,
    batch_size: usize,
    name: &str,
) -> Result<ArrayRef, TrainingBackendError> {
    fixed_list(
        Arc::new(UInt8Array::from(values)),
        DataType::UInt8,
        batch_size,
        name,
    )
}

fn fixed_list(
    values: ArrayRef,
    data_type: DataType,
    batch_size: usize,
    name: &str,
) -> Result<ArrayRef, TrainingBackendError> {
    if batch_size == 0 || !values.len().is_multiple_of(batch_size) {
        return Err(backend_error(format!(
            "{name} has {} values for batch size {batch_size}",
            values.len()
        )));
    }
    let width = values.len() / batch_size;
    let width = i32::try_from(width)
        .map_err(|_| backend_error(format!("{name} row is too wide for Arrow")))?;
    FixedSizeListArray::try_new(
        Arc::new(Field::new("item", data_type, false)),
        width,
        values,
        None,
    )
    .map(|array| Arc::new(array) as ArrayRef)
    .map_err(|error| backend_error(format!("building {name} Arrow tensor: {error}")))
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
) -> spiral_robotics_inference::Result<T> {
    require_inference_success(response, context)?
        .json()
        .map_err(|error| inference_error(format!("decoding {context} response: {error}")))
}

fn require_inference_success(
    response: reqwest::blocking::Response,
    context: &str,
) -> spiral_robotics_inference::Result<reqwest::blocking::Response> {
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

fn response_header(
    response: &reqwest::blocking::Response,
    name: &str,
) -> Result<String, TrainingBackendError> {
    response
        .headers()
        .get(name)
        .ok_or_else(|| backend_error(format!("checkpoint response is missing {name}")))?
        .to_str()
        .map(str::to_owned)
        .map_err(|error| backend_error(format!("checkpoint response has invalid {name}: {error}")))
}

fn backend_error(message: impl Into<String>) -> TrainingBackendError {
    TrainingBackendError::new(message)
}

fn inference_error(message: impl Into<String>) -> spiral_robotics_inference::Error {
    spiral_robotics_inference::Error::Inference(message.into())
}

fn duration_from_millis(milliseconds: f64) -> spiral_robotics_inference::Result<Duration> {
    Duration::try_from_secs_f64(milliseconds / 1_000.0)
        .map_err(|error| inference_error(format!("invalid inference duration: {error}")))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use arrow_array::{Array, FixedSizeListArray, Float32Array, UInt8Array};
    use arrow_ipc::reader::StreamReader;

    use super::*;

    #[test]
    fn training_batch_uses_fixed_size_list_tensor_columns() {
        let body = encode_training_batch(TrainingBatch::owned(
            2,
            vec![1.0, 2.0, 3.0, 4.0],
            vec![0.0; 12],
            vec![5.0; 8],
            vec![0, 1, 0, 0],
        ))
        .unwrap();
        let mut reader = StreamReader::try_new(Cursor::new(body), None).unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert!(reader.next().is_none());
        assert_eq!(batch.num_rows(), 2);

        let states = batch
            .column_by_name("states")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        assert_eq!(states.value_length(), 2);
        let values = states
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        assert_eq!(values.values().as_ref(), &[1.0, 2.0, 3.0, 4.0]);

        let padding = batch
            .column_by_name("action_is_pad")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        let values = padding
            .values()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        assert_eq!(values.values().as_ref(), &[0, 1, 0, 0]);
    }

    #[test]
    fn empty_runtime_endpoints_are_rejected_before_network_io() {
        assert!(HttpTrainingBackend::new(" ").health().is_err());
        assert!(HttpPolicyLoader::new("").url("/v1/policies").is_err());
    }
}
