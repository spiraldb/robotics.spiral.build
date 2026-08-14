//! Lightweight HTTP control plane and Arrow IPC data plane for training.
//!
//! JSON carries session lifecycle, telemetry, losses, and checkpoint requests.
//! Optimizer inputs are one Arrow IPC stream whose fixed-size-list rows are
//! borrowed directly by the backend for the duration of a synchronous step.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use arrow_array::{Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, UInt8Array};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingBatch,
    TrainingCheckpointArtifact, TrainingCheckpointRequest, TrainingLoss, TrainingModelConfig,
    TrainingSession, TrainingSessionInfo, TrainingSessionRequest, TrainingTelemetry,
};

const ARROW_STREAM_CONTENT_TYPE: &str = "application/vnd.apache.arrow.stream";
const CHECKPOINT_CONTENT_TYPE: &str = "application/octet-stream";
const CHECKPOINT_FORMAT_HEADER: &str = "x-spiral-checkpoint-format";
const CHECKPOINT_EXTENSION_HEADER: &str = "x-spiral-checkpoint-extension";
const MAX_ARROW_REQUEST_BYTES: usize = 320 * 1024 * 1024;
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

#[derive(Clone)]
struct AppState {
    backend: Arc<dyn TrainingBackend>,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<SessionState>>>>>,
    session_ids: Arc<AtomicU64>,
}

struct SessionState {
    session: Box<dyn TrainingSession>,
    shape: BatchShape,
    last_train: Option<(u64, StepResponse)>,
}

#[derive(Clone)]
struct TrainingHttpService {
    state: AppState,
}

impl TrainingHttpService {
    fn new(backend: Arc<dyn TrainingBackend>) -> Self {
        Self {
            state: AppState {
                backend,
                sessions: Arc::new(Mutex::new(HashMap::new())),
                session_ids: Arc::new(AtomicU64::new(0)),
            },
        }
    }

    fn router(self) -> Router {
        Router::new()
            .route("/v1/health", get(health))
            .route("/v1/sessions", post(create_session))
            .route("/v1/sessions/{id}", delete(delete_session))
            .route("/v1/sessions/{id}/telemetry", get(telemetry))
            .route("/v1/sessions/{id}/train", post(train))
            .route("/v1/sessions/{id}/validate", post(validate))
            .route("/v1/sessions/{id}/checkpoint", post(checkpoint))
            .layer(DefaultBodyLimit::max(MAX_ARROW_REQUEST_BYTES))
            .with_state(self.state)
    }
}

pub async fn serve(
    address: std::net::SocketAddr,
    backend: Arc<dyn TrainingBackend>,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, TrainingHttpService::new(backend).router()).await
}

async fn health(State(state): State<AppState>) -> Result<Json<TrainingBackendInfo>, ApiError> {
    let info = tokio::task::spawn_blocking(move || state.backend.health())
        .await
        .map_err(task_error)?
        .map_err(ApiError::unavailable)?;
    Ok(Json(info))
}

