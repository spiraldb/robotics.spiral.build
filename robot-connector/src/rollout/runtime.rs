//! Client for the policy runtime that computes rollout actions: an HTTP
//! control plane and an Arrow IPC data plane.
//!
//! The runtime is a separate Python/PyTorch service. This adapter owns only
//! request encoding and response decoding, keeping that tensor framework
//! outside the connector process.

use std::sync::Arc;
use std::time::Duration;

use arrow_array::{ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, UInt8Array};
use arrow_ipc::CompressionType;
use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow_schema::{DataType, Field};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, de::DeserializeOwned};
use spiral_robotics_common::rollout::POLICY_ENVELOPE_MEDIA_TYPE;
use spiral_robotics_inference::{
    ActionChunk, ActionChunkShape, InferenceInput, InferenceStats, ModelArtifact, Policy,
    PolicyLoader, PolicySpecification,
};

const ARROW_STREAM_CONTENT_TYPE: &str = "application/vnd.apache.arrow.stream";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(7);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
        let body = artifact.envelope();
        let response = self
            .client
            .post(self.url("/v1/policies")?)
            .header(CONTENT_TYPE, POLICY_ENVELOPE_MEDIA_TYPE)
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
    model_ms: f64,
    total_ms: f64,
}

impl InferenceStatsResponse {
    fn into_stats(self) -> spiral_robotics_inference::InferenceResult<InferenceStats> {
        Ok(InferenceStats::default()
            .with_model(self.model_ms.inference_duration()?)
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
            FixedListLayout::new(DataType::Float32, "state")
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
                || expected.channels() != 3
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
                FixedListLayout::new(DataType::UInt8, &format!("image_{index}"))
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

/// One tensor sent as a single fixed-size list row.
struct FixedListLayout<'a> {
    data_type: DataType,
    name: &'a str,
}

impl<'a> FixedListLayout<'a> {
    fn new(data_type: DataType, name: &'a str) -> Self {
        Self { data_type, name }
    }

    fn build(self, values: ArrayRef) -> Result<ArrayRef, String> {
        let width = i32::try_from(values.len())
            .map_err(|_| format!("{} row is too wide for Arrow", self.name))?;
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
        assert!(HttpPolicyLoader::new("").url("/v1/policies").is_err());
    }
}
