use super::*;
use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutPhase, RolloutSnapshot};
use spiral_robotics_storage::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationMetadata,
    RelationName, SqlStatement,
};
use std::sync::Mutex as StdMutex;
use tokio::sync::{Notify, Semaphore};

#[derive(Default)]
struct LoadingQueries {
    reads: Vec<String>,
    schemas: Vec<String>,
    rollout: Vec<RecordBatch>,
    fail_rollout: bool,
    fail_preferences: bool,
    block_preferences: bool,
    robots: Option<Vec<RecordBatch>>,
}

struct LoadingDatabase {
    queries: StdMutex<LoadingQueries>,
    rollout_started: Notify,
    rollout_release: Semaphore,
    preferences_started: Notify,
    preferences_release: Semaphore,
}

impl LoadingDatabase {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queries: StdMutex::new(LoadingQueries::default()),
            rollout_started: Notify::new(),
            rollout_release: Semaphore::new(0),
            preferences_started: Notify::new(),
            preferences_release: Semaphore::new(0),
        })
    }

    fn rollout_reads(&self) -> usize {
        self.queries
            .lock()
            .unwrap()
            .reads
            .iter()
            .filter(|sql| sql.contains("_rollout_runs"))
            .count()
    }
}

struct LoadingDatabaseAdapter(Arc<LoadingDatabase>);

impl Database for LoadingDatabaseAdapter {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move { Ok(Box::new(LoadingConnection(self.0.clone())) as Box<dyn DatabaseConnection>) }.boxed()
    }
}

struct LoadingConnection(Arc<LoadingDatabase>);

impl DatabaseSession for LoadingConnection {
    fn query_schema(&mut self, _: SqlStatement) -> DatabaseFuture<'_, Arc<Schema>> {
        async {
            Err(DatabaseError::new(
                DatabaseErrorKind::InvalidState,
                "project loading must not plan workflow queries",
            ))
        }
        .boxed()
    }

    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move {
            self.0
                .queries
                .lock()
                .unwrap()
                .reads
                .push(sql.as_str().to_owned());
            let batches = if sql.as_str().contains("_settings") {
                let (failed, blocked) = {
                    let queries = self.0.queries.lock().unwrap();
                    (queries.fail_preferences, queries.block_preferences)
                };
                if failed {
                    return Err(DatabaseError::new(
                        DatabaseErrorKind::InvalidState,
                        "preferences unavailable",
                    ));
                }
                if blocked {
                    self.0.preferences_started.notify_one();
                    self.0.preferences_release.acquire().await.unwrap().forget();
                }

                vec![RecordBatch::new_empty(Arc::new(Schema::new(vec![
                    Field::new("value_json", DataType::Utf8, false),
                ])))]
            } else if sql.as_str().contains("_rollout_runs") {
                if self.0.queries.lock().unwrap().fail_rollout {
                    return Err(DatabaseError::new(
                        DatabaseErrorKind::InvalidState,
                        "rollout history unavailable",
                    ));
                }
                self.0.rollout_started.notify_one();
                self.0.rollout_release.acquire().await.unwrap().forget();
                self.0.queries.lock().unwrap().rollout.clone()
            } else if sql.as_str().contains("_robots") {
                self.0
                    .queries
                    .lock()
                    .unwrap()
                    .robots
                    .clone()
                    .ok_or_else(|| {
                        DatabaseError::new(
                            DatabaseErrorKind::InvalidState,
                            "robot configuration unavailable",
                        )
                    })?
            } else {
                return Err(DatabaseError::new(
                    DatabaseErrorKind::InvalidState,
                    "robot configuration unavailable",
                ));
            };
            let schema = batches
                .first()
                .map(RecordBatch::schema)
                .unwrap_or_else(|| Arc::new(Schema::empty()));
            Ok(RecordBatchStream::new(
                schema,
                futures::stream::iter(batches.into_iter().map(Ok)).boxed(),
            ))
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        self.0
            .queries
            .lock()
            .unwrap()
            .schemas
            .push(relation.as_str().to_owned());
        async {
            Err(DatabaseError::new(
                DatabaseErrorKind::InvalidState,
                "schema validation requested",
            ))
        }
        .boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let (relation, _, mut stream) = ingestion.into_parts();
            assert!(relation.as_str().ends_with("_rollout_runs"));
            let mut batches = Vec::new();
            while let Some(batch) = stream.next().await {
                batches.push(batch?);
            }
            self.0.queries.lock().unwrap().rollout = batches;
            Ok(ExecutionSummary)
        }
        .boxed()
    }

    fn execute_update(&mut self, _: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        panic!("loading must not execute updates")
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        panic!("loading must not enumerate the catalog")
    }
}

