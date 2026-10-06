use super::*;
use crate::curations::CurationPersistence;
use crate::settings::StudioPreferencePersistence;
use crate::studio::StudioStore;
use crate::training::TrainingRunPersistence;
use spiral_robotics_common::training::{
    TrainingConfig, TrainingInputSummary, TrainingRunIdentity,
};

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
    fn curation(id: &str) -> spiral_robotics_common::curation::CurationDocument {
        spiral_robotics_common::curation::CurationDocument::new(
            id,
            "Cloth folding",
            1,
            r#"{"nodes":[]}"#,
        )
    }
}

#[tokio::test]
async fn snapshot_restores_all_features_without_credentials_or_other_projects() {
    let directory = tempfile::tempdir().unwrap();
    let store = StudioStore::at(directory.path().join("studio.sqlite3"));
    let source = Fixture::project(&store, "source");
    let target = Fixture::project(&store, "target");
    let other = Fixture::project(&store, "other");
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
    let curation = Fixture::curation("folding");
    for (project, curation) in [
        (&source, curation.clone()),
        (&other, Fixture::curation("excluded-other-curation-456789")),
    ] {
        assert!(
            CurationPersistence::new(project)
                .save(&curation)
                .await
                .unwrap()
        );
    }
    let run = TrainingRunIdentity::new(
        "run",
        TrainingConfig::default(),
        TrainingInputSummary::new(1, 1, 2.0),
        10,
    );
    TrainingRunPersistence::new(&source)
        .insert(&run)
        .await
        .unwrap();
    StudioPreferencePersistence::new(&source)
        .save(
            &crate::StoredStudioPreferences::default()
                .with_robot_connector("http://localhost:9000")
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot = StudioSnapshot::capture(&source).await.unwrap();
    let bytes = snapshot.bytes().unwrap();
    for excluded in [
        "excluded-refresh-token-456789",
        "excluded-other-curation-456789",
    ] {
        assert!(
            !bytes
                .windows(excluded.len())
                .any(|window| window == excluded.as_bytes())
        );
    }
    // Later source mutations cannot change an already captured snapshot.
    TrainingRunPersistence::new(&source)
        .delete("run")
        .await
        .unwrap();
    let imported = StudioSnapshot::open(&bytes, &target).await.unwrap();
    imported
        .stage(&target)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        TrainingRunPersistence::new(&source)
            .load()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        CurationPersistence::new(&other)
            .load("excluded-other-curation-456789")
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
    assert_eq!(
        StudioPreferencePersistence::new(&target)
            .load()
            .await
            .unwrap()
            .robot_connector()
            .address(),
        "http://localhost:9000"
    );
}

#[tokio::test]
async fn imported_sqlite_rejects_extra_schema_secrets_and_invalid_documents() {
    let store = StudioStore::in_memory();
    let project = Fixture::project(&store, "source");
    for corruption in [
        "CREATE TRIGGER injected AFTER INSERT ON curations BEGIN SELECT 1; END",
        "INSERT INTO sessions VALUES ('browser', 'access', 'refresh', '')",
        "INSERT INTO curations VALUES ('other', 'folding', 'Folding', 1, 1, '{}', 0)",
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
