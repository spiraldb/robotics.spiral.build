use super::*;
use spiral_robotics_common::curation::PublishedView;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_storage::{ProjectStore, StudioStore};

struct CurationFixture {
    manager: CurationManager,
}
impl CurationFixture {
    fn new() -> Self {
        let store = Arc::new(ProjectStore::in_memory());
        let studio = StudioStore::in_memory().project(&store.connection_settings());
        Self {
            manager: CurationManager::new(
                Arc::new(StorageManager::new(store, studio)),
                Arc::new(ProviderKeys::default()),
            ),
        }
    }
    fn document() -> CurationDocument {
        CurationDocument::new("folding", "Cloth folding", 1, r#"{"nodes":[]}"#)
    }
}

#[tokio::test]
async fn an_unknown_document_is_not_found_and_an_invalid_one_is_never_stored() {
    let fixture = CurationFixture::new();
    assert!(fixture.manager.list().await.unwrap().is_empty());
    assert!(matches!(
        fixture.manager.load("folding").await,
        Err(OperationError::NotFound(_))
    ));
    for invalid in [
        CurationDocument::new("", "Unnamed identity", 1, "{}"),
        CurationDocument::new("folding", "   ", 1, "{}"),
        CurationDocument::new("folding", "Unversioned encoding", 0, "{}"),
        CurationDocument::new(
            "folding",
            "Oversized payload",
            1,
            "x".repeat(spiral_robotics_common::curation::MAX_CURATION_PAYLOAD_BYTES + 1),
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
    let fixture = CurationFixture::new();
    let saved = fixture
        .manager
        .save(CurationFixture::document())
        .await
        .unwrap();
    assert_eq!(saved.revision(), 1);
    assert_eq!(fixture.manager.load("folding").await.unwrap(), saved);
    assert_eq!(fixture.manager.list().await.unwrap(), vec![saved.summary()]);

    let next = fixture.manager.save(saved.clone()).await.unwrap();
    assert_eq!(next.revision(), 2);
    assert_eq!(next.payload(), saved.payload());

    // The revision the caller loaded is gone, and the stored copy is untouched.
    for stale in [CurationFixture::document(), saved] {
        assert_eq!(
            fixture.manager.save(stale).await,
            Err(OperationError::Conflict(SAVED_ELSEWHERE.into()))
        );
    }
    assert_eq!(fixture.manager.load("folding").await.unwrap(), next);
}

#[tokio::test]
async fn a_delete_removes_only_the_revision_it_names() {
    let fixture = CurationFixture::new();
    let saved = fixture
        .manager
        .save(CurationFixture::document())
        .await
        .unwrap();
    let next = fixture.manager.save(saved.clone()).await.unwrap();
    // The revision the caller loaded is gone: the stored copy stays.
    assert_eq!(
        fixture
            .manager
            .delete(CurationDeletion::new("folding", saved.revision()))
            .await,
        Err(OperationError::Conflict(DELETE_SAVED_ELSEWHERE.into()))
    );
    assert_eq!(fixture.manager.load("folding").await.unwrap(), next);
    fixture
        .manager
        .delete(CurationDeletion::new("folding", next.revision()))
        .await
        .unwrap();
    assert!(fixture.manager.list().await.unwrap().is_empty());
    assert!(matches!(
        fixture
            .manager
            .delete(CurationDeletion::new("folding", next.revision()))
            .await,
        Err(OperationError::NotFound(_))
    ));
}

/// A run over the folding dataset that only publishes `view` as `statement`.
fn publishing(view: &str, statement: &str) -> FillRequest {
    FillRequest::new(
        DatasetSelection::new(vec!["dataset_folding".into()]).unwrap(),
        Vec::new(),
    )
    .publishing(vec![PublishedView::new(view, statement)])
}

#[tokio::test]
async fn a_publish_is_refused_when_its_name_is_not_an_identifier() {
    let fixture = CurationFixture::new();
    for name in ["", "2folds", "folds; DROP TABLE robots", "my folds"] {
        assert!(matches!(
            fixture.manager.fill(publishing(name, "SELECT 1")),
            Err(OperationError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture.manager.unpublish(UnpublishRequest::new(name)).await,
            Err(OperationError::InvalidInput(_))
        ));
    }
}

#[tokio::test]
async fn a_publish_admits_only_one_read_only_query() {
    let fixture = CurationFixture::new();
    for statement in [
        "DROP TABLE robots",
        "SELECT 1; SELECT 2",
        "EXPLAIN SELECT 1",
        "WITH gone AS (DELETE FROM robots RETURNING *) SELECT * FROM gone",
    ] {
        assert!(
            matches!(
                fixture.manager.fill(publishing("folds", statement)),
                Err(OperationError::InvalidInput(_))
            ),
            "{statement}"
        );
    }
}