impl DatabaseConnection for LoadingConnection {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        panic!("loading must not open transactions")
    }
}

struct LoadingFixture {
    project: tests::ProjectFixture,
    session: Arc<ProjectSession>,
    database: Arc<LoadingDatabase>,
}

impl LoadingFixture {
    fn new() -> Self {
        let project = tests::ProjectFixture::new();
        let database = LoadingDatabase::new();
        let connection = project
            .projects
            .active()
            .storage()
            .connection_settings()
            .with_project("loading_test")
            .unwrap();
        let store = Arc::new(
            ProjectStore::new(
                connection,
                "project_lifetime_test".to_owned(),
                Arc::new(LoadingDatabaseAdapter(database.clone())),
            )
            .unwrap(),
        );
        let session = project.projects.factory.create_session(
            ProjectSessionInfo::new("loading-session", "loading_test"),
            store,
        );
        Self {
            project,
            session,
            database,
        }
    }

    async fn seed_interrupted_rollout(&self) {
        self.session
            .storage()
            .save_rollout_session(
                &RolloutSnapshot::new(
                    Some("interrupted-session".to_owned()),
                    Some("training-run".to_owned()),
                )
                .with_lifecycle(RolloutPhase::Running, Some(10)),
            )
            .await
            .unwrap();
    }