async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<TrainingSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let shape = BatchShape::from_model(&request.model).map_err(ApiError::bad_request)?;
    let backend = state.backend.clone();
    let sessions = state.sessions.clone();
    let session_id = format!(
        "session-{}",
        state.session_ids.fetch_add(1, Ordering::Relaxed) + 1
    );
    let response_id = session_id.clone();
    let response = tokio::task::spawn_blocking(move || {
        let session = backend
            .create_session(request)
            .map_err(ApiError::unprocessable)?;
        let info = session.info().clone();
        lock(&sessions, "training session registry")?.insert(
            session_id,
            Arc::new(Mutex::new(SessionState {
                session,
                shape,
                last_train: None,
            })),
        );
        Ok::<_, ApiError>(CreateSessionResponse {
            session_id: response_id,
            info,
        })
    })
    .await
    .map_err(task_error)??;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    lock(&state.sessions, "training session registry")?.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn telemetry(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TrainingTelemetry>, ApiError> {
    let session = find_session(&state, &id)?;
    let telemetry = tokio::task::spawn_blocking(move || {
        lock(&session, "training session")?
            .session
            .telemetry()
            .map_err(ApiError::unprocessable)
    })
    .await
    .map_err(task_error)??;
    Ok(Json(telemetry))
}

#[derive(Deserialize)]
struct TrainQuery {
    step: u64,
}

async fn train(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TrainQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<StepResponse>, ApiError> {
    require_arrow_content_type(&headers)?;
    let session = find_session(&state, &id)?;
    let response = tokio::task::spawn_blocking(move || {
        let record = decode_training_batch(body).map_err(ApiError::bad_request)?;
        let mut state = lock(&session, "training session")?;
        if let Some((last_step, response)) = &state.last_train {
            if query.step == *last_step {
                return Ok(response.clone());
            }
            if query.step != last_step.saturating_add(1) {
                return Err(ApiError::conflict(format!(
                    "optimizer step {} cannot follow completed step {last_step}",
                    query.step
                )));
            }
        }
        let batch = borrow_training_batch(&record, &state.shape).map_err(ApiError::bad_request)?;
        let loss = state
            .session
            .train_step(batch, query.step)
            .map_err(ApiError::unprocessable)?;
        let response = StepResponse {
            loss,
            learning_rate: state.session.learning_rate(query.step),
            telemetry: state.session.telemetry().map_err(ApiError::unprocessable)?,
        };
        state.last_train = Some((query.step, response.clone()));
        Ok::<_, ApiError>(response)
    })
    .await
    .map_err(task_error)??;
    Ok(Json(response))
}

async fn validate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<StepResponse>, ApiError> {
    require_arrow_content_type(&headers)?;
    let session = find_session(&state, &id)?;
    let response = tokio::task::spawn_blocking(move || {
        let record = decode_training_batch(body).map_err(ApiError::bad_request)?;
        let mut state = lock(&session, "training session")?;
        let batch = borrow_training_batch(&record, &state.shape).map_err(ApiError::bad_request)?;
        let loss = state
            .session
            .validation_step(batch)
            .map_err(ApiError::unprocessable)?;
        Ok::<_, ApiError>(StepResponse {
            loss,
            learning_rate: state.session.learning_rate(0),
            telemetry: state.session.telemetry().map_err(ApiError::unprocessable)?,
        })
    })
    .await
    .map_err(task_error)??;
    Ok(Json(response))
}

async fn checkpoint(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TrainingCheckpointRequest>,
) -> Result<Response, ApiError> {
    let session = find_session(&state, &id)?;
    let artifact = tokio::task::spawn_blocking(move || {
        lock(&session, "training session")?
            .session
            .checkpoint(request)
            .map_err(ApiError::unprocessable)
    })
    .await
    .map_err(task_error)??;
    checkpoint_response(artifact)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CreateSessionResponse {
    session_id: String,
    info: TrainingSessionInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StepResponse {
    loss: TrainingLoss,
    learning_rate: f64,
    telemetry: TrainingTelemetry,
}

#[derive(Clone, Copy)]
struct BatchShape {
    states: usize,
    images: usize,
    actions: usize,
    action_is_pad: usize,
}

impl BatchShape {
    fn from_model(model: &TrainingModelConfig) -> Result<Self, String> {
        let TrainingModelConfig::Act(config) = model;
        let images = config
            .camera_streams
            .len()
            .checked_mul(3)
            .and_then(|value| value.checked_mul(config.image_height))
            .and_then(|value| value.checked_mul(config.image_width))
            .ok_or_else(|| "ACT image shape overflowed".to_owned())?;
        let actions = config
            .chunk_size
            .checked_mul(config.action_dimensions)
            .ok_or_else(|| "ACT action shape overflowed".to_owned())?;
        let shape = Self {
            states: config.state_dimensions,
            images,
            actions,
            action_is_pad: config.chunk_size,
        };
        if [
            shape.states,
            shape.images,
            shape.actions,
            shape.action_is_pad,
        ]
        .contains(&0)
        {
            return Err("ACT tensor dimensions must be non-zero".to_owned());
        }
        Ok(shape)
    }
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
    let mut body = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut body, &record.schema())
            .map_err(|error| backend_error(format!("starting Arrow IPC stream: {error}")))?;
        writer
            .write(&record)
            .map_err(|error| backend_error(format!("writing Arrow IPC stream: {error}")))?;
        writer
            .finish()
            .map_err(|error| backend_error(format!("finishing Arrow IPC stream: {error}")))?;
    }
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

fn decode_training_batch(body: Bytes) -> Result<RecordBatch, String> {
    let mut reader = StreamReader::try_new(Cursor::new(body), None)
        .map_err(|error| format!("invalid Arrow IPC stream: {error}"))?;
    let batch = reader
        .next()
        .transpose()
        .map_err(|error| format!("invalid Arrow record batch: {error}"))?
        .ok_or_else(|| "Arrow IPC stream contains no record batch".to_owned())?;
    if reader.next().is_some() {
        return Err("Arrow IPC stream must contain exactly one record batch".to_owned());
    }
    Ok(batch)
}

fn borrow_training_batch<'a>(
    batch: &'a RecordBatch,
    shape: &BatchShape,
) -> Result<TrainingBatch<'a>, String> {
    let batch_size = batch.num_rows();
    if batch_size == 0 {
        return Err("Arrow training batch cannot be empty".to_owned());
    }
    let states = f32_values(batch, "states", shape.states)?;
    let images = f32_values(batch, "images", shape.images)?;
    let actions = f32_values(batch, "actions", shape.actions)?;
    let action_is_pad = u8_values(batch, "action_is_pad", shape.action_is_pad)?;
    if action_is_pad.iter().any(|value| *value > 1) {
        return Err("action_is_pad values must be 0 or 1".to_owned());
    }
    Ok(TrainingBatch::borrowed(
        batch_size,
        states,
        images,
        actions,
        action_is_pad,
    ))
}

fn f32_values<'a>(batch: &'a RecordBatch, name: &str, width: usize) -> Result<&'a [f32], String> {
    let list = fixed_column(batch, name, width)?;
    let values = list
        .values()
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| format!("Arrow tensor {name:?} must contain Float32 values"))?;
    if values.null_count() != 0 {
        return Err(format!("Arrow tensor {name:?} cannot contain null values"));
    }
    Ok(values.values().as_ref())
}

