//! The Modal training service as Studio reaches it, and the wire it speaks
//! (`docs/training.md#the-modal-service`): the deployment's address and
//! token, one request per call, and the fixtures in `fixtures/` fixing every
//! body both sides read.
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::training::{
    ModelPublication, PublishedModel, TrainingConfig, TrainingRunHistory, TrainingRunStatus,
};
use spiral_robotics_storage::ProviderKeys;
use url::Url;

/// The service API this Studio speaks.
const API_VERSION: u32 = 1;
/// A control call: the service answers from its Volume.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
/// One stream's upload, which may be a few hundred megabytes.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Publishing reads, hashes and copies a checkpoint's weights on the Volume.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// The service the provider keys name.
#[derive(Clone)]
pub(super) struct TrainingService {
    client: reqwest::Client,
    base: Url,
    token: String,
}

impl TrainingService {
    /// The service the provider keys name; a missing or malformed one is said
    /// plainly.
    pub(super) fn of(keys: &ProviderKeys) -> Result<Self, String> {
        let (Some(base), Some(token)) = (keys.training_service_url(), keys.modal_token()) else {
            return Err(
                "Training needs the Modal training service, which this Studio was started without: set TRAINING_SERVICE_URL and MODAL_SERVICE_TOKEN."
                    .into(),
            );
        };
        let base = Url::parse(base)
            .map_err(|error| format!("TRAINING_SERVICE_URL is invalid: {error}"))?;
        let local = matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base.scheme() != "https" && !(base.scheme() == "http" && local) {
            return Err(
                "TRAINING_SERVICE_URL must use HTTPS (HTTP is allowed only for localhost).".into(),
            );
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            base,
            token: token.to_owned(),
        })
    }

    /// The GPU the service trains on, once it is known to speak this API.
    pub(super) async fn health(&self) -> Result<String, String> {
        let health: ServiceHealth = self
            .send(
                self.client.get(self.url(&["v1", "health"])),
                "Asking the training service",
            )
            .await?
            .json()
            .await
            .map_err(|error| format!("Reading the training service's health: {error}"))?;
        if health.api_version != API_VERSION {
            return Err(format!(
                "The training service speaks API {}, this Studio {API_VERSION}; deploy matching versions.",
                health.api_version
            ));
        }
        Ok(health.gpu)
    }

    /// Which of `keys` the service does not hold yet.
    pub(super) async fn missing(&self, keys: &[StreamKey]) -> Result<Vec<StreamKey>, String> {
        let missing: MissingStreams = self
            .json(
                self.authorized(self.client.post(self.url(&["v1", "streams", "missing"])))
                    .json(&StreamKeys { keys }),
                "Listing the streams the training service lacks",
            )
            .await?;
        Ok(missing.missing)
    }

    /// Store `body`, one stream's Arrow IPC, under `key`.
    pub(super) async fn upload(&self, key: &StreamKey, body: reqwest::Body) -> Result<(), String> {
        let request = self
            .authorized(self.client.put(self.url(&["v1", "streams", key.as_str()])))
            .timeout(UPLOAD_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, ARROW_STREAM)
            .body(body);
        self.send(request, "Uploading a stream").await.map(drop)
    }

    /// Start the run `request` describes; the same request again answers its
    /// status.
    pub(super) async fn start(&self, request: &RunRequest) -> Result<RunStatus, String> {
        self.json(
            self.authorized(self.client.post(self.url(&["v1", "runs"])))
                .json(request),
            "Starting the run",
        )
        .await
    }

    /// Where each of `project`'s runs `ids` is; a run the service does not
    /// hold is left out.
    pub(super) async fn statuses(
        &self,
        project: &str,
        ids: &[String],
    ) -> Result<Vec<RunStatus>, String> {
        let mut url = self.url(&["v1", "runs", project]);
        url.query_pairs_mut().append_pair("ids", &ids.join(","));
        self.json(
            self.authorized(self.client.get(url)),
            "Asking where the runs are",
        )
        .await
    }

    /// Run `id` with its metrics and log tail, or `None` when the service
    /// does not hold it.
    pub(super) async fn history(
        &self,
        project: &str,
        id: &str,
    ) -> Result<Option<RunHistory>, String> {
        let response = self
            .authorized(self.client.get(self.url(&["v1", "runs", project, id])))
            .timeout(CONTROL_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("Reading the run: {error}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::checked(response, "Reading the run")
            .await?
            .json()
            .await
            .map(Some)
            .map_err(|error| format!("Reading the run: {error}"))
    }

    pub(super) async fn stop(&self, project: &str, id: &str) -> Result<RunStatus, String> {
        self.json(
            self.authorized(
                self.client
                    .post(self.url(&["v1", "runs", project, id, "stop"])),
            ),
            "Stopping the run",
        )
        .await
    }

    /// Delete run `id` and its checkpoints; a run the service never held
    /// has nothing to delete.
    pub(super) async fn delete(&self, project: &str, id: &str) -> Result<(), String> {
        let response = self
            .authorized(self.client.delete(self.url(&["v1", "runs", project, id])))
            .timeout(CONTROL_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("Deleting the run: {error}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::checked(response, "Deleting the run").await.map(drop)
    }

    /// Checkpoint `step` of run `id`: its policy config and its weights.
    pub(super) async fn checkpoint(
        &self,
        project: &str,
        id: &str,
        step: u64,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let step = step.to_string();
        let part = |name: &'static str| {
            let url = self.url(&["v1", "runs", project, id, "checkpoints", &step, name]);
            async move {
                self.send(
                    self.authorized(self.client.get(url))
                        .timeout(UPLOAD_TIMEOUT),
                    "Reading the checkpoint",
                )
                .await?
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| format!("Reading the checkpoint: {error}"))
            }
        };
        Ok((part("config").await?, part("model").await?))
    }

    /// Publish the checkpoint `request` names as a model the scorer reads
    /// by the selector it answers; the same weights again answer the same
    /// selector.
    pub(super) async fn publish(
        &self,
        request: &ModelPublishRequest,
    ) -> Result<PublishedWeights, String> {
        let context = "Publishing the model";
        let response = self
            .authorized(self.client.post(self.url(&["v1", "models"])))
            .json(request)
            .timeout(PUBLISH_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("{context}: {error}"))?;
        Self::checked(response, context)
            .await?
            .json()
            .await
            .map_err(|error| format!("{context}: {error}"))
    }

    /// The service's URL at `segments`, each one escaped.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("an HTTP URL has a path")
            .pop_if_empty()
            .extend(segments);
        url
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.bearer_auth(&self.token)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        context: &str,
    ) -> Result<T, String> {
        self.send(request.timeout(CONTROL_TIMEOUT), context)
            .await?
            .json()
            .await
            .map_err(|error| format!("{context}: {error}"))
    }

    /// Send `request` with the timeout its caller set: a control call's, or
    /// an upload's or a checkpoint read's much longer one.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        context: &str,
    ) -> Result<reqwest::Response, String> {
        let response = request
            .send()
            .await
            .map_err(|error| format!("{context}: {error}"))?;
        Self::checked(response, context).await
    }

    /// `response` when it succeeded, or what the service said went wrong.
    async fn checked(
        response: reqwest::Response,
        context: &str,
    ) -> Result<reqwest::Response, String> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err("The training service refused MODAL_SERVICE_TOKEN.".into());
        }
        let body = response.text().await.unwrap_or_default();
        let reason = serde_json::from_str::<ServiceError>(&body)
            .map(|error| error.error)
            .unwrap_or(body);
        Err(format!(
            "{context}: the training service answered {status}: {reason}"
        ))
    }
}

