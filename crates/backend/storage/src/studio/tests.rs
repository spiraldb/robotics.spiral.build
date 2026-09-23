use super::*;
use crate::rollout_runs::RolloutSessionPersistence;
use crate::settings::{StoredStudioPreferences, StudioPreferencePersistence};
use crate::workflows::{WorkflowJson, WorkflowPersistence};
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, QueryFilter, Statement};
use spiral_robotics_common::robots::PortSettings;
use spiral_robotics_common::robots::config::RobotConfig;
use spiral_robotics_common::rollout::{RolloutPhase, RolloutSnapshot};
use spiral_robotics_common::workflow::WorkflowSpec;

struct Fixture;
impl Fixture {
    fn connection(project: &str) -> crate::SpiralConnectionSettings {
        crate::SpiralConnectionSettings::new(
            "http://127.0.0.1:1".into(),
            "http://127.0.0.1:2".into(),
        )
        .unwrap()
        .with_project(project)
        .unwrap()
    }
    fn robot(id: &str) -> RobotConfig {
        RobotConfig::so101(id, "so101_follower")
            .and_then(|config| config.with_port(PortSettings::new("/dev/arm", 1_000_000)))
            .unwrap()
    }
    fn graph(workflow: &WorkflowSpec) -> WorkflowJson {
        WorkflowJson::new(4, serde_json::json!({"workflow":workflow}).to_string())
    }
    fn conversation() -> WorkflowJson {
        WorkflowJson::new(1, "[]".into())
    }
    async fn sql(store: &StudioStore, sql: &str) -> Result<(), String> {
        store
            .connection()
            .await?
            .execute_unprepared(sql)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[tokio::test]
async fn dataset_models_follow_dataset_revision_and_project() {
    let store = StudioStore::in_memory();
    let project = store.project(&Fixture::connection("models"));
    let identity = ("dataset-id", "revision-1");
    assert_eq!(project.dataset_robot(identity).await.unwrap(), None);
    project.save_dataset_robot(identity, None).await.unwrap();
    assert_eq!(project.dataset_robot(identity).await.unwrap(), None);
    assert_eq!(
        project
            .dataset_robot((identity.0, "revision-2"))
            .await
            .unwrap(),
        None
    );
    let robot = serde_json::from_value(serde_json::json!({
        "urdf":"<robot name=\"fixture\"><link name=\"base\"/></robot>",
        "bindings":[], "meshes":{"arm.stl":"Zml4dHVyZQ=="}
    }))
    .unwrap();
    project
        .save_dataset_robot(identity, Some(&robot))
        .await
        .unwrap();
    assert_eq!(
        project.dataset_robot(identity).await.unwrap(),
        Some(robot.clone())
    );
    let other = store.project(&Fixture::connection("other"));
    assert_eq!(other.dataset_robot(identity).await.unwrap(), None);
    let archive = StudioSnapshot::capture(&project).await.unwrap();
    StudioSnapshot::open(&archive.bytes().unwrap(), &other)
        .await
        .unwrap()
        .stage(&other)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(other.dataset_robot(identity).await.unwrap(), Some(robot));
}

#[tokio::test]
async fn local_state_survives_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Studio state #1.sqlite3");
    let store = StudioStore::at(path.clone());
    let connection = Fixture::connection("bench");
    let project = store.project(&connection);
    assert_eq!(
        project
            .save_robot_config(&Fixture::robot("arm"), None)
            .await
            .unwrap(),
        Some(1)
    );
    StudioPreferencePersistence::new(&project)
        .save(&StoredStudioPreferences::from_settings("http://localhost:9", false).unwrap())
        .await
        .unwrap();
    for session in ["run", "second_run"] {
        RolloutSessionPersistence::new(&project)
            .save(
                &RolloutSnapshot::new(Some(session.into()), None)
                    .with_lifecycle(RolloutPhase::Failed, Some(123)),
            )
            .await
            .unwrap();
    }
    let reopened = StudioStore::at(path);
    let recovered = reopened.project(&connection);
    assert_eq!(
        recovered.robot_config("arm").await.unwrap(),
        project.robot_config("arm").await.unwrap()
    );
    assert_eq!(
        StudioPreferencePersistence::new(&recovered)
            .load()
            .await
            .unwrap()
            .training()
            .endpoint(),
        "http://localhost:9"
    );
    assert_eq!(
        RolloutSessionPersistence::new(&recovered)
            .load()
            .await
            .unwrap()
            .iter()
            .map(RolloutSnapshot::session_id)
            .collect::<Vec<_>>(),
        [Some("run"), Some("second_run")]
    );
    let other = reopened.project(&Fixture::connection("other"));
    assert!(other.robot_configs().await.unwrap().is_empty());
    assert!(
        RolloutSessionPersistence::new(&other)
            .load()
            .await
            .unwrap()
            .is_empty()
    );
    let db = reopened.connection().await.unwrap();
    let journal = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA journal_mode",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        journal.try_get::<String>("", "journal_mode").unwrap(),
        "wal"
    );
    let foreign_keys = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_keys",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(foreign_keys.try_get::<i64>("", "foreign_keys").unwrap(), 1);
}