    async fn shutdown(self) {
        self.session.shutdown().await;
        self.project.projects.hardware().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn resume_does_no_database_io_and_defers_robot_failure_to_its_feature() {
    let fixture = LoadingFixture::new();
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    {
        let queries = fixture.database.queries.lock().unwrap();
        assert!(queries.schemas.is_empty());
        assert!(queries.reads.is_empty());
    }
    assert_eq!(
        fixture.session.readiness().current().state(),
        BackendReadinessState::Ready
    );
    assert!(fixture.session.workspace().try_list().await.is_err());
    assert_eq!(
        fixture.session.readiness().current().state(),
        BackendReadinessState::Ready
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn explicit_validation_still_checks_all_managed_schemas_before_loading() {
    let fixture = LoadingFixture::new();
    let error = fixture
        .session
        .prepare(ProjectPreparation::Validate)
        .await
        .unwrap_err();
    assert!(error.message().contains("schema validation requested"));
    {
        let queries = fixture.database.queries.lock().unwrap();
        assert_eq!(queries.schemas.len(), 9);
        assert!(queries.reads.is_empty());
    }
    assert!(!fixture.session.readiness().current().catalog_available());
    fixture.shutdown().await;
}

#[tokio::test]
async fn resuming_without_a_saved_project_keeps_setup_unavailable() {
    let fixture = tests::ProjectFixture::new();
    let original = fixture.projects.active();
    assert!(fixture.projects.resume(&original).await.is_err());
    assert!(Arc::ptr_eq(&fixture.projects.active(), &original));
    assert!(!original.readiness().current().catalog_available());
    fixture.projects.hardware().shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_rollout_reads_restore_once_and_never_resume_interrupted_motion() {
    let fixture = LoadingFixture::new();
    fixture.seed_interrupted_rollout().await;
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    assert_eq!(fixture.database.rollout_reads(), 0);
    assert_eq!(
        fixture.session.rollout().snapshot().phase(),
        RolloutPhase::Idle
    );
    fixture.database.rollout_release.add_permits(1);
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::try_join!(
            fixture.session.rollout().load_snapshot(),
            fixture.session.rollout().load_snapshot()
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.session_id(), Some("interrupted-session"));
    assert_eq!(first.phase(), RolloutPhase::Failed);
    assert!(!fixture.session.rollout().is_active());
    assert_eq!(fixture.database.rollout_reads(), 1);
    assert_eq!(
        fixture.session.rollout().load_snapshot().await.unwrap(),
        first
    );
    assert_eq!(fixture.database.rollout_reads(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn rollout_failure_is_scoped_and_can_be_retried_by_a_later_request() {
    let fixture = LoadingFixture::new();
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    fixture.database.queries.lock().unwrap().fail_rollout = true;
    assert!(fixture.session.rollout().load_snapshot().await.is_err());
    assert_eq!(fixture.database.rollout_reads(), 1);
    assert!(fixture.session.readiness().current().catalog_available());
    fixture.database.queries.lock().unwrap().fail_rollout = false;
    fixture.database.rollout_release.add_permits(1);
    assert_eq!(
        fixture.session.rollout().load_snapshot().await.unwrap(),
        RolloutSnapshot::default()
    );
    assert_eq!(fixture.database.rollout_reads(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancelled_rollout_loading_does_not_poison_the_project_scoped_state() {
    let fixture = LoadingFixture::new();
    let rollout = fixture.session.rollout().clone();
    let loading = tokio::spawn(async move { rollout.load_snapshot().await });
    fixture.database.rollout_started.notified().await;
    loading.abort();
    assert!(loading.await.unwrap_err().is_cancelled());
    fixture.database.rollout_release.add_permits(1);
    fixture.session.rollout().load_snapshot().await.unwrap();
    assert_eq!(fixture.database.rollout_reads(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn starting_without_opening_rollout_loads_history_before_admission() {
    let fixture = LoadingFixture::new();
    fixture.seed_interrupted_rollout().await;
    fixture.database.rollout_release.add_permits(1);
    // Invalid duration rejects admission before a hardware task can start.
    let error = fixture
        .session
        .rollout()
        .start(RolloutConfig::new("training-run", 1, "follower").with_duration(0))
        .await
        .unwrap_err();
    assert!(error.contains("duration"), "{error}");
    assert_eq!(fixture.database.rollout_reads(), 1);
    assert_eq!(
        fixture.session.rollout().snapshot().session_id(),
        Some("interrupted-session")
    );
    assert_eq!(
        fixture.session.rollout().snapshot().phase(),
        RolloutPhase::Failed
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn preferences_are_loaded_once_and_failure_stays_local_to_the_requesting_feature() {
    let fixture = LoadingFixture::new();
    fixture.database.queries.lock().unwrap().fail_preferences = true;
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    assert!(fixture.session.preferences().loaded().is_err());
    assert_eq!(
        fixture.session.connection_settings().project_id(),
        "loading_test"
    );
    assert!(fixture.database.queries.lock().unwrap().reads.is_empty());
    assert!(
        fixture
            .session
            .settings()
            .await
            .unwrap_err()
            .to_string()
            .contains("preferences unavailable")
    );
    assert!(fixture.session.readiness().current().catalog_available());
    fixture.database.queries.lock().unwrap().fail_preferences = false;
    let (first, second) = tokio::join!(fixture.session.settings(), fixture.session.settings());
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(fixture.database.queries.lock().unwrap().reads.len(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancelled_preference_loading_can_restart_and_saved_edits_win_over_an_inflight_read() {
    let fixture = LoadingFixture::new();
    fixture.database.queries.lock().unwrap().block_preferences = true;
    let preferences = fixture.session.preferences().clone();
    let first = tokio::spawn(async move { preferences.load().await });
    fixture.database.preferences_started.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(fixture.session.preferences().loaded().is_err());
    let preferences = fixture.session.preferences().clone();
    let second = tokio::spawn(async move { preferences.load().await });
    fixture.database.preferences_started.notified().await;
    let preferences = fixture.session.preferences().clone();
    let save = tokio::spawn(async move {
        preferences
            .replace(
                spiral_robotics_storage::StoredStudioPreferences::from_settings(
                    "http://runtime.example:5050",
                    false,
                )
                .unwrap(),
            )
            .await;
    });
    fixture.database.preferences_release.add_permits(1);
    second.await.unwrap().unwrap();
    save.await.unwrap();
    let saved = fixture.session.preferences().load().await.unwrap();
    assert_eq!(saved.training().endpoint(), "http://runtime.example:5050");
    assert!(!saved.video().enabled());
    assert_eq!(fixture.database.queries.lock().unwrap().reads.len(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn auto_connect_reads_saved_robots_once() {
    let fixture = LoadingFixture::new();
    let schema = Arc::new(Schema::new(vec![
        Field::new("robot_id", DataType::Utf8, false),
        Field::new("config_json", DataType::Utf8, false),
    ]));
    fixture.database.queries.lock().unwrap().robots = Some(vec![
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["left", "right"])),
                Arc::new(StringArray::from(vec!["{}", "{}"])),
            ],
        )
        .unwrap(),
    ]);
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    fixture
        .project
        .projects
        .active
        .send_replace(fixture.session.clone());
    let context =
        crate::project_session::context::ProjectContext::new(fixture.project.projects.clone());
    crate::robots::commands::DeviceCommands::from(context)
        .auto_connect_saved_robots()
        .await;
    assert_eq!(fixture.database.queries.lock().unwrap().reads.len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn robot_setup_errors_do_not_change_project_readiness_or_leak_to_a_new_session() {
    let fixture = LoadingFixture::new();
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    fixture
        .project
        .projects
        .active
        .send_replace(fixture.session.clone());
    let context =
        crate::project_session::context::ProjectContext::new(fixture.project.projects.clone());
    let devices = crate::robots::commands::DeviceCommands::from(context);
    assert!(
        devices
            .setup()
            .await
            .unwrap_err()
            .to_string()
            .contains("robot configuration unavailable")
    );
    assert!(fixture.session.readiness().current().catalog_available());
    let next = fixture.project.replace("another_project");
    assert!(devices.setup().await.is_err());
    assert!(next.preferences().loaded().is_err());
    fixture.shutdown().await;
}

#[tokio::test]
async fn direct_training_and_recording_requests_fail_before_starting_with_unloaded_preferences() {
    let fixture = LoadingFixture::new();
    fixture.database.queries.lock().unwrap().fail_preferences = true;
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    fixture
        .project
        .projects
        .active
        .send_replace(fixture.session.clone());
    let context =
        crate::project_session::context::ProjectContext::new(fixture.project.projects.clone());
    let recording = crate::recording::commands::RecordingCommands::from(context.clone());
    assert!(
        recording
            .start_recording("test_dataset".to_owned())
            .await
            .unwrap_err()
            .to_string()
            .contains("preferences unavailable")
    );
    let training = crate::training::commands::TrainingCommands::from(context);
    assert!(
        training
            .snapshot(None)
            .await
            .unwrap_err()
            .to_string()
            .contains("preferences unavailable")
    );
    assert!(
        fixture
            .database
            .queries
            .lock()
            .unwrap()
            .reads
            .iter()
            .all(|sql| sql.contains("_settings"))
    );
    assert!(
        fixture
            .project
            .projects
            .hardware()
            .state()
            .recording()
            .is_none()
    );
    let runtime = crate::training::remote::ConfiguredHttpTrainingBackend::new(
        fixture.session.preferences().clone(),
    );
    assert!(spiral_robotics_training::TrainingBackend::health(&runtime).is_err());
    fixture.shutdown().await;
}
