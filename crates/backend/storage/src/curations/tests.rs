use super::CurationPersistence;
use crate::StudioStore;
use spiral_robotics_common::curation::{CurationDocument, CurationSummary};

struct Fixture;
impl Fixture {
    fn project(store: &StudioStore, id: &str) -> crate::StudioProject {
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
    fn document(id: &str, name: &str) -> CurationDocument {
        CurationDocument::new(id, name, 1, r#"{"nodes":[]}"#)
    }
}

#[tokio::test]
async fn a_document_inserts_once_and_then_replaces_only_the_revision_it_was_loaded_at() {
    let store = StudioStore::in_memory();
    let project = Fixture::project(&store, "curation");
    let documents = CurationPersistence::new(&project);
    assert_eq!(documents.load("folding").await.unwrap(), None);

    let draft = Fixture::document("folding", "Cloth folding");
    assert!(documents.save(&draft).await.unwrap());
    // A second save at revision 0 is another tab that never loaded the stored copy.
    assert!(!documents.save(&draft).await.unwrap());
    let stored = documents.load("folding").await.unwrap().unwrap();
    assert_eq!(stored, draft.clone().with_revision(1));

    let mut edited = stored.clone();
    edited.set_name("Cloth folding, revised");
    assert!(documents.save(&edited).await.unwrap());
    assert_eq!(
        documents.load("folding").await.unwrap().unwrap(),
        edited.clone().with_revision(2)
    );
    // The same edit replayed carries a revision the row no longer holds.
    assert!(!documents.save(&edited).await.unwrap());
    // Nor may a save skip ahead of the stored revision.
    assert!(
        !documents
            .save(&edited.clone().with_revision(9))
            .await
            .unwrap()
    );
    assert_eq!(
        documents.load("folding").await.unwrap().unwrap().revision(),
        2
    );
}

#[tokio::test]
async fn the_list_is_newest_first_and_scoped_to_one_project() {
    let store = StudioStore::in_memory();
    let project = Fixture::project(&store, "curation");
    let documents = CurationPersistence::new(&project);
    assert!(
        documents
            .save(&Fixture::document("a-first", "First"))
            .await
            .unwrap()
    );
    // The recency index orders by the storage clock before the identity tiebreak.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    assert!(
        documents
            .save(&Fixture::document("b-second", "Second"))
            .await
            .unwrap()
    );
    assert_eq!(
        documents.list().await.unwrap(),
        vec![
            CurationSummary::new("b-second", "Second", 1),
            CurationSummary::new("a-first", "First", 1),
        ]
    );

    let other = Fixture::project(&store, "other");
    assert_eq!(
        CurationPersistence::new(&other).list().await.unwrap(),
        Vec::new()
    );
    assert_eq!(
        CurationPersistence::new(&other)
            .load("a-first")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn documents_that_break_the_wire_contract_are_never_written() {
    let store = StudioStore::in_memory();
    let project = Fixture::project(&store, "curation");
    let documents = CurationPersistence::new(&project);
    for invalid in [
        CurationDocument::new("", "Unnamed identity", 1, "{}"),
        CurationDocument::new("folding", "   ", 1, "{}"),
        CurationDocument::new("folding", "No encoding", 0, "{}"),
        CurationDocument::new("folding", "Empty payload", 1, ""),
    ] {
        assert!(documents.save(&invalid).await.is_err());
    }
    assert_eq!(documents.list().await.unwrap(), Vec::new());
}
