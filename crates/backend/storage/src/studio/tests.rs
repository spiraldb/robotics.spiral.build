use super::*;
use crate::training::{PublishedModels, TrainingRunPersistence};
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement};
use spiral_robotics_common::training::{
    ModelPublication, PublishedModel, TrainingConfig, TrainingInputSummary, TrainingRunIdentity,
};

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
async fn joint_roles_follow_dataset_version_and_project() {
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

    let other = store.project(&Fixture::connection("other"));
    assert_eq!(other.dataset_joint_roles(identity).await.unwrap(), None);
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
}

#[tokio::test]
async fn local_state_survives_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Studio state #1.sqlite3");
    let store = StudioStore::at(path.clone());
    let connection = Fixture::connection("bench");
    let project = store.project(&connection);
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
    TrainingRunPersistence::new(&source)
        .insert(&TrainingRunIdentity::new(
            "run",
            TrainingConfig::default(),
            TrainingInputSummary::new(1, 1, 2.0),
            1,
        ))
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
    Fixture::sql(&store, "CREATE TRIGGER reject_training_runs BEFORE INSERT ON training_runs BEGIN SELECT RAISE(ABORT, 'restore failed'); END;").await.unwrap();
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

/// Encoding 5 dropped every curation before it, and encoding 6 every one
/// before it, each with the views its curations published; encoding 7 drops
/// the rest and every view, since each read Spiral's retired Datasets.
#[tokio::test]
async fn every_curation_before_encoding_7_is_dropped_with_every_view() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-encoding-6.sqlite3");
    let raw = migrated_until(&path, "m20261008_000001_curations_encoding_5").await;
    raw.execute_unprepared(
        r#"INSERT INTO curations VALUES ('p', 'old', 'Steady drops', 1, 4, '{"published":[{"node":"select_1","view":"steady_drops","revision":1}]}', 1);
           INSERT INTO curations VALUES ('p', 'broken', 'Broken', 1, 3, 'not json', 1);
           INSERT INTO curations VALUES ('p', 'revised', 'Experts', 1, 5, '{"published":[{"node":"select_2","view":"expert_demos","revision":1}]}', 1);
           INSERT INTO curations VALUES ('p', 'new', 'Bottles', 1, 6, '{"published":[{"node":"select_3","view":"bottle_experts","statement":"5e0f"}]}', 1);
           INSERT INTO curated_views VALUES ('p', 'Steady_Drops', 'SELECT 1');
           INSERT INTO curated_views VALUES ('p', 'expert_demos', 'SELECT 1');
           INSERT INTO curated_views VALUES ('p', 'bottle_experts', 'SELECT 1');
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
    assert_eq!(left, [Vec::<String>::new(), Vec::new()]);
}

/// A training run keeps its record and names its dataset by the id its
/// tables live under; what Studio kept about the retired Dataset resources
/// goes.
#[tokio::test]
async fn training_runs_name_their_dataset_by_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-studio-datasets.sqlite3");
    let raw = migrated_until(&path, "m20261012_000001_drop_dataset_check_reports").await;
    raw.execute_unprepared(
        r#"INSERT INTO curations VALUES ('p', 'mcap', 'Bottles', 1, 6, '{"dataset":{"path":["dataset_warp_sim_bottles"]},"nodes":[]}', 1);
           INSERT INTO curations VALUES ('p', 'lerobot', 'Pick', 1, 6, '{"dataset":{"path":["lerobot_pick"],"producer":"spql.lerobot.dataset"},"nodes":[]}', 1);
           INSERT INTO training_runs VALUES ('p', 'train-1', '{"id":"train-1","config":{"source":{"dataset":{"path":["dataset_default"]},"view":"v"}}}');
           INSERT INTO dataset_robots VALUES ('p', 'resource', 'version', '{}');
           INSERT INTO dataset_joint_roles VALUES ('p', 'resource', 'version', '{}');"#,
    )
    .await
    .unwrap();
    let store = StudioStore::at(path);
    store.initialize().await.unwrap();
    let connection = store.connection().await.unwrap();
    let mut read = Vec::new();
    for sql in [
        "SELECT CAST(count(*) AS TEXT) FROM curations",
        "SELECT json_extract(document, '$.config.source.dataset') FROM training_runs",
        "SELECT CAST((SELECT count(*) FROM dataset_robots) + (SELECT count(*) FROM dataset_joint_roles) AS TEXT)",
    ] {
        let rows = connection
            .query_all(Statement::from_string(DbBackend::Sqlite, sql))
            .await
            .unwrap();
        read.extend(
            rows.iter()
                .map(|row| row.try_get_by_index::<String>(0).unwrap()),
        );
    }
    assert_eq!(
        read,
        [
            "0",
            r#"{"id":"default"}"#,
            "0",
        ]
    );
}