#[tokio::test]
async fn concurrent_creations_and_stale_robot_updates_do_not_overwrite() {
    let store = StudioStore::in_memory();
    let project = store.project(&Fixture::connection("bench"));
    let original = Fixture::robot("arm");
    let (first, second) = tokio::join!(
        project.save_robot_config(&original, None),
        project.save_robot_config(&original, None)
    );
    assert_ne!(first.unwrap(), second.unwrap());
    let port = |path: &str| {
        original
            .clone()
            .with_port(PortSettings::new(path, 1_000_000))
    };
    let changed = port("/dev/changed").unwrap();
    assert_eq!(
        project.save_robot_config(&changed, Some(1)).await.unwrap(),
        Some(2)
    );
    assert_eq!(
        project
            .save_robot_config(&port("/dev/stale").unwrap(), Some(1))
            .await
            .unwrap(),
        None
    );
    Fixture::sql(&store, "CREATE TRIGGER reject_robot BEFORE UPDATE ON robots BEGIN SELECT RAISE(ABORT, 'test failure'); END;").await.unwrap();
    assert!(
        project
            .save_robot_config(&port("/dev/failed").unwrap(), Some(2))
            .await
            .is_err()
    );
    assert_eq!(
        project.robot_config("arm").await.unwrap(),
        Some((changed, 2))
    );
}

#[tokio::test]
async fn workflow_updates_are_atomic_and_scoped_to_one_revision_and_project() {
    let store = StudioStore::in_memory();
    let project = store.project(&Fixture::connection("bench"));
    let workflow = WorkflowSpec::new("workflow", "First").with_revision(1);
    let persistence = WorkflowPersistence::new(&project);
    let graph = Fixture::graph(&workflow);
    let conversation = Fixture::conversation();
    assert!(
        persistence
            .save(&workflow, (&graph, &conversation))
            .await
            .unwrap()
    );
    let mut next = workflow.clone().with_revision(2);
    next.set_name("Second");
    let graph2 = Fixture::graph(&next);
    let (one, two) = tokio::join!(
        persistence.save(&next, (&graph2, &conversation)),
        persistence.save(&next, (&graph2, &conversation))
    );
    assert_ne!(one.unwrap(), two.unwrap());
    assert!(
        !persistence
            .save(&workflow, (&graph, &conversation))
            .await
            .unwrap()
    );
    assert_eq!(
        persistence.load("workflow").await.unwrap().unwrap().graph(),
        &graph2
    );
    assert_eq!(persistence.list().await.unwrap()[0].name(), "Second");
    let other = store.project(&Fixture::connection("other"));
    assert!(
        WorkflowPersistence::new(&other)
            .load("workflow")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn migration_runs_once_and_failed_schema_changes_roll_back() {
    let store = StudioStore::in_memory();
    store.initialize().await.unwrap();
    store.initialize().await.unwrap();
    let db = store.connection().await.unwrap();
    let before = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT count(*) AS count FROM seaql_migrations",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.try_get::<i64>("", "count").unwrap(), 8);
    // A conflict halfway through the actual initial migration must undo both
    // earlier CREATE TABLE statements and the migration journal itself.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("failed-upgrade.sqlite3");
    let raw = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    raw.execute_unprepared("CREATE TABLE workflows (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    let failed = StudioStore::at(path);
    assert!(failed.initialize().await.is_err());
    let result = raw
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE name IN ('robots', 'backend_settings', 'seaql_migrations')",
        ))
        .await
        .unwrap();
    assert!(result.is_none());
    raw.execute_unprepared("DROP TABLE workflows")
        .await
        .unwrap();
    failed.initialize().await.unwrap();
}

#[tokio::test]
async fn project_restore_is_atomic_and_preserves_other_projects() {
    let store = StudioStore::in_memory();
    let source = store.project(&Fixture::connection("source"));
    let target = store.project(&Fixture::connection("target"));
    let document = (Fixture::robot("arm"), 1);
    source.save_robot_config(&document.0, None).await.unwrap();
    StudioPreferencePersistence::new(&source)
        .save(&StoredStudioPreferences::default())
        .await
        .unwrap();
    let archive = StudioSnapshot::capture(&source).await.unwrap();
    let imported = StudioSnapshot::open(&archive.bytes().unwrap(), &target)
        .await
        .unwrap();
    imported
        .stage(&target)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(
        target.robot_config("arm").await.unwrap(),
        Some(document.clone())
    );
    // A later feature failure must undo earlier robot deletion/insertion too.
    Fixture::sql(&store, "CREATE TRIGGER reject_preferences BEFORE INSERT ON studio_settings BEGIN SELECT RAISE(ABORT, 'restore failed'); END;").await.unwrap();
    let failed = imported.stage(&target).await;
    assert!(failed.is_err());
    assert_eq!(
        target.robot_config("arm").await.unwrap(),
        Some(document.clone())
    );
    assert_eq!(source.robot_config("arm").await.unwrap(), Some(document));
    assert_eq!(
        robots::Entity::find()
            .filter(robots::Column::Project.eq(target.key()))
            .all(store.connection().await.unwrap())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn the_latest_rollout_session_carries_over_under_its_identity() {
    use sea_orm_migration::MigratorTrait as _;
    let current = StudioStore::in_memory();
    let bench = current.project(&Fixture::connection("bench"));
    RolloutSessionPersistence::new(&bench)
        .save(
            &RolloutSnapshot::new(Some("rollout-1".into()), Some("run".into()))
                .with_lifecycle(RolloutPhase::Failed, Some(123)),
        )
        .await
        .unwrap();
    let row = current
        .connection()
        .await
        .unwrap()
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT project, document FROM rollout_runs",
        ))
        .await
        .unwrap()
        .unwrap();
    let (project, document) = (
        row.try_get::<String>("", "project").unwrap(),
        row.try_get::<String>("", "document").unwrap(),
    );

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rollouts.sqlite3");
    let raw = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    super::migrations::Migrator::up(&raw, Some(6))
        .await
        .unwrap();
    raw.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO rollout_runs (project, document) VALUES (?, ?)",
        [project.into(), document.into()],
    ))
    .await
    .unwrap();
    super::migrations::Migrator::up(&raw, None).await.unwrap();

    let migrated = StudioStore::at(path).project(&Fixture::connection("bench"));
    let sessions = RolloutSessionPersistence::new(&migrated)
        .load()
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id(), Some("rollout-1"));
    assert_eq!(sessions[0].run_id(), Some("run"));
}