fn u8_values<'a>(batch: &'a RecordBatch, name: &str, width: usize) -> Result<&'a [u8], String> {
    let list = fixed_column(batch, name, width)?;
    let values = list
        .values()
        .as_any()
        .downcast_ref::<UInt8Array>()
        .ok_or_else(|| format!("Arrow tensor {name:?} must contain UInt8 values"))?;
    if values.null_count() != 0 {
        return Err(format!("Arrow tensor {name:?} cannot contain null values"));
    }
    Ok(values.values().as_ref())
}

fn fixed_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
    width: usize,
) -> Result<&'a FixedSizeListArray, String> {
    let list = batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<FixedSizeListArray>())
        .ok_or_else(|| format!("missing FixedSizeList Arrow tensor {name:?}"))?;
    let width = i32::try_from(width).map_err(|_| format!("{name} tensor width is too large"))?;
    if list.len() != batch.num_rows() || list.value_length() != width {
        return Err(format!(
            "Arrow tensor {name:?} has shape [{}, {}], expected [{}, {width}]",
            list.len(),
            list.value_length(),
            batch.num_rows()
        ));
    }
    if list.null_count() != 0 {
        return Err(format!("Arrow tensor {name:?} cannot contain null rows"));
    }
    Ok(list)
}

fn require_arrow_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type.starts_with(ARROW_STREAM_CONTENT_TYPE) {
        return Err(ApiError::unsupported_media(format!(
            "optimizer batches require Content-Type {ARROW_STREAM_CONTENT_TYPE}"
        )));
    }
    Ok(())
}

fn checkpoint_response(artifact: TrainingCheckpointArtifact) -> Result<Response, ApiError> {
    let mut response = (StatusCode::OK, artifact.payload).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(CHECKPOINT_CONTENT_TYPE),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=checkpoint.safetensors"),
    );
    headers.insert(
        CHECKPOINT_FORMAT_HEADER,
        header_value(&artifact.format, "checkpoint format")?,
    );
    headers.insert(
        CHECKPOINT_EXTENSION_HEADER,
        header_value(&artifact.extension, "checkpoint extension")?,
    );
    Ok(response)
}

