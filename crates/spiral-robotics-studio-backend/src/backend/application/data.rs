//! Framework-neutral data settings, publication, query, and remote replay actions.

use std::collections::BTreeMap;

use spiral_robotics_core::query::{ReadOnlyMode, ensure_read_only};

use super::error::ApplicationError;

const MAX_QUERY_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SpiralDestinationSettings {
    pub uri: String,
    pub project_id: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    pub spiral: SpiralDestinationSettings,
    pub training_endpoint: String,
    pub video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsView {
    pub spiral: SpiralDestinationSettings,
    pub spiral_configured: bool,
    pub training_endpoint: String,
    pub video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionCheck {
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicationView {
    pub episode_id: String,
    pub state: String,
    pub spiral: Option<String>,
    pub updated_ms: i64,
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
    fn publications(&self) -> Vec<PublicationView>;
    fn sync_existing_episode(&self, episode: String) -> Result<(), ApplicationError>;
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
    // Training readiness is derived from the saved destination, so publishing
    // the settings must happen before rebuilding the readiness snapshot.
    port.refresh_training_readiness().await;
    Ok(settings)
}

pub(crate) async fn test_connection<P: DataCatalogPort + ?Sized>(port: &P) -> ConnectionCheck {
    port.test_connection().await
}

pub(crate) fn list_publications<P: DataCatalogPort + ?Sized>(port: &P) -> Vec<PublicationView> {
    port.publications()
}

pub(crate) fn sync_existing_episode<P: DataCatalogPort + ?Sized>(
    port: &P,
    episode: String,
) -> Result<(), ApplicationError> {
    port.sync_existing_episode(episode)
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
    command.spiral.uri = command.spiral.uri.trim().to_owned();
    command.spiral.project_id = command.spiral.project_id.trim().to_owned();
    command.spiral.namespace = command.spiral.namespace.trim().to_owned();
    command.training_endpoint = command.training_endpoint.trim().to_owned();

    if !command.training_endpoint.starts_with("http://")
        || command.training_endpoint.len() <= "http://".len()
    {
        return Err(ApplicationError::InvalidInput(
            "Training runtime endpoint must be a plain http:// address".to_owned(),
        ));
    }

    if !matches!(
        command.spiral.uri.as_str(),
        "spiraldb://" | "spiral:memory:"
    ) {
        return Err(ApplicationError::InvalidInput(
            "Spiral ADBC URI must be 'spiraldb://' or 'spiral:memory:'".to_owned(),
        ));
    }
    let configured =
        command.spiral.uri == "spiral:memory:" || !command.spiral.project_id.is_empty();
    if configured {
        if command.spiral.uri != "spiral:memory:"
            && !command
                .spiral
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ApplicationError::InvalidInput(
                "Spiral project must contain only letters, numbers, '_' or '-'".to_owned(),
            ));
        }
        let mut namespace = command.spiral.namespace.bytes();
        if !namespace
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            || !namespace.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(ApplicationError::InvalidInput(
                "Spiral namespace must start with a letter or '_' and contain only letters, numbers, or '_'"
                    .to_owned(),
            ));
        }
    }
    Ok(command)
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
                "update:{}:{}:{}",
                command.spiral.uri, command.spiral.project_id, command.spiral.namespace
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

        fn publications(&self) -> Vec<PublicationView> {
            vec![PublicationView {
                episode_id: "episode".to_owned(),
                state: "published".to_owned(),
                spiral: None,
                updated_ms: 1,
            }]
        }

        fn sync_existing_episode(&self, episode: String) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push(format!("sync:{episode}"));
            Ok(())
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
                uri: "spiral:memory:".to_owned(),
                project_id: project.to_owned(),
                namespace: "dataset".to_owned(),
            },
            spiral_configured: true,
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
            spiral: SpiralDestinationSettings {
                uri: " spiraldb:// ".to_owned(),
                project_id: " project-1 ".to_owned(),
                namespace: " robot_data ".to_owned(),
            },
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
            ["update:spiraldb://:project-1:robot_data", "refresh"]
        );
    }

    #[tokio::test]
    async fn invalid_or_unsaved_settings_never_refresh_training() {
        let data = FakeData::new();
        let mut invalid = settings();
        invalid.spiral.namespace = "robot-data".to_owned();
        assert!(matches!(
            update_settings(&data, invalid).await,
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.update_error = Some(ApplicationError::Internal("disk full".to_owned()));
        assert_eq!(
            update_settings(&data, settings()).await,
            Err(ApplicationError::Internal("disk full".to_owned()))
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
