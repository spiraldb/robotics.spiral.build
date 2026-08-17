//! Framework-neutral data settings, query, and remote replay actions.

use std::collections::BTreeMap;

use spiral_robotics_core::query::{ReadOnlyMode, ensure_read_only};
use url::Url;

use super::error::ApplicationError;

const MAX_QUERY_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SpiralDestinationSettings {
    pub endpoint: String,
    pub spfs_endpoint: String,
    pub project_id: String,
    pub client_id: String,
    pub client_secret_configured: bool,
    pub namespace: String,
    pub endpoint_restart_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    pub spiral_endpoint: String,
    pub spiral_spfs_endpoint: String,
    pub project_id: String,
    pub client_id: String,
    /// `None` preserves the saved secret; `Some("")` clears it.
    pub client_secret: Option<String>,
    pub training_endpoint: String,
    pub video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsView {
    pub spiral: SpiralDestinationSettings,
    pub training_endpoint: String,
    pub video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionCheck {
    pub ok: bool,
    pub message: String,
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
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryResult {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<BTreeMap<String, QueryValue>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryCommand {
    pub statement: String,
    pub max_rows: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartSpiralReplayCommand {
    pub episode: String,
    pub robot: String,
    pub stream: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayTrajectory {
    pub sample_hz: f64,
    pub frames: Vec<BTreeMap<String, f64>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VideoAssetQuery {
    pub episode: String,
    pub stream: String,
    pub segment: Option<u32>,
}

/// Data-catalog consequences expressed only in application-owned models.
pub(crate) trait DataCatalogPort: Send + Sync {
    fn settings(&self) -> DataSettingsView;
    fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsView, ApplicationError>;
    async fn refresh_training_readiness(&self);

    async fn test_connection(&self) -> ConnectionCheck;
    async fn query(&self, command: QueryCommand) -> Result<QueryResult, ApplicationError>;
    async fn load_replay_trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<ReplayTrajectory, ApplicationError>;
    async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, ApplicationError>;
}

pub(crate) async fn video_asset<P: DataCatalogPort + ?Sized>(
    port: &P,
    query: VideoAssetQuery,
) -> Result<Vec<u8>, ApplicationError> {
    validate_identifier("episode", &query.episode)?;
    validate_identifier("stream", &query.stream)?;
    port.video_asset(query).await
}

/// Control consequence for a trajectory already admitted and loaded by the
/// data-catalog action.
pub(crate) trait SpiralReplayPort: Send + Sync {
    async fn start_spiral_replay(
        &self,
        command: StartSpiralReplayCommand,
        trajectory: ReplayTrajectory,
    ) -> Result<(), ApplicationError>;
}

pub(crate) fn get_settings<P: DataCatalogPort + ?Sized>(port: &P) -> DataSettingsView {
    port.settings()
}

pub(crate) async fn update_settings<P: DataCatalogPort + ?Sized>(
    port: &P,
    command: UpdateDataSettingsCommand,
) -> Result<DataSettingsView, ApplicationError> {
    let command = validate_settings(command)?;
    let settings = port.update_settings(command)?;
    // Training readiness depends on the selected runtime endpoint, so publish
    // preferences before rebuilding it.
    port.refresh_training_readiness().await;
    Ok(settings)
}

pub(crate) async fn test_connection<P: DataCatalogPort + ?Sized>(port: &P) -> ConnectionCheck {
    port.test_connection().await
}

pub(crate) async fn query<P: DataCatalogPort + ?Sized>(
    port: &P,
    mut command: QueryCommand,
) -> Result<QueryResult, ApplicationError> {
    ensure_read_only(&command.statement, ReadOnlyMode::Preview)
        .map_err(|error| ApplicationError::InvalidInput(error.to_string()))?;
    command.max_rows = command.max_rows.clamp(1, MAX_QUERY_ROWS);
    port.query(command).await
}

pub(crate) async fn start_spiral_replay<D, R>(
    data: &D,
    replay: &R,
    command: StartSpiralReplayCommand,
) -> Result<(), ApplicationError>
where
    D: DataCatalogPort + ?Sized,
    R: SpiralReplayPort + ?Sized,
{
    validate_replay_command(&command)?;
    let trajectory = data
        .load_replay_trajectory(&command.episode, &command.stream)
        .await?;
    validate_replay_trajectory(&trajectory)?;
    replay.start_spiral_replay(command, trajectory).await
}

fn validate_replay_command(command: &StartSpiralReplayCommand) -> Result<(), ApplicationError> {
    for (label, value) in [
        ("episode", command.episode.as_str()),
        ("robot", command.robot.as_str()),
        ("stream", command.stream.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ApplicationError::InvalidInput(format!(
                "Spiral replay requires a non-empty {label}"
            )));
        }
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), ApplicationError> {
    if value.trim().is_empty() || value.trim() != value || value.contains('\0') {
        return Err(ApplicationError::InvalidInput(format!(
            "{label} must be non-empty, contain no surrounding whitespace, and contain no NUL bytes"
        )));
    }
    Ok(())
}

fn validate_replay_trajectory(trajectory: &ReplayTrajectory) -> Result<(), ApplicationError> {
    if !trajectory.sample_hz.is_finite() || trajectory.sample_hz <= 0.0 {
        return Err(ApplicationError::External(
            "Spiral returned an invalid replay sample rate".to_owned(),
        ));
    }
    if trajectory.frames.is_empty()
        || trajectory.frames.iter().any(|frame| {
            frame.is_empty()
                || frame
                    .values()
                    .any(|value| !value.is_finite() || !(-1.0..=1.0).contains(value))
        })
    {
        return Err(ApplicationError::External(
            "Spiral returned an empty replay trajectory or a position outside normalized [-1, 1]"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_settings(
    mut command: UpdateDataSettingsCommand,
) -> Result<UpdateDataSettingsCommand, ApplicationError> {
    command.spiral_endpoint =
        normalize_spiral_endpoint(&command.spiral_endpoint, "Spiral API endpoint")?;
    command.spiral_spfs_endpoint =
        normalize_spiral_endpoint(&command.spiral_spfs_endpoint, "Spiral SpFS endpoint")?;
    command.project_id = command.project_id.trim().to_owned();
    command.client_id = command.client_id.trim().to_owned();
    command.client_secret = command.client_secret.map(|secret| secret.trim().to_owned());
    command.training_endpoint = command.training_endpoint.trim().to_owned();

    if command.project_id.is_empty()
        || !command
            .project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ApplicationError::InvalidInput(
            "Spiral project must contain only letters, numbers, '_' or '-'".to_owned(),
        ));
    }

    if command.client_id.is_empty()
        && command
            .client_secret
            .as_ref()
            .is_some_and(|secret| !secret.is_empty())
    {
        return Err(ApplicationError::InvalidInput(
            "Spiral client secret requires a client ID".to_owned(),
        ));
    }

    command.training_endpoint =
        normalize_spiral_endpoint(&command.training_endpoint, "Training runtime endpoint")?;

    Ok(command)
}

fn normalize_spiral_endpoint(endpoint: &str, label: &str) -> Result<String, ApplicationError> {
    let parsed = Url::parse(endpoint.trim()).map_err(|error| {
        ApplicationError::InvalidInput(format!(
            "{label} must be an http:// or https:// URL: {error}"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ApplicationError::InvalidInput(format!(
            "{label} must be an http:// or https:// URL with a host"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ApplicationError::InvalidInput(format!(
            "{label} must not contain embedded credentials"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ApplicationError::InvalidInput(format!(
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
        update_error: Option<ApplicationError>,
        query_error: Option<ApplicationError>,
        trajectory_error: Option<ApplicationError>,
        trajectory: ReplayTrajectory,
        video: Result<Vec<u8>, ApplicationError>,
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
        fn settings(&self) -> DataSettingsView {
            settings_view("settings")
        }

        fn update_settings(
            &self,
            command: UpdateDataSettingsCommand,
        ) -> Result<DataSettingsView, ApplicationError> {
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

        async fn query(&self, command: QueryCommand) -> Result<QueryResult, ApplicationError> {
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
        ) -> Result<ReplayTrajectory, ApplicationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("load:{episode}:{stream}"));
            if let Some(error) = &self.trajectory_error {
                return Err(error.clone());
            }
            Ok(self.trajectory.clone())
        }

        async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, ApplicationError> {
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
            video_asset(
                &data,
                VideoAssetQuery {
                    episode: "episode_000001".to_owned(),
                    stream: "wrist".to_owned(),
                    segment: Some(2),
                },
            )
            .await
            .unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            data.events.lock().unwrap().as_slice(),
            ["video:episode_000001:wrist:Some(2)"]
        );

        let error = video_asset(
            &data,
            VideoAssetQuery {
                episode: " episode_000001".to_owned(),
                stream: "wrist".to_owned(),
                segment: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ApplicationError::InvalidInput(_)));
        assert_eq!(data.events.lock().unwrap().len(), 1);
    }

    fn settings_view(project: &str) -> DataSettingsView {
        DataSettingsView {
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
        error: Option<ApplicationError>,
    }

    impl SpiralReplayPort for FakeReplay {
        async fn start_spiral_replay(
            &self,
            command: StartSpiralReplayCommand,
            trajectory: ReplayTrajectory,
        ) -> Result<(), ApplicationError> {
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
        assert_eq!(
            update_settings(&data, settings()).await,
            Ok(settings_view("saved"))
        );
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
            update_settings(&data, invalid).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_endpoint = "file:///tmp/spiral".to_owned();
        assert!(matches!(
            update_settings(&data, invalid).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_spfs_endpoint = "file:///tmp/spfs".to_owned();
        assert!(matches!(
            update_settings(&data, invalid).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.training_endpoint = "ftp://runtime.example".to_owned();
        assert!(matches!(
            update_settings(&data, invalid).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.update_error = Some(ApplicationError::Internal("Spiral unavailable".to_owned()));
        assert_eq!(
            update_settings(&data, settings()).await,
            Err(ApplicationError::Internal("Spiral unavailable".to_owned()))
        );
        assert_eq!(data.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn query_row_limits_are_owned_by_the_application_action() {
        let data = FakeData::new();
        query(
            &data,
            QueryCommand {
                statement: "SELECT * FROM episodes".to_owned(),
                max_rows: usize::MAX,
            },
        )
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
            query(
                &data,
                QueryCommand {
                    statement: "DELETE FROM episodes".to_owned(),
                    max_rows: 100,
                },
            )
            .await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_loads_before_control_and_stops_on_external_failure() {
        let mut data = FakeData::new();
        data.trajectory_error = Some(ApplicationError::External("catalog offline".to_owned()));
        let control = FakeReplay {
            events: Mutex::new(Vec::new()),
            error: None,
        };
        assert_eq!(
            start_spiral_replay(&data, &control, replay()).await,
            Err(ApplicationError::External("catalog offline".to_owned()))
        );
        assert_eq!(*data.events.lock().unwrap(), ["load:episode-1:leader"]);
        assert!(control.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_delegates_the_loaded_trajectory_and_preserves_control_conflicts() {
        let data = FakeData::new();
        let control = FakeReplay {
            events: Mutex::new(Vec::new()),
            error: Some(ApplicationError::Conflict("robot busy".to_owned())),
        };
        assert_eq!(
            start_spiral_replay(&data, &control, replay()).await,
            Err(ApplicationError::Conflict("robot busy".to_owned()))
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
            start_spiral_replay(&data, &control, command).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.trajectory.sample_hz = f64::NAN;
        assert!(matches!(
            start_spiral_replay(&data, &control, replay()).await,
            Err(ApplicationError::External(_))
        ));
        assert_eq!(*data.events.lock().unwrap(), ["load:episode-1:leader"]);
        assert!(control.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.trajectory.frames[0].insert("joint".to_owned(), 1.01);
        assert!(matches!(
            start_spiral_replay(&data, &control, replay()).await,
            Err(ApplicationError::External(_))
        ));
        assert!(control.events.lock().unwrap().is_empty());
    }
}