fn find_session(state: &AppState, id: &str) -> Result<Arc<Mutex<SessionState>>, ApiError> {
    lock(&state.sessions, "training session registry")?
        .get(id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("training session {id:?} does not exist")))
}

fn lock<'a, T>(mutex: &'a Mutex<T>, name: &str) -> Result<MutexGuard<'a, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::internal(format!("{name} is unavailable")))
}

fn task_error(error: tokio::task::JoinError) -> ApiError {
    ApiError::internal(format!("training runtime task failed: {error}"))
}

fn header_value(value: &str, name: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value)
        .map_err(|error| ApiError::internal(format!("invalid {name} metadata: {error}")))
}

fn decode_json_response<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
    context: &str,
) -> Result<T, TrainingBackendError> {
    require_success(response, context)?
        .json()
        .map_err(|error| backend_error(format!("decoding {context} response: {error}")))
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

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl ToString) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn unsupported_media(message: impl ToString) -> Self {
        Self::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, message)
    }

    fn not_found(message: impl ToString) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: impl ToString) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn unprocessable(message: impl ToString) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    fn unavailable(message: impl ToString) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    fn internal(message: impl ToString) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    fn new(status: StatusCode, message: impl ToString) -> Self {
        Self {
            status,
            message: message.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: &self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    use super::*;
    use spiral_robotics_training::{
        ActTrainingConfig, TRAINING_BACKEND_API_VERSION, TrainingInitialization,
    };

    struct FakeBackend {
        borrowed: Arc<AtomicBool>,
        train_count: Arc<AtomicUsize>,
        telemetry_count: Arc<AtomicUsize>,
    }

    impl TrainingBackend for FakeBackend {
        fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
            Ok(TrainingBackendInfo {
                api_version: TRAINING_BACKEND_API_VERSION,
                id: "fake.http".to_owned(),
                display_name: "Fake HTTP".to_owned(),
                runtime: "test".to_owned(),
                devices: vec!["CPU".to_owned()],
            })
        }

        fn create_session(
            &self,
            _request: TrainingSessionRequest,
        ) -> Result<Box<dyn TrainingSession>, TrainingBackendError> {
            Ok(Box::new(FakeSession {
                info: TrainingSessionInfo {
                    backend: self.health()?,
                    device: "CPU".to_owned(),
                    parameter_count: 42,
                },
                borrowed: self.borrowed.clone(),
                train_count: self.train_count.clone(),
                telemetry_count: self.telemetry_count.clone(),
            }))
        }
    }

    struct FakeSession {
        info: TrainingSessionInfo,
        borrowed: Arc<AtomicBool>,
        train_count: Arc<AtomicUsize>,
        telemetry_count: Arc<AtomicUsize>,
    }

    impl TrainingSession for FakeSession {
        fn info(&self) -> &TrainingSessionInfo {
            &self.info
        }

        fn learning_rate(&self, step: u64) -> f64 {
            step as f64 * 0.001
        }

        fn telemetry(&self) -> Result<TrainingTelemetry, TrainingBackendError> {
            self.telemetry_count.fetch_add(1, Ordering::Relaxed);
            Ok(TrainingTelemetry {
                cpu_percent: Some(12.0),
                gpu_percent: Some(34.0),
                memory_bytes: Some(56),
            })
        }

        fn train_step(
            &mut self,
            batch: TrainingBatch<'_>,
            _step: u64,
        ) -> Result<TrainingLoss, TrainingBackendError> {
            self.borrowed.store(
                matches!(batch.states, Cow::Borrowed(_))
                    && matches!(batch.images, Cow::Borrowed(_))
                    && matches!(batch.actions, Cow::Borrowed(_))
                    && matches!(batch.action_is_pad, Cow::Borrowed(_)),
                Ordering::Relaxed,
            );
            self.train_count.fetch_add(1, Ordering::Relaxed);
            Ok(TrainingLoss {
                total: 1.0,
                l1: 2.0,
                kl: 3.0,
                pad: 4.0,
            })
        }

        fn validation_step(
            &mut self,
            batch: TrainingBatch<'_>,
        ) -> Result<TrainingLoss, TrainingBackendError> {
            self.train_step(batch, 0)
        }

        fn checkpoint(
            &self,
            _request: TrainingCheckpointRequest,
        ) -> Result<TrainingCheckpointArtifact, TrainingBackendError> {
            Ok(TrainingCheckpointArtifact {
                format: "safetensors".to_owned(),
                extension: "safetensors".to_owned(),
                payload: vec![1, 2, 3, 4],
            })
        }
    }

    fn batch() -> TrainingBatch<'static> {
        TrainingBatch::owned(1, vec![0.0], vec![0.0; 3], vec![0.0], vec![0])
    }

    fn session_request() -> TrainingSessionRequest {
        TrainingSessionRequest {
            model: TrainingModelConfig::Act(ActTrainingConfig {
                state_dimensions: 1,
                action_dimensions: 1,
                camera_streams: vec!["camera".to_owned()],
                image_width: 1,
                image_height: 1,
                chunk_size: 1,
                hidden_dimensions: 8,
                latent_dimensions: 2,
                attention_heads: 1,
                feedforward_dimensions: 8,
                encoder_layers: 1,
                vae_encoder_layers: 1,
                decoder_layers: 1,
                kl_weight: 1.0,
                learning_rate: 0.001,
                total_steps: 2,
                seed: 1,
            }),
            device: "cpu".to_owned(),
            initialization: TrainingInitialization::Random,
        }
    }

    #[test]
    fn arrow_tensor_rows_round_trip_without_a_decode_copy() {
        let encoded = encode_training_batch(batch()).unwrap();
        let record = decode_training_batch(Bytes::from(encoded)).unwrap();
        let view = borrow_training_batch(
            &record,
            &BatchShape {
                states: 1,
                images: 3,
                actions: 1,
                action_is_pad: 1,
            },
        )
        .unwrap();
        assert!(matches!(view.states, Cow::Borrowed(_)));
        assert_eq!(view.images.as_ref(), &[0.0; 3]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_service_executes_a_remote_session() {
        let borrowed = Arc::new(AtomicBool::new(false));
        let train_count = Arc::new(AtomicUsize::new(0));
        let telemetry_count = Arc::new(AtomicUsize::new(0));
        let service = TrainingHttpService::new(Arc::new(FakeBackend {
            borrowed: borrowed.clone(),
            train_count: train_count.clone(),
            telemetry_count: telemetry_count.clone(),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, service.router()).await.unwrap();
        });
        tokio::task::spawn_blocking(move || {
            let backend = HttpTrainingBackend::new(format!("http://{address}"));
            assert_eq!(backend.health().unwrap().id, "fake.http");
            let mut session = backend.create_session(session_request()).unwrap();
            assert_eq!(session.info().device, "CPU");
            assert_eq!(session.telemetry().unwrap().gpu_percent, Some(34.0));
            let loss = session.train_step(batch(), 2).unwrap();
            assert_eq!(loss.total, 1.0);
            let retry = session.train_step(batch(), 2).unwrap();
            assert_eq!(retry, loss);
            assert_eq!(session.learning_rate(2), 0.002);
            assert_eq!(session.telemetry().unwrap().gpu_percent, Some(34.0));
            let artifact = session
                .checkpoint(TrainingCheckpointRequest {
                    manifest_json: "{}".to_owned(),
                    state_mean: vec![0.0],
                    state_std: vec![1.0],
                    action_mean: vec![0.0],
                    action_std: vec![1.0],
                })
                .unwrap();
            assert_eq!(artifact.format, "safetensors");
            assert_eq!(artifact.payload, vec![1, 2, 3, 4]);
        })
        .await
        .unwrap();
        assert!(borrowed.load(Ordering::Relaxed));
        assert_eq!(train_count.load(Ordering::Relaxed), 1);
        assert_eq!(telemetry_count.load(Ordering::Relaxed), 2);
        server.abort();
    }
}
