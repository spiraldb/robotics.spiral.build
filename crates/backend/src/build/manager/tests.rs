use super::*;
use spiral_robotics_storage::{ProjectStore, StudioStore};

struct BuildFixture {
    manager: BuildDocumentManager,
}
impl BuildFixture {
    fn new() -> Self {
        let store = Arc::new(ProjectStore::workflow_test());
        let studio = StudioStore::in_memory().project(&store.connection_settings());
        Self {
            manager: BuildDocumentManager::new(Arc::new(StorageManager::new(store, studio))),
        }
    }
    fn document() -> BuildDocument {
        BuildDocument::new("folding", "Cloth folding", 1, r#"{"nodes":[]}"#)
    }
}

#[tokio::test]
async fn an_unknown_document_is_not_found_and_an_invalid_one_is_never_stored() {
    let fixture = BuildFixture::new();
    assert!(fixture.manager.list().await.unwrap().is_empty());
    assert!(matches!(
        fixture.manager.load("folding").await,
        Err(OperationError::NotFound(_))
    ));
    for invalid in [
        BuildDocument::new("", "Unnamed identity", 1, "{}"),
        BuildDocument::new("folding", "   ", 1, "{}"),
        BuildDocument::new("folding", "Unversioned encoding", 0, "{}"),
        BuildDocument::new(
            "folding",
            "Oversized payload",
            1,
            "x".repeat(spiral_robotics_common::build::MAX_BUILD_PAYLOAD_BYTES + 1),
        ),
    ] {
        assert!(matches!(
            fixture.manager.save(invalid).await,
            Err(OperationError::InvalidInput(_))
        ));
    }
    assert!(fixture.manager.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn each_save_returns_the_next_revision_and_a_stale_one_conflicts() {
    let fixture = BuildFixture::new();
    let saved = fixture
        .manager
        .save(BuildFixture::document())
        .await
        .unwrap();
    assert_eq!(saved.revision(), 1);
    assert_eq!(fixture.manager.load("folding").await.unwrap(), saved);
    assert_eq!(fixture.manager.list().await.unwrap(), vec![saved.summary()]);

    let next = fixture.manager.save(saved.clone()).await.unwrap();
    assert_eq!(next.revision(), 2);
    assert_eq!(next.payload(), saved.payload());

    // The revision the caller loaded is gone, and the stored copy is untouched.
    for stale in [BuildFixture::document(), saved] {
        assert_eq!(
            fixture.manager.save(stale).await,
            Err(OperationError::Conflict(SAVED_ELSEWHERE.into()))
        );
    }
    assert_eq!(fixture.manager.load("folding").await.unwrap(), next);
}
