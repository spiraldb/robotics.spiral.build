use super::*;
use spiral_robotics_common::robots::document::{RobotDocument, RobotDocumentSave};
use spiral_robotics_storage::{SpiralConnectionSettings, StudioStore};

struct DocumentFixture {
    workspace: RobotWorkspace,
}
impl DocumentFixture {
    fn new() -> Self {
        let connection =
            SpiralConnectionSettings::new("http://127.0.0.1:1".into(), "http://127.0.0.1:2".into())
                .unwrap()
                .with_project("documents")
                .unwrap();
        Self {
            workspace: RobotWorkspace::new(StudioStore::in_memory().project(&connection)),
        }
    }
    fn document(port: &str) -> RobotDocument {
        serde_json::from_value(serde_json::json!({"schema_version":1, "robot_id":"arm", "motors":{"joint":{"id":1}}, "port":{"path":port,"baud_rate":1000000}, "extension":{"preserve":true}})).unwrap()
    }
}

#[tokio::test]
async fn stale_documents_and_duplicate_creation_leave_the_saved_document_unchanged() {
    let fixture = DocumentFixture::new();
    let original = DocumentFixture::document("/dev/original");
    fixture
        .workspace
        .save_document(&RobotDocumentSave::new(original.clone(), None))
        .await
        .unwrap();
    let changed = DocumentFixture::document("/dev/changed");
    fixture
        .workspace
        .save_document(&RobotDocumentSave::new(
            changed.clone(),
            Some(original.clone()),
        ))
        .await
        .unwrap();
    let saved = fixture.workspace.get("arm").await.unwrap().config;
    for expected in [None, Some(original)] {
        let error = fixture
            .workspace
            .save_document(&RobotDocumentSave::new(
                DocumentFixture::document("/dev/stale"),
                expected,
            ))
            .await
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::Conflict(_)));
        assert_eq!(fixture.workspace.get("arm").await.unwrap().config, saved);
    }
    assert_eq!(fixture.workspace.try_list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn robot_reads_and_deletion_work_without_spiral() {
    let fixture = DocumentFixture::new();
    fixture
        .workspace
        .save_document(&RobotDocumentSave::new(
            DocumentFixture::document("/dev/arm"),
            None,
        ))
        .await
        .unwrap();
    let record = fixture.workspace.get("arm").await.unwrap();
    assert_eq!(record.config.root()["extension"]["preserve"], true);
    fixture.workspace.delete("arm").await.unwrap();
    assert!(matches!(
        fixture.workspace.get("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
    assert!(matches!(
        fixture.workspace.delete("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
}
