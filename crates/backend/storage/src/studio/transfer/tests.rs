use super::*;
use crate::curations::CurationPersistence;
use crate::settings::StudioPreferencePersistence;
use crate::studio::StudioStore;
use crate::training::{TrainingRunPersistence, TrainingRunRecord};
use spiral_robotics_common::robots::PortSettings;
use spiral_robotics_common::robots::config::RobotConfig;
use spiral_robotics_common::training::{TrainingConfig, TrainingRunIdentity};

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
    fn robot(id: &str) -> RobotConfig {
        RobotConfig::so101(id, "so101_follower")
            .and_then(|config| config.with_port(PortSettings::new("/dev/arm", 1_000_000)))
            .unwrap()
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
        .patch_backend_settings(serde_json::json!({"secret":"excluded-backend-secret-456789"}))
        .await
        .unwrap();
    crate::SessionStore::new(store.clone())
        .save(
            "browser",
            &crate::StoredSession::new(
                "access".into(),
                "excluded-refresh-token-456789".into(),
                "source".into(),
            ),
        )
        .await
        .unwrap();
    for (project, id) in [(&source, "arm"), (&other, "excluded-other-robot-456789")] {
        project
            .save_robot_config(&Fixture::robot(id), None)
            .await
            .unwrap();
    }
    let curation = spiral_robotics_common::curation::CurationDocument::new(
        "folding",
        "Cloth folding",
        1,
        r#"{"nodes":[]}"#,
    );
    assert!(
        CurationPersistence::new(&source)
            .save(&curation)
            .await
            .unwrap()
    );
    let run = TrainingRunRecord::new(
        TrainingRunIdentity::new("run", TrainingConfig::default(), 10),
        "http://localhost:9000",
    );
    TrainingRunPersistence::new(&source)
        .insert(&run)
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
        "excluded-refresh-token-456789",
        "excluded-other-robot-456789",
    ] {
        assert!(
            !bytes
                .windows(excluded.len())
                .any(|window| window == excluded.as_bytes())
        );
    }
    // Later source mutations cannot change an already captured snapshot.
    source.delete_robot_config("arm").await.unwrap();
    let imported = StudioSnapshot::open(&bytes, &target).await.unwrap();
    imported
        .stage(&target)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(
        target.robot_config("arm").await.unwrap(),
        Some((Fixture::robot("arm"), 1))
    );
    assert!(source.robot_config("arm").await.unwrap().is_none());
    assert!(
        other
            .robot_config("excluded-other-robot-456789")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        CurationPersistence::new(&target)
            .load("folding")
            .await
            .unwrap(),
        Some(curation.with_revision(1))
    );
    assert_eq!(
        TrainingRunPersistence::new(&target).load().await.unwrap(),
        [run]
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
        "INSERT INTO sessions VALUES ('browser', 'access', 'refresh', '')",
        "INSERT INTO robots VALUES ('', 'arm', '{}', 1)",
        "INSERT INTO robots VALUES ('other', 'arm', '{}', 1)",
        "INSERT INTO curations VALUES ('', 'folding', '   ', 1, 1, '{}', 0)",
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