/// `GET /v1/health`.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct ServiceHealth {
    api_version: u32,
    gpu: String,
}

/// What the service says went wrong.
#[derive(Deserialize)]
struct ServiceError {
    error: String,
}

#[derive(Serialize)]
struct StreamKeys<'a> {
    keys: &'a [StreamKey],
}

#[derive(Deserialize)]
struct MissingStreams {
    missing: Vec<StreamKey>,
}

/// A stream's name on the service: the hash of what was read, so one read
/// of one dataset version is uploaded once, by any run.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct StreamKey(String);

impl StreamKey {
    /// The key of `statement` read for `episode` of the dataset version
    /// `(resource id, version id)`.
    pub(super) fn of(dataset: (&str, &str), episode: &str, statement: &str) -> Self {
        let read = serde_json::json!([dataset.0, dataset.1, episode, statement]);
        Self(format!("{:x}", Sha256::digest(read.to_string().as_bytes())))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// `POST /v1/runs`: everything the job needs, with Studio's config unchanged.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct RunRequest {
    project: String,
    run_id: String,
    config: TrainingConfig,
    #[serde(flatten)]
    data: RunData,
}

impl RunRequest {
    pub(super) fn new(
        (project, run_id): (&str, &str),
        config: TrainingConfig,
        data: RunData,
    ) -> Self {
        Self {
            project: project.to_owned(),
            run_id: run_id.to_owned(),
            config,
            data,
        }
    }
}

/// What a run trains on, in the shape its model family reads.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum RunData {
    Act(ActRunData),
    Progress(ProgressRunData),
}