#[tokio::test]
async fn migration_moves_each_measured_range_into_its_motor() {
    let mut options = sea_orm::ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let database = sea_orm::Database::connect(options).await.unwrap();
    migrations::Migrator::up(&database, Some(7)).await.unwrap();
    database
        .execute_unprepared(
            r#"INSERT INTO robots VALUES ('bench', 'arm', '{
                "schema_version": 1, "robot_id": "arm", "robot_type": "so101_follower",
                "extension": {"kept": false},
                "port": {"path": "/dev/arm"},
                "motors": {
                    "shoulder": {"id": 1, "model": "sts3215"},
                    "elbow": {"id": 2, "direction": "reversed"}
                },
                "calibration": {
                    "format_version": 1, "units": "raw", "coordinate_space": "unhomed_raw",
                    "motors": {
                        "shoulder": {"id": 1, "direction": "normal", "range_min": 900, "center": 2100, "range_max": 3300},
                        "elbow": {"id": 2, "direction": "normal", "range_min": 100, "center": 500, "range_max": 800}
                    }
                },
                "cameras": {"wrist": {"id": "0", "type": "opencv", "width": 640, "mount": {"frame": "gripper_link"}}}
            }', 0)"#,
        )
        .await
        .unwrap();
    migrations::Migrator::up(&database, None).await.unwrap();

    let row = robots::Entity::find()
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    let (config, revision) = row.into_config().unwrap();
    assert_eq!(revision, 1);
    assert_eq!(
        config.motors()["shoulder"].calibration(),
        Some(spiral_robotics_common::robots::config::MotorRange::new(900, 2100, 3300).unwrap())
    );
    assert_eq!(
        config.motors()["elbow"].calibration(),
        None,
        "a range measured under another direction was never usable"
    );
    assert_eq!(config.port().map(PortSettings::baud_rate), Some(1_000_000));
    assert_eq!(
        config.cameras()["wrist"]
            .mount()
            .and_then(|mount| mount.frame()),
        Some("gripper_link")
    );
}
