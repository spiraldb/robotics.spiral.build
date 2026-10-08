use super::*;
use crate::settings::{StoredStudioPreferences, StudioPreferencePersistence};
use crate::training::TrainingRunPersistence;
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement};
use spiral_robotics_common::training::{TrainingConfig, TrainingInputSummary, TrainingRunIdentity};

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
    StudioPreferencePersistence::new(&project)
        .save(
            &StoredStudioPreferences::default()
                .with_robot_connector("http://localhost:9")
                .unwrap(),
        )
        .await
        .unwrap();
    for (run, started_ms) in [("run", 2), ("second_run", 1)] {
        let identity = TrainingRunIdentity::new(
            run,
            TrainingConfig::default(),
            TrainingInputSummary::new(1, 1, 2.0),
            started_ms,
        );
        TrainingRunPersistence::new(&project)
            .insert(&identity)
            .await
            .unwrap();
    }
    // The file holds every browser's Spiral sign-in.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let reopened = StudioStore::at(path);
    let recovered = reopened.project(&connection);
    assert_eq!(
        StudioPreferencePersistence::new(&recovered)
            .load()
            .await
            .unwrap()
            .robot_connector()
            .address(),
        "http://localhost:9"
    );
    assert_eq!(
        TrainingRunPersistence::new(&recovered)
            .load()
            .await
            .unwrap()
            .iter()
            .map(TrainingRunIdentity::id)
            .collect::<Vec<_>>(),
        ["run", "second_run"]
    );
    let other = reopened.project(&Fixture::connection("other"));
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
    let document = spiral_robotics_common::curation::CurationDocument::new(
        "folding",
        "Cloth folding",
        1,
        r#"{"nodes":[]}"#,
    );
    assert!(
        crate::curations::CurationPersistence::new(&source)
            .save(&document)
            .await
            .unwrap()
    );
    let saved = |project| async move {
        crate::curations::CurationPersistence::new(project)
            .load("folding")
            .await
            .unwrap()
    };
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
    let document = document.with_revision(1);
    assert_eq!(saved(&target).await, Some(document.clone()));
    // A later feature failure must undo earlier curation deletion/insertion too.
    Fixture::sql(&store, "CREATE TRIGGER reject_preferences BEFORE INSERT ON studio_settings BEGIN SELECT RAISE(ABORT, 'restore failed'); END;").await.unwrap();
    let failed = imported.stage(&target).await;
    assert!(failed.is_err());
    assert_eq!(saved(&target).await, Some(document.clone()));
    assert_eq!(saved(&source).await, Some(document));
    assert_eq!(
        curations::Entity::find()
            .filter(curations::Column::Project.eq(target.key()))
            .all(store.connection().await.unwrap())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn projects_with_records_become_workspaces_named_by_their_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-workspaces.sqlite3");
    let raw = migrated_until(&path, "m20261005_000004_workspaces").await;
    raw.execute_unprepared(
        r#"INSERT INTO curations VALUES ('["https://api.spiraldb.dev","arms-618551"]', 'c', 'Grasp', 1, 1, '{}', 1);
           INSERT INTO studio_settings VALUES ('["https://api.spiraldb.dev","arms-618551"]', '{}');
           INSERT INTO studio_settings VALUES ('["https://api.spiraldb.dev",""]', '{}');"#,
    )
    .await
    .unwrap();
    let store = StudioStore::at(path);
    store.initialize().await.unwrap();
    assert_eq!(
        crate::WorkspaceStore::new(store, "https://api.spiraldb.dev")
            .all()
            .await
            .unwrap(),
        [spiral_robotics_common::workspace::Workspace::new(
            "arms-618551",
            "arms-618551"
        )]
    );
}

#[tokio::test]
async fn curations_before_encoding_5_are_dropped_with_the_views_they_published() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-encoding-5.sqlite3");
    let raw = migrated_until(&path, "m20261008_000001_curations_encoding_5").await;
    raw.execute_unprepared(
        r#"INSERT INTO curations VALUES ('p', 'old', 'Steady drops', 1, 4, '{"published":[{"node":"select_1","view":"steady_drops","revision":1}]}', 1);
           INSERT INTO curations VALUES ('p', 'broken', 'Broken', 1, 3, 'not json', 1);
           INSERT INTO curations VALUES ('p', 'new', 'Experts', 1, 5, '{"published":[{"node":"select_2","view":"expert_demos","revision":1}]}', 1);
           INSERT INTO curated_views VALUES ('p', 'Steady_Drops', 'SELECT 1');
           INSERT INTO curated_views VALUES ('p', 'expert_demos', 'SELECT 1');
           INSERT INTO curated_views VALUES ('q', 'steady_drops', 'SELECT 1');"#,
    )
    .await
    .unwrap();
    let store = StudioStore::at(path);
    store.initialize().await.unwrap();
    let connection = store.connection().await.unwrap();
    let mut left = Vec::new();
    for sql in [
        "SELECT id FROM curations ORDER BY id",
        "SELECT project || ':' || name FROM curated_views ORDER BY project, name",
    ] {
        let rows = connection
            .query_all(Statement::from_string(DbBackend::Sqlite, sql))
            .await
            .unwrap();
        left.push(
            rows.iter()
                .map(|row| row.try_get_by_index::<String>(0).unwrap())
                .collect::<Vec<_>>(),
        );
    }
    // A view of the same name in another project is not the dropped one.
    assert_eq!(
        left,
        [vec!["new"], vec!["p:expert_demos", "q:steady_drops"]]
    );
}

/// A database at `path` with every migration before `name` applied.
async fn migrated_until(path: &std::path::Path, name: &str) -> sea_orm::DatabaseConnection {
    use sea_orm_migration::MigratorTrait as _;
    let raw = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    let before = super::migrations::Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == name)
        .unwrap();
    super::migrations::Migrator::up(&raw, Some(before as u32))
        .await
        .unwrap();
    raw
}