/// A run recorded before training models had families trained ACT, and
/// lists after the migration with its settings under `model`.
#[tokio::test]
async fn runs_recorded_before_model_families_become_act_runs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-training-models.sqlite3");
    let raw = migrated_until(&path, "m20261008_000002_training_models").await;
    let source =
        serde_json::json!({"dataset": {"path": ["dataset_fixture"]}, "view": "fixture_view"});
    let observation = serde_json::json!({
        "cameras": ["/top-camera", "/left-wrist-camera"],
        "image": {"width": 64, "height": 48},
        "rate_hz": 10.0
    });
    let architecture = serde_json::json!({
        "hidden_dim": 16, "latent_dim": 4, "encoder_layers": 1, "vae_encoder_layers": 1,
        "decoder_layers": 1, "attention_heads": 4, "feedforward_dim": 32, "kl_weight": 1.0,
        "pretrained_backbone": false
    });
    let recorded = serde_json::json!({
        "id": "train-1",
        "config": {
            "run_name": "fixture-run",
            "policy": "act",
            "source": source,
            "observation": observation,
            "optimization": {
                "steps": 6, "batch_size": 2, "validation_split": 0.5, "learning_rate": 1e-05,
                "checkpoint_every": 3, "chunk_size": 4, "seed": 7
            },
            "act": architecture
        },
        "input": {"episodes": 2, "intervals": 3, "seconds": 5.5, "state": ["left.j1"], "action": ["left.j1"]},
        "started_ms": 1
    });
    raw.execute_unprepared(&format!(
        "INSERT INTO training_runs VALUES ('p', 'train-1', '{recorded}')"
    ))
    .await
    .unwrap();
    let store = StudioStore::at(path);
    store.initialize().await.unwrap();
    let row = store
        .connection()
        .await
        .unwrap()
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT document FROM training_runs WHERE id = 'train-1'",
        ))
        .await
        .unwrap()
        .unwrap();
    let document: serde_json::Value =
        serde_json::from_str(&row.try_get_by_index::<String>(0).unwrap()).unwrap();
    assert_eq!(
        document["config"],
        serde_json::json!({
            "run_name": "fixture-run",
            "source": {"dataset": {"id": "fixture"}, "view": "fixture_view"},
            "model": {
                "kind": "act",
                "observation": observation,
                "chunk_size": 4,
                "architecture": architecture
            },
            "optimization": {
                "steps": 6, "batch_size": 2, "validation_split": 0.5, "learning_rate": 1e-05,
                "checkpoint_every": 3, "seed": 7
            }
        })
    );
    assert_eq!(document["input"], recorded["input"]);
    let run: TrainingRunIdentity = serde_json::from_value(document).unwrap();
    assert_eq!(run.config().model().cameras().len(), 2);
}

/// A database from before published models gains their table, empty, and
/// a name is one name whatever its case, within one project.
#[tokio::test]
async fn published_models_start_empty_and_a_name_is_one_name_in_any_case() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before-published-models.sqlite3");
    migrated_until(&path, "m20261008_000003_published_models").await;
    let store = StudioStore::at(path);
    let project = store.project(&Fixture::connection("models"));
    let models = PublishedModels::new(&project);
    assert!(models.list().await.unwrap().is_empty());
    let published = |name: &str, step| {
        PublishedModel::new(
            ModelPublication::new("train-1", step, name),
            "spiral-robotics-models/0f3c",
            "/top-camera",
        )
    };
    assert!(
        models
            .insert(&published("Bottle experts", 3))
            .await
            .unwrap()
    );
    assert!(
        !models
            .insert(&published("bottle EXPERTS", 6))
            .await
            .unwrap()
    );
    // The same weights may be published under a second name.
    assert!(models.insert(&published("Any bottle", 3)).await.unwrap());
    assert_eq!(
        models.list().await.unwrap(),
        [published("Any bottle", 3), published("Bottle experts", 3)]
    );
    let other = store.project(&Fixture::connection("other"));
    assert!(
        PublishedModels::new(&other)
            .insert(&published("Bottle experts", 6))
            .await
            .unwrap()
    );
    assert_eq!(models.list().await.unwrap().len(), 2);
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
