use super::*;
use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use futures::FutureExt as _;
use spiral_robotics_common::robots::document::{RobotDocument, RobotDocumentSave};
use spiral_robotics_storage::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationMetadata,
    RelationName, SpiralConnectionSettings, SqlStatement,
};
use std::sync::Mutex;

#[derive(Default)]
struct DocumentStore {
    saved: Option<String>,
    fail_ingest: bool,
    events: Vec<&'static str>,
}
struct DocumentDatabase(Arc<Mutex<DocumentStore>>);
struct DocumentConnection {
    store: Arc<Mutex<DocumentStore>>,
    pending: Option<Option<String>>,
}
impl Database for DocumentDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            Ok(Box::new(DocumentConnection {
                store: self.0.clone(),
                pending: None,
            }) as Box<dyn DatabaseConnection>)
        }
        .boxed()
    }
}
impl DatabaseSession for DocumentConnection {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        assert!(sql.as_str().starts_with("SELECT config_json FROM"));
        async move {
            self.store.lock().unwrap().events.push("read");
            let saved = self
                .pending
                .as_ref()
                .expect("comparison must be inside the transaction");
            let schema = Arc::new(Schema::new(vec![Field::new(
                "config_json",
                DataType::Utf8,
                false,
            )]));
            let values = saved.iter().map(String::as_str).collect::<Vec<_>>();
            let batch =
                RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(values))])
                    .unwrap();
            Ok(RecordBatchStream::new(
                schema,
                futures::stream::iter([Ok(batch)]).boxed(),
            ))
        }
        .boxed()
    }
    fn query_schema(&mut self, _: SqlStatement) -> DatabaseFuture<'_, Arc<Schema>> {
        panic!("unexpected planning")
    }
    fn table_schema(&mut self, _: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        panic!("unexpected schema read")
    }
    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        panic!("unexpected catalog read")
    }
    fn execute_update(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        assert!(sql.as_str().starts_with("DELETE FROM"));
        async move {
            self.store.lock().unwrap().events.push("delete");
            *self.pending.as_mut().unwrap() = None;
            Ok(ExecutionSummary)
        }
        .boxed()
    }
    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            {
                let mut store = self.store.lock().unwrap();
                store.events.push("ingest");
                if store.fail_ingest {
                    return Err(DatabaseError::new(
                        DatabaseErrorKind::InvalidState,
                        "injected ingestion failure",
                    ));
                }
            }
            let (_, _, mut batches) = ingestion.into_parts();
            let batch = batches.next().await.unwrap()?;
            let saved = batch
                .column_by_name("config_json")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0)
                .to_owned();
            *self.pending.as_mut().unwrap() = Some(saved);
            Ok(ExecutionSummary)
        }
        .boxed()
    }
}
impl DatabaseConnection for DocumentConnection {
    fn begin(mut self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move {
            let saved = {
                let mut store = self.store.lock().unwrap();
                store.events.push("begin");
                store.saved.clone()
            };
            self.pending = Some(saved);
            Ok(self as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}
impl DatabaseTransaction for DocumentConnection {
    fn commit(mut self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        async move {
            let mut store = self.store.lock().unwrap();
            store.saved = self.pending.take().unwrap();
            store.events.push("commit");
            Ok(ExecutionSummary)
        }
        .boxed()
    }
    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async move {
            self.store.lock().unwrap().events.push("rollback");
            Ok(())
        }
        .boxed()
    }
}

struct DocumentFixture {
    store: Arc<Mutex<DocumentStore>>,
    workspace: RobotWorkspace,
}
impl DocumentFixture {
    fn new() -> Self {
        let store = Arc::new(Mutex::new(DocumentStore::default()));
        let settings = SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".into(),
            "https://spfs.spiraldb.dev".into(),
        )
        .unwrap()
        .with_project("documents")
        .unwrap();
        let project = Arc::new(
            ProjectStore::new(
                settings,
                "studio".into(),
                Arc::new(DocumentDatabase(store.clone())),
            )
            .unwrap(),
        );
        Self {
            store,
            workspace: RobotWorkspace::new(project),
        }
    }
    fn document(port: &str) -> RobotDocument {
        serde_json::from_value(serde_json::json!({"schema_version":1, "robot_id":"arm", "motors":{"joint":{"id":1}}, "port":{"path":port,"baud_rate":1000000}, "extension":{"preserve":true}})).unwrap()
    }
}

#[tokio::test]
async fn stale_documents_and_duplicate_creation_roll_back_before_any_write() {
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
    let saved = fixture.store.lock().unwrap().saved.clone();
    for expected in [None, Some(original)] {
        fixture.store.lock().unwrap().events.clear();
        let error = fixture
            .workspace
            .save_document(&RobotDocumentSave::new(
                DocumentFixture::document("/dev/stale"),
                expected,
            ))
            .await
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::Conflict(_)));
        let store = fixture.store.lock().unwrap();
        assert_eq!(store.saved, saved);
        assert_eq!(store.events, ["begin", "read", "rollback"]);
    }
}

#[tokio::test]
async fn failed_ingestion_does_not_delete_the_saved_document() {
    let fixture = DocumentFixture::new();
    let original = DocumentFixture::document("/dev/original");
    fixture
        .workspace
        .save_document(&RobotDocumentSave::new(original.clone(), None))
        .await
        .unwrap();
    let saved = fixture.store.lock().unwrap().saved.clone();
    {
        let mut store = fixture.store.lock().unwrap();
        store.events.clear();
        store.fail_ingest = true;
    }
    assert!(
        fixture
            .workspace
            .save_document(&RobotDocumentSave::new(
                DocumentFixture::document("/dev/new"),
                Some(original)
            ))
            .await
            .is_err()
    );
    let store = fixture.store.lock().unwrap();
    assert_eq!(store.saved, saved);
    assert_eq!(
        store.events,
        ["begin", "read", "delete", "ingest", "rollback"]
    );
}