/// What an ACT run trains on: the robot, what its joints are, and each
/// episode's intervals and streams.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ActRunData {
    robot_type: String,
    joints: JointNames,
    episodes: Vec<EpisodeStreams<ActStreams>>,
}

impl ActRunData {
    pub(super) fn new(
        robot_type: String,
        joints: JointNames,
        episodes: Vec<EpisodeStreams<ActStreams>>,
    ) -> Self {
        Self {
            robot_type,
            joints,
            episodes,
        }
    }
}

/// What a progress run trains on: each episode's intervals and its one
/// camera stream.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ProgressRunData {
    episodes: Vec<EpisodeStreams<ProgressStreams>>,
}

impl ProgressRunData {
    pub(super) fn new(episodes: Vec<EpisodeStreams<ProgressStreams>>) -> Self {
        Self { episodes }
    }
}

/// What each state and action value is, in order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct JointNames {
    state: Vec<String>,
    action: Vec<String>,
}

impl JointNames {
    pub(super) fn new(state: Vec<String>, action: Vec<String>) -> Self {
        Self { state, action }
    }
    pub(super) fn state(&self) -> &[String] {
        &self.state
    }
    pub(super) fn action(&self) -> &[String] {
        &self.action
    }
}

/// One episode of a run: the intervals its view keeps, and its streams.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct EpisodeStreams<S> {
    episode_id: String,
    intervals: Vec<[i64; 2]>,
    #[serde(flatten)]
    streams: S,
}

impl<S> EpisodeStreams<S> {
    pub(super) fn new(episode_id: String, intervals: Vec<[i64; 2]>, streams: S) -> Self {
        Self {
            episode_id,
            intervals,
            streams,
        }
    }
}

/// An ACT episode's streams: one per camera in the config's order, and the
/// value streams whose columns concatenate into the state and the action.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ActStreams {
    cameras: Vec<StreamKey>,
    state: Vec<StreamKey>,
    action: Vec<StreamKey>,
}

impl ActStreams {
    pub(super) fn new(
        cameras: Vec<StreamKey>,
        state: Vec<StreamKey>,
        action: Vec<StreamKey>,
    ) -> Self {
        Self {
            cameras,
            state,
            action,
        }
    }
}

/// A progress episode's one stream: the config's camera.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ProgressStreams {
    camera: StreamKey,
}

impl ProgressStreams {
    pub(super) fn new(camera: StreamKey) -> Self {
        Self { camera }
    }
}

/// Where a run is, as the service reports it.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct RunStatus {
    run_id: String,
    #[serde(flatten)]
    status: TrainingRunStatus,
    checkpoints: Vec<SavedCheckpoint>,
}

impl RunStatus {
    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }
    pub(super) fn status(&self) -> &TrainingRunStatus {
        &self.status
    }
    pub(super) fn checkpoints(&self) -> &[SavedCheckpoint] {
        &self.checkpoints
    }
}

/// A checkpoint a run saved.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct SavedCheckpoint {
    step: u64,
    validation_loss: Option<f64>,
    created_ms: i64,
}

impl SavedCheckpoint {
    pub(super) const fn step(&self) -> u64 {
        self.step
    }
    pub(super) const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }
}

/// `POST /v1/models`: which checkpoint to publish.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ModelPublishRequest {
    project: String,
    run_id: String,
    step: u64,
}

impl ModelPublishRequest {
    pub(super) fn new(project: &str, run_id: &str, step: u64) -> Self {
        Self {
            project: project.to_owned(),
            run_id: run_id.to_owned(),
            step,
        }
    }
}

/// What the service published: the selector the scorer reads the weights by,
/// and the checkpoint's manifest.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct PublishedWeights {
    model: String,
    manifest: ProgressManifest,
}

impl PublishedWeights {
    /// The model `publication` names, as the service published it.
    pub(super) fn named(self, publication: ModelPublication) -> PublishedModel {
        PublishedModel::new(publication, self.model, self.manifest.camera)
    }
}

