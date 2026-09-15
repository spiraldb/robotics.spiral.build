use super::*;
use crate::rollout_runs::RolloutSessionPersistence;
use crate::settings::StudioPreferencePersistence;
use crate::studio::{RobotDocumentWrite, StudioStore};
use crate::training::TrainingHistoryPersistence;
use crate::workflows::{WorkflowJson, WorkflowPersistence};
use spiral_robotics_common::rollout::{RolloutPhase, RolloutSnapshot};
use spiral_robotics_common::training::{
    TrainingConfig, TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingSnapshot,
};
use spiral_robotics_common::workflow::WorkflowSpec;

struct Fixture;
impl Fixture {
    fn project(store: &StudioStore, id: &str) -> StudioProject {
        store.project(
            &crate::SpiralConnectionSettings::new(
                "http://localhost:1".into(),
                "http://localhost:2".into(),
            )
            .unwrap()
            .with_project(id)
            .unwrap(),
        )
    }
    fn robot(id: &str) -> serde_json::Value {
        serde_json::json!({"schema_version":1, "robot_id":id, "motors":{"joint":{"id":1}}, "port":{"path":"/dev/arm","baud_rate":1000000}})
    }
}

#[tokio::test]
async fn snapshot_restores_all_features_without_credentials_or_other_projects() {
    let directory = tempfile::tempdir().unwrap();
    let store = StudioStore::at(directory.path().join("studio.sqlite3"));
    let source = Fixture::project(&store, "source");
    let target = Fixture::project(&store, "target");
    let other = Fixture::project(&store, "other");
    store
        .save_backend_settings(serde_json::json!({"secret":"excluded-backend-secret-456789"}))
        .await
        .unwrap();
    for (project, id) in [(&source, "arm"), (&other, "excluded-other-robot-456789")] {
        project
            .save_robot_document(RobotDocumentWrite::new(id.into(), Fixture::robot(id), None))
            .await
            .unwrap();
    }
    let workflow = WorkflowSpec::new("flow", "Stored workflow").with_revision(1);
    let graph = WorkflowJson::new(4, serde_json::json!({"workflow":workflow}).to_string());
    let conversation = crate::workflow_assistant::WorkflowConversation::new("flow")
        .persisted()
        .unwrap();
    assert!(
        WorkflowPersistence::new(&source)
            .save(&workflow, (&graph, &conversation))
            .await
            .unwrap()
    );
    let history = TrainingSnapshot::new(
        TrainingReadiness::default(),
        vec![
            TrainingRun::starting("run", TrainingConfig::default())
                .with_metrics(vec![TrainingMetricPoint::new(1, f64::NAN)]),
        ],
    );
    TrainingHistoryPersistence::new(&source)
        .save(&history)
        .await
        .unwrap();
    let rollout = RolloutSnapshot::new(Some("rollout".into()), Some("run".into()))
        .with_lifecycle(RolloutPhase::Failed, Some(10));
    RolloutSessionPersistence::new(&source)
        .save(&rollout)
        .await
        .unwrap();
    StudioPreferencePersistence::new(&source)
        .save(
            &crate::StoredStudioPreferences::from_settings("http://localhost:9000", false).unwrap(),
        )
        .await
        .unwrap();
    let snapshot = StudioSnapshot::capture(&source).await.unwrap();
    let bytes = snapshot.bytes().unwrap();
    for excluded in [
        "excluded-backend-secret-456789",
        "excluded-other-robot-456789",
    ] {
        assert!(
            !bytes
                .windows(excluded.len())
                .any(|window| window == excluded.as_bytes())
        );
    }
    // Later source mutations cannot change an already captured snapshot.
    source.delete_robot_document("arm").await.unwrap();
    let imported = StudioSnapshot::open(&bytes, &target).await.unwrap();
    imported
        .stage(&target)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(
        target.robot_document("arm").await.unwrap(),
        Some(Fixture::robot("arm"))
    );
    assert!(source.robot_document("arm").await.unwrap().is_none());
    assert!(
        other
            .robot_document("excluded-other-robot-456789")
            .await
            .unwrap()
            .is_some()
    );
    let workflow = WorkflowPersistence::new(&target)
        .load("flow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workflow.graph(), &graph);
    assert_eq!(workflow.conversation(), Some(&conversation));
    let restored = TrainingHistoryPersistence::new(&target)
        .load()
        .await
        .unwrap();
    assert_eq!(restored[0].id(), "run");
    assert!(restored[0].metrics()[0].train_loss().is_nan());
    assert_eq!(
        RolloutSessionPersistence::new(&target)
            .load()
            .await
            .unwrap(),
        Some(rollout)
    );
    assert!(
        !StudioPreferencePersistence::new(&target)
            .load()
            .await
            .unwrap()
            .video()
            .enabled()
    );
    assert!(store.backend_settings().await.unwrap().is_some());
}

#[tokio::test]
async fn imported_sqlite_rejects_extra_schema_secrets_and_invalid_documents() {
    let store = StudioStore::in_memory();
    let project = Fixture::project(&store, "source");
    for corruption in [
        "CREATE TRIGGER injected AFTER INSERT ON robots BEGIN SELECT 1; END",
        "INSERT INTO backend_settings VALUES (1, '{}')",
        "INSERT INTO robots VALUES ('', 'arm', '{}', 1)",
        "INSERT INTO robots VALUES ('other', 'arm', '{}', 1)",
    ] {
        let snapshot = StudioSnapshot::capture(&project).await.unwrap();
        snapshot
            .database
            .execute_unprepared(corruption)
            .await
            .unwrap();
        assert!(
            StudioSnapshot::open(&snapshot.bytes().unwrap(), &project)
                .await
                .is_err(),
            "{corruption}"
        );
    }
}
