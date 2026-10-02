use super::*;
use crate::settings::{StoredStudioPreferences, StudioPreferencePersistence};
use crate::training::{TrainingRunPersistence, TrainingRunRecord};
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, QueryFilter, Statement};
use spiral_robotics_common::robots::PortSettings;
use spiral_robotics_common::robots::config::RobotConfig;
use spiral_robotics_common::training::{TrainingConfig, TrainingRunIdentity};

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
async fn joint_roles_and_check_reports_follow_dataset_version_definition_and_project() {
    let store = StudioStore::in_memory();
    let project = store.project(&Fixture::connection("checks"));
    let identity = ("dataset-id", "revision-1");
    assert_eq!(project.dataset_joint_roles(identity).await.unwrap(), None);
    let roles: spiral_robotics_common::datasets::DatasetJointRoles =
        serde_json::from_value(serde_json::json!({"arms": [{
            "name": "left",
            "state": {"topic": "/frame", "schema": "Frame", "field": ["state"], "element": [], "indices": [0, 1]},
            "action": null, "gripper": null
        }]}))
        .unwrap();
    project
        .save_dataset_joint_roles(identity, &roles)
        .await
        .unwrap();
    assert_eq!(
        project.dataset_joint_roles(identity).await.unwrap(),
        Some(roles.clone())
    );
    assert_eq!(
        project
            .dataset_joint_roles((identity.0, "revision-2"))
            .await
            .unwrap(),
        None
    );

    let report = serde_json::json!({"streams": []});
    assert_eq!(
        project.dataset_check_report(identity, 1).await.unwrap(),
        None
    );
    project
        .save_dataset_check_report(identity, 1, report.clone())
        .await
        .unwrap();
    assert_eq!(
        project.dataset_check_report(identity, 1).await.unwrap(),
        Some(report.clone())
    );
    // Changed checks, or a new dataset version, read as no report.
    assert_eq!(
        project.dataset_check_report(identity, 2).await.unwrap(),
        None
    );
    assert_eq!(
        project
            .dataset_check_report((identity.0, "revision-2"), 1)
            .await
            .unwrap(),
        None
    );

    let other = store.project(&Fixture::connection("other"));
    assert_eq!(other.dataset_check_report(identity, 1).await.unwrap(), None);
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
    assert_eq!(
        other.dataset_joint_roles(identity).await.unwrap(),
        Some(roles)
    );
    assert_eq!(
        other.dataset_check_report(identity, 1).await.unwrap(),
        Some(report)
    );
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
    for (run, started_ms) in [("run", 2), ("second_run", 1)] {
        let identity = TrainingRunIdentity::new(run, TrainingConfig::default(), started_ms);
        TrainingRunPersistence::new(&project)
            .insert(&TrainingRunRecord::new(identity, "http://localhost:9"))
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
        TrainingRunPersistence::new(&recovered)
            .load()
            .await
            .unwrap()
            .iter()
            .map(|record| record.identity().id())
            .collect::<Vec<_>>(),
        ["run", "second_run"]
    );
    let other = reopened.project(&Fixture::connection("other"));
    assert!(other.robot_configs().await.unwrap().is_empty());
    assert!(
        TrainingRunPersistence::new(&other)
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
    // Initializing twice journals each migration once.
    assert_eq!(
        before.try_get::<i64>("", "count").unwrap(),
        <super::migrations::Migrator as sea_orm_migration::MigratorTrait>::migrations().len()
            as i64
    );
    // A conflict halfway through the baseline must undo both earlier CREATE
    // TABLE statements and the migration journal itself.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("failed-upgrade.sqlite3");
    let raw = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    raw.execute_unprepared("CREATE TABLE curations (id INTEGER PRIMARY KEY)")
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
    raw.execute_unprepared("DROP TABLE curations")
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
