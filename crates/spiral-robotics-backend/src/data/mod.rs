//! Framework-neutral data settings, query, and remote replay actions.

pub(crate) mod catalog;
pub(crate) mod catalog_lease;
pub(crate) mod episode_arms;
pub(crate) mod queries;
pub(crate) mod seek;

use std::collections::BTreeMap;

use spiral_robotics_core::query::{ReadOnlyQuery, ReadOnlyQueryMode};
use url::Url;

use spiral_robotics_core::OperationError;

const MAX_QUERY_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SpiralDestinationSettings {
    endpoint: String,
    spfs_endpoint: String,
    project_id: String,
    client_id: String,
    client_secret_configured: bool,
    namespace: String,
    endpoint_restart_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    spiral_endpoint: String,
    spiral_spfs_endpoint: String,
    project_id: String,
    client_id: String,
    /// `None` preserves the saved secret; `Some("")` clears it.
    client_secret: Option<String>,
    training_endpoint: String,
    video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsSnapshot {
    spiral: SpiralDestinationSettings,
    training_endpoint: String,
    video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionCheck {
    ok: bool,
    message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryValue {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Array(Vec<QueryValue>),
    Object(BTreeMap<String, QueryValue>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryColumn {
    name: String,
    data_type: String,
    nullable: bool,
    semantic_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryResult {
    columns: Vec<QueryColumn>,
    rows: Vec<BTreeMap<String, QueryValue>>,
    truncated: bool,
    elapsed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryCommand {
    statement: String,
    max_rows: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartSpiralReplayCommand {
    episode: String,
    robot: String,
    stream: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayTrajectory {
    sample_hz: f64,
    frames: Vec<BTreeMap<String, f64>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VideoAssetQuery {
    episode: String,
    stream: String,
    segment: Option<u32>,
}

impl SpiralDestinationSettings {
    pub(crate) fn new(
        endpoint: impl Into<String>,
        spfs_endpoint: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            spfs_endpoint: spfs_endpoint.into(),
            project_id: project_id.into(),
            client_id: String::new(),
            client_secret_configured: false,
            namespace: String::new(),
            endpoint_restart_required: false,
        }
    }

    pub(crate) fn with_client(
        mut self,
        client_id: impl Into<String>,
        secret_configured: bool,
    ) -> Self {
        self.client_id = client_id.into();
        self.client_secret_configured = secret_configured;
        self
    }

    pub(crate) fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub(crate) fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }
    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    pub(crate) fn client_secret_configured(&self) -> bool {
        self.client_secret_configured
    }
    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }
    pub(crate) fn endpoint_restart_required(&self) -> bool {
        self.endpoint_restart_required
    }
}

impl UpdateDataSettingsCommand {
    pub(crate) fn new(
        spiral_endpoint: impl Into<String>,
        project_id: impl Into<String>,
        training_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            spiral_endpoint: spiral_endpoint.into(),
            spiral_spfs_endpoint: String::new(),
            project_id: project_id.into(),
            client_id: String::new(),
            client_secret: None,
            training_endpoint: training_endpoint.into(),
            video_enabled: false,
        }
    }

    pub(crate) fn with_spfs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.spiral_spfs_endpoint = endpoint.into();
        self
    }

    pub(crate) fn with_client(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = client_id.into();
        self
    }

    pub(crate) fn with_client_secret(mut self, secret: Option<String>) -> Self {
        self.client_secret = secret;
        self
    }

    pub(crate) fn with_video_enabled(mut self, enabled: bool) -> Self {
        self.video_enabled = enabled;
        self
    }

    pub(crate) fn spiral_endpoint(&self) -> &str {
        &self.spiral_endpoint
    }
    pub(crate) fn spiral_spfs_endpoint(&self) -> &str {
        &self.spiral_spfs_endpoint
    }
    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    pub(crate) fn client_secret(&self) -> Option<&str> {
        self.client_secret.as_deref()
    }
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl DataSettingsSnapshot {
    pub(crate) fn new(
        spiral: SpiralDestinationSettings,
        training_endpoint: impl Into<String>,
        video_enabled: bool,
    ) -> Self {
        Self {
            spiral,
            training_endpoint: training_endpoint.into(),
            video_enabled,
        }
    }

    pub(crate) fn spiral(&self) -> &SpiralDestinationSettings {
        &self.spiral
    }
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl ConnectionCheck {
    pub(crate) fn new(ok: bool, message: impl Into<String>) -> Self {
        Self {
            ok,
            message: message.into(),
        }
    }

    pub(crate) fn into_parts(self) -> (bool, String) {
        (self.ok, self.message)
    }
}

impl QueryColumn {
    pub(crate) fn new(
        name: impl Into<String>,
        data_type: impl Into<String>,
        nullable: bool,
    ) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            semantic_type: None,
        }
    }

    pub(crate) fn with_semantic_type(mut self, semantic_type: Option<String>) -> Self {
        self.semantic_type = semantic_type;
        self
    }

    pub(crate) fn into_parts(self) -> (String, String, bool, Option<String>) {
        (self.name, self.data_type, self.nullable, self.semantic_type)
    }
}

impl QueryResult {
    pub(crate) fn new(columns: Vec<QueryColumn>, rows: Vec<BTreeMap<String, QueryValue>>) -> Self {
        Self {
            columns,
            rows,
            truncated: false,
            elapsed_ms: 0,
        }
    }

    pub(crate) fn with_status(mut self, truncated: bool, elapsed_ms: u64) -> Self {
        self.truncated = truncated;
        self.elapsed_ms = elapsed_ms;
        self
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Vec<QueryColumn>,
        Vec<BTreeMap<String, QueryValue>>,
        bool,
        u64,
    ) {
        (self.columns, self.rows, self.truncated, self.elapsed_ms)
    }
}

impl QueryCommand {
    pub(crate) fn new(statement: impl Into<String>, max_rows: usize) -> Self {
        Self {
            statement: statement.into(),
            max_rows,
        }
    }

    pub(crate) fn statement(&self) -> &str {
        &self.statement
    }
    pub(crate) fn max_rows(&self) -> usize {
        self.max_rows
    }
}

impl StartSpiralReplayCommand {
    pub(crate) fn new(
        episode: impl Into<String>,
        robot: impl Into<String>,
        stream: impl Into<String>,
    ) -> Self {
        Self {
            episode: episode.into(),
            robot: robot.into(),
            stream: stream.into(),
        }
    }

    pub(crate) fn into_parts(self) -> (String, String, String) {
        (self.episode, self.robot, self.stream)
    }
}

impl ReplayTrajectory {
    pub(crate) fn new(sample_hz: f64, frames: Vec<BTreeMap<String, f64>>) -> Self {
        Self { sample_hz, frames }
    }

    pub(crate) fn into_parts(self) -> (f64, Vec<BTreeMap<String, f64>>) {
        (self.sample_hz, self.frames)
    }
}

impl VideoAssetQuery {
    pub(crate) fn new(
        episode: impl Into<String>,
        stream: impl Into<String>,
        segment: Option<u32>,
    ) -> Self {
        Self {
            episode: episode.into(),
            stream: stream.into(),
            segment,
        }
    }

    pub(crate) fn into_parts(self) -> (String, String, Option<u32>) {
        (self.episode, self.stream, self.segment)
    }
}

/// Data-catalog consequences expressed only in capability-owned models.
pub(crate) trait DataCatalogPort: Send + Sync {
    fn settings(&self) -> DataSettingsSnapshot;
    fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsSnapshot, OperationError>;
    async fn refresh_training_readiness(&self);

    async fn test_connection(&self) -> ConnectionCheck;
    async fn query(&self, command: QueryCommand) -> Result<QueryResult, OperationError>;
    async fn load_replay_trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<ReplayTrajectory, OperationError>;
    async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, OperationError>;
}

impl VideoAssetQuery {
    pub(crate) async fn load<P: DataCatalogPort + ?Sized>(
        self,
        port: &P,
    ) -> Result<Vec<u8>, OperationError> {
        validate_identifier("episode", &self.episode)?;
        validate_identifier("stream", &self.stream)?;
        port.video_asset(self).await
    }
}

/// Control consequence for a trajectory already admitted and loaded by the
/// data-catalog action.
pub(crate) trait SpiralReplayPort: Send + Sync {
    async fn start_spiral_replay(
        &self,
        command: StartSpiralReplayCommand,
        trajectory: ReplayTrajectory,
    ) -> Result<(), OperationError>;
}

impl UpdateDataSettingsCommand {
    pub(crate) async fn apply<P: DataCatalogPort + ?Sized>(
        self,
        port: &P,
    ) -> Result<DataSettingsSnapshot, OperationError> {
        let command = self.validated()?;
        let settings = port.update_settings(command)?;
        // Training readiness depends on the selected runtime endpoint, so publish
        // preferences before rebuilding it.
        port.refresh_training_readiness().await;
        Ok(settings)
    }
}

impl QueryCommand {
    pub(crate) async fn execute<P: DataCatalogPort + ?Sized>(
        mut self,
        port: &P,
    ) -> Result<QueryResult, OperationError> {
        ReadOnlyQuery::new(&self.statement, ReadOnlyQueryMode::Preview)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.max_rows = self.max_rows.clamp(1, MAX_QUERY_ROWS);
        port.query(self).await
    }
}

impl StartSpiralReplayCommand {
    pub(crate) async fn start<D, R>(self, data: &D, replay: &R) -> Result<(), OperationError>
    where
        D: DataCatalogPort + ?Sized,
        R: SpiralReplayPort + ?Sized,
    {
        self.validate()?;
        let trajectory = data
            .load_replay_trajectory(&self.episode, &self.stream)
            .await?;
        trajectory.validate()?;
        replay.start_spiral_replay(self, trajectory).await
    }

    fn validate(&self) -> Result<(), OperationError> {
        for (label, value) in [
            ("episode", self.episode.as_str()),
            ("robot", self.robot.as_str()),
            ("stream", self.stream.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(OperationError::InvalidInput(format!(
                    "Spiral replay requires a non-empty {label}"
                )));
            }
        }
        Ok(())
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), OperationError> {
    if value.trim().is_empty() || value.trim() != value || value.contains('\0') {
        return Err(OperationError::InvalidInput(format!(
            "{label} must be non-empty, contain no surrounding whitespace, and contain no NUL bytes"
        )));
    }
    Ok(())
}

impl ReplayTrajectory {
    fn validate(&self) -> Result<(), OperationError> {
        if !self.sample_hz.is_finite() || self.sample_hz <= 0.0 {
            return Err(OperationError::External(
                "Spiral returned an invalid replay sample rate".to_owned(),
            ));
        }
        if self.frames.is_empty()
            || self.frames.iter().any(|frame| {
                frame.is_empty()
                    || frame
                        .values()
                        .any(|value| !value.is_finite() || !(-1.0..=1.0).contains(value))
            })
        {
            return Err(OperationError::External(
                "Spiral returned an empty replay trajectory or a position outside normalized [-1, 1]"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

impl UpdateDataSettingsCommand {
    fn validated(mut self) -> Result<Self, OperationError> {
        self.spiral_endpoint =
            normalize_spiral_endpoint(&self.spiral_endpoint, "Spiral API endpoint")?;
        self.spiral_spfs_endpoint =
            normalize_spiral_endpoint(&self.spiral_spfs_endpoint, "Spiral SpFS endpoint")?;
        self.project_id = self.project_id.trim().to_owned();
        self.client_id = self.client_id.trim().to_owned();
        self.client_secret = self.client_secret.map(|secret| secret.trim().to_owned());
        self.training_endpoint = self.training_endpoint.trim().to_owned();

        if self.project_id.is_empty()
            || !self
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(OperationError::InvalidInput(
                "Spiral project must contain only letters, numbers, '_' or '-'".to_owned(),
            ));
        }

        if self.client_id.is_empty()
            && self
                .client_secret
                .as_ref()
                .is_some_and(|secret| !secret.is_empty())
        {
            return Err(OperationError::InvalidInput(
                "Spiral client secret requires a client ID".to_owned(),
            ));
        }

        self.training_endpoint =
            normalize_spiral_endpoint(&self.training_endpoint, "Training runtime endpoint")?;

        Ok(self)
    }
}

fn normalize_spiral_endpoint(endpoint: &str, label: &str) -> Result<String, OperationError> {
    let parsed = Url::parse(endpoint.trim()).map_err(|error| {
        OperationError::InvalidInput(format!(
            "{label} must be an http:// or https:// URL: {error}"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(OperationError::InvalidInput(format!(
            "{label} must be an http:// or https:// URL with a host"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(OperationError::InvalidInput(format!(
            "{label} must not contain embedded credentials"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(OperationError::InvalidInput(format!(
            "{label} must not contain a query string or fragment"
        )));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeData {
        events: Mutex<Vec<String>>,
        update_error: Option<OperationError>,
        query_error: Option<OperationError>,
        trajectory_error: Option<OperationError>,
        trajectory: ReplayTrajectory,
        video: Result<Vec<u8>, OperationError>,
    }

    impl FakeData {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                update_error: None,
                query_error: None,
                trajectory_error: None,
                trajectory: ReplayTrajectory {
                    sample_hz: 60.0,
                    frames: vec![BTreeMap::from([("joint".to_owned(), 0.25)])],
                },
                video: Ok(vec![1, 2, 3]),
            }
        }
    }

    impl DataCatalogPort for FakeData {
        fn settings(&self) -> DataSettingsSnapshot {
            settings_view("settings")
        }

        fn update_settings(
            &self,
            command: UpdateDataSettingsCommand,
        ) -> Result<DataSettingsSnapshot, OperationError> {
            self.events.lock().unwrap().push(format!(
                "update:{}:{}:{}:{}:{}",
                command.spiral_endpoint,
                command.spiral_spfs_endpoint,
                command.project_id,
                command.training_endpoint,
                command.video_enabled
            ));
            self.update_error
                .clone()
                .map_or_else(|| Ok(settings_view("saved")), Err)
        }

        async fn refresh_training_readiness(&self) {
            self.events.lock().unwrap().push("refresh".to_owned());
        }

        async fn test_connection(&self) -> ConnectionCheck {
            ConnectionCheck {
                ok: true,
                message: "connected".to_owned(),
            }
        }

        async fn query(&self, command: QueryCommand) -> Result<QueryResult, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("query:{}:{}", command.statement, command.max_rows));
            self.query_error.clone().map_or_else(
                || {
                    Ok(QueryResult {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        truncated: false,
                        elapsed_ms: 1,
                    })
                },
                Err,
            )
        }

        async fn load_replay_trajectory(
            &self,
            episode: &str,
            stream: &str,
        ) -> Result<ReplayTrajectory, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("load:{episode}:{stream}"));
            if let Some(error) = &self.trajectory_error {
                return Err(error.clone());
            }
            Ok(self.trajectory.clone())
        }

        async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, OperationError> {
            self.events.lock().unwrap().push(format!(
                "video:{}:{}:{:?}",
                query.episode, query.stream, query.segment
            ));
            self.video.clone()
        }
    }

    #[tokio::test]
    async fn video_query_validates_identity_before_delegating_once() {
        let data = FakeData::new();
        assert_eq!(
            VideoAssetQuery {
                episode: "episode_000001".to_owned(),
                stream: "wrist".to_owned(),
                segment: Some(2),
            }
            .load(&data)
            .await
            .unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            data.events.lock().unwrap().as_slice(),
            ["video:episode_000001:wrist:Some(2)"]
        );

        let error = VideoAssetQuery {
            episode: " episode_000001".to_owned(),
            stream: "wrist".to_owned(),
            segment: None,
        }
        .load(&data)
        .await
        .unwrap_err();
        assert!(matches!(error, OperationError::InvalidInput(_)));
        assert_eq!(data.events.lock().unwrap().len(), 1);
    }

    fn settings_view(project: &str) -> DataSettingsSnapshot {
        DataSettingsSnapshot {
            spiral: SpiralDestinationSettings {
                endpoint: "https://api.spiraldb.dev".to_owned(),
                spfs_endpoint: "https://spfs.spiraldb.dev".to_owned(),
                project_id: project.to_owned(),
                client_id: String::new(),
                client_secret_configured: false,
                namespace: "dataset".to_owned(),
                endpoint_restart_required: false,
            },
            training_endpoint: "http://127.0.0.1:50051".to_owned(),
            video_enabled: true,
        }
    }

    struct FakeReplay {
        events: Mutex<Vec<String>>,
        error: Option<OperationError>,
    }

    impl SpiralReplayPort for FakeReplay {
        async fn start_spiral_replay(
            &self,
            command: StartSpiralReplayCommand,
            trajectory: ReplayTrajectory,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push(format!(
                "start:{}:{}:{}:{}",
                command.robot,
                command.episode,
                trajectory.sample_hz,
                trajectory.frames.len()
            ));
            self.error.clone().map_or(Ok(()), Err)
        }
    }

    fn settings() -> UpdateDataSettingsCommand {
        UpdateDataSettingsCommand {
            spiral_endpoint: " https://api.spiraldb.dev/ ".to_owned(),
            spiral_spfs_endpoint: " https://spfs.spiraldb.dev/ ".to_owned(),
            project_id: " project ".to_owned(),
            client_id: String::new(),
            client_secret: None,
            training_endpoint: " http://127.0.0.1:50051 ".to_owned(),
            video_enabled: true,
        }
    }

    fn replay() -> StartSpiralReplayCommand {
        StartSpiralReplayCommand {
            episode: "episode-1".to_owned(),
            robot: "follower".to_owned(),
            stream: "leader".to_owned(),
        }
    }

    #[tokio::test]
    async fn settings_are_validated_normalized_saved_then_readiness_is_refreshed() {
        let data = FakeData::new();
        assert_eq!(settings().apply(&data).await, Ok(settings_view("saved")));
        assert_eq!(
            *data.events.lock().unwrap(),
            [
                "update:https://api.spiraldb.dev:https://spfs.spiraldb.dev:project:http://127.0.0.1:50051:true",
                "refresh"
            ]
        );
    }

    #[tokio::test]
    async fn invalid_or_unsaved_settings_never_refresh_training() {
        let data = FakeData::new();
        let mut invalid = settings();
        invalid.project_id = "not/a/project".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_endpoint = "file:///tmp/spiral".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_spfs_endpoint = "file:///tmp/spfs".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.training_endpoint = "ftp://runtime.example".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.update_error = Some(OperationError::Internal("Spiral unavailable".to_owned()));
        assert_eq!(
            settings().apply(&data).await,
            Err(OperationError::Internal("Spiral unavailable".to_owned()))
        );
        assert_eq!(data.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn query_row_limits_are_owned_by_the_capability_action() {
        let data = FakeData::new();
        QueryCommand {
            statement: "SELECT * FROM episodes".to_owned(),
            max_rows: usize::MAX,
        }
        .execute(&data)
        .await
        .unwrap();
        assert_eq!(
            *data.events.lock().unwrap(),
            ["query:SELECT * FROM episodes:1000"]
        );
    }

    #[tokio::test]
    async fn unauthorized_query_is_rejected_before_the_adapter() {
        let data = FakeData::new();
        assert!(matches!(
            QueryCommand {
                statement: "DELETE FROM episodes".to_owned(),
                max_rows: 100,
            }
            .execute(&data)
            .await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_loads_before_control_and_stops_on_external_failure() {
        let mut data = FakeData::new();
        data.trajectory_error = Some(OperationError::External("catalog offline".to_owned()));
        let control = FakeReplay {
            events: Mutex::new(Vec::new()),
            error: None,
        };
        assert_eq!(
            replay().start(&data, &control).await,
            Err(OperationError::External("catalog offline".to_owned()))
        );
        assert_eq!(*data.events.lock().unwrap(), ["load:episode-1:leader"]);
        assert!(control.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_delegates_the_loaded_trajectory_and_preserves_control_conflicts() {
        let data = FakeData::new();
        let control = FakeReplay {
            events: Mutex::new(Vec::new()),
            error: Some(OperationError::Conflict("robot busy".to_owned())),
        };
        assert_eq!(
            replay().start(&data, &control).await,
            Err(OperationError::Conflict("robot busy".to_owned()))
        );
        assert_eq!(*data.events.lock().unwrap(), ["load:episode-1:leader"]);
        assert_eq!(
            *control.events.lock().unwrap(),
            ["start:follower:episode-1:60:1"]
        );
    }

    #[tokio::test]
    async fn replay_rejects_empty_identity_and_invalid_trajectory_before_control() {
        let data = FakeData::new();
        let control = FakeReplay {
            events: Mutex::new(Vec::new()),
            error: None,
        };
        let mut command = replay();
        command.robot = "  ".to_owned();
        assert!(matches!(
            command.start(&data, &control).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.trajectory.sample_hz = f64::NAN;
        assert!(matches!(
            replay().start(&data, &control).await,
            Err(OperationError::External(_))
        ));
        assert_eq!(*data.events.lock().unwrap(), ["load:episode-1:leader"]);
        assert!(control.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.trajectory.frames[0].insert("joint".to_owned(), 1.01);
        assert!(matches!(
            replay().start(&data, &control).await,
            Err(OperationError::External(_))
        ));
        assert!(control.events.lock().unwrap().is_empty());
    }
}