/// A progress checkpoint's manifest. Studio reads the camera it was trained
/// on; the rest belongs to the job and the scorer, kept as written.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct ProgressManifest {
    camera: String,
    #[serde(flatten)]
    recipe: serde_json::Map<String, serde_json::Value>,
}

/// A run's status with the metrics and log tail its job keeps.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct RunHistory {
    #[serde(flatten)]
    status: RunStatus,
    #[serde(flatten)]
    history: TrainingRunHistory,
}

impl RunHistory {
    pub(super) fn into_parts(self) -> (RunStatus, TrainingRunHistory) {
        (self.status, self.history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use spiral_robotics_common::training::TrainingModelKind;

    /// A fixture both sides read, unchanged by a round trip through its type.
    fn round_trips<T: DeserializeOwned + Serialize>(fixture: &str) -> T {
        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let parsed: T = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
        parsed
    }

    #[test]
    fn the_wire_fixtures_are_what_studio_writes_and_reads() {
        let health: ServiceHealth = round_trips(include_str!("fixtures/health.json"));
        assert_eq!(health.api_version, API_VERSION);
        let request: RunRequest = round_trips(include_str!("fixtures/run-request.json"));
        assert_eq!(request.config.model().cameras().len(), 2);
        let RunData::Act(data) = &request.data else {
            panic!("an ACT request carries ACT data");
        };
        assert!(
            data.episodes
                .iter()
                .all(|episode| episode.streams.cameras.len() == 2)
        );
        let request: RunRequest = round_trips(include_str!("fixtures/progress-run-request.json"));
        assert_eq!(request.config.model().kind(), TrainingModelKind::Progress);
        assert_eq!(request.config.model().cameras(), ["/top-camera"]);
        let RunData::Progress(data) = &request.data else {
            panic!("a progress request carries one camera per episode");
        };
        assert_eq!(data.episodes.len(), 2);
        let status: RunStatus = round_trips(include_str!("fixtures/run-status.json"));
        assert_eq!(status.checkpoints()[0].step(), 3);
        let history: RunHistory = round_trips(include_str!("fixtures/run-history.json"));
        let (status, history) = history.into_parts();
        assert_eq!(status.status().phase().label(), "Running");
        assert_eq!(
            serde_json::to_value(history).unwrap()["metrics"][1]["event"],
            "validation"
        );
        let request: ModelPublishRequest =
            round_trips(include_str!("fixtures/model-publish-request.json"));
        assert_eq!(
            request,
            ModelPublishRequest::new("fixture-project", "train-1791300000000-1", 3)
        );
        let published: PublishedWeights = round_trips(include_str!("fixtures/model-publish.json"));
        let model = published.named(ModelPublication::new(
            "train-1791300000000-1",
            3,
            "Bottle experts",
        ));
        assert!(model.model().starts_with("spiral-robotics-models/"));
        assert_eq!(model.camera(), "/top-camera");
    }

    #[test]
    fn a_key_names_exactly_one_read_of_one_dataset_version() {
        let key = StreamKey::of(("resource", "v1"), "episode", "SELECT 1");
        assert_eq!(key.as_str().len(), 64);
        assert_eq!(
            key,
            StreamKey::of(("resource", "v1"), "episode", "SELECT 1")
        );
        assert_ne!(
            key,
            StreamKey::of(("resource", "v2"), "episode", "SELECT 1")
        );
        assert_ne!(
            key,
            StreamKey::of(("resource", "v1"), "episode", "SELECT 2")
        );
    }

    #[test]
    fn paths_escape_every_segment() {
        let keys = ProviderKeys::from_variables(|name| match name {
            "TRAINING_SERVICE_URL" => Some("https://train.example/base/".into()),
            "MODAL_SERVICE_TOKEN" => Some("token".into()),
            _ => None,
        });
        let service = TrainingService::of(&keys).unwrap();
        assert_eq!(
            service.url(&["v1", "runs", "a/b", "c"]).as_str(),
            "https://train.example/base/v1/runs/a%2Fb/c"
        );
        let insecure = ProviderKeys::from_variables(|name| match name {
            "TRAINING_SERVICE_URL" => Some("http://train.example".into()),
            "MODAL_SERVICE_TOKEN" => Some("token".into()),
            _ => None,
        });
        assert!(TrainingService::of(&insecure).is_err());
        assert!(TrainingService::of(&ProviderKeys::default()).is_err());
    }
}
