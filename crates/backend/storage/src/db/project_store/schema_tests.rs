use super::*;
use arrow_schema::{DataType, Field};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Semaphore, mpsc};

struct SchemaDatabase {
    started: mpsc::UnboundedSender<String>,
    release: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
}

impl Database for SchemaDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        let connection = SchemaConnection {
            started: self.started.clone(),
            release: self.release.clone(),
            active: self.active.clone(),
        };
        async move { Ok(Box::new(connection) as Box<dyn DatabaseConnection>) }.boxed()
    }
}

struct SchemaConnection {
    started: mpsc::UnboundedSender<String>,
    release: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
}

struct ActiveSchema(Arc<AtomicUsize>);
impl Drop for ActiveSchema {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl DatabaseSession for SchemaConnection {
    fn table_schema(&mut self, name: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveSchema(self.active.clone());
            self.started.send(name.as_str().to_owned()).unwrap();
            self.release.acquire().await.unwrap().forget();
            Ok(Some(Arc::new(Schema::new(vec![Field::new(
                name.as_str(),
                DataType::Int64,
                false,
            )]))))
        }
        .boxed()
    }
    fn query(&mut self, _: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        unimplemented!()
    }
    fn execute_update(&mut self, _: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        unimplemented!()
    }
    fn ingest(&mut self, _: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        unimplemented!()
    }
    fn relations(&mut self) -> DatabaseFuture<'_, Vec<super::super::database::RelationMetadata>> {
        unimplemented!()
    }
}
impl DatabaseConnection for SchemaConnection {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        unimplemented!()
    }
}

#[tokio::test]
async fn schema_reads_are_concurrent_bounded_ordered_and_keep_project_admission() {
    let (started, mut arrivals) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let database = Arc::new(SchemaDatabase {
        started,
        release: release.clone(),
        active: active.clone(),
    });
    let gate = Arc::new(tokio::sync::RwLock::new(()));
    let mut connection = ProjectConnection::open(
        database,
        ProjectOperationLease::Shared {
            _lease: gate.clone().read_owned().await,
        },
    )
    .await
    .unwrap();
    let work = tokio::spawn(async move {
        let names = (0..12)
            .map(|index| RelationName::parse(format!("table_{index}")).unwrap())
            .collect();
        connection.relation_schemas(names).await.unwrap()
    });
    for _ in 0..8 {
        tokio::time::timeout(std::time::Duration::from_secs(2), arrivals.recv())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(active.load(Ordering::SeqCst), 8);
    assert!(arrivals.try_recv().is_err());
    assert!(
        gate.try_write().is_err(),
        "schema reads retain project admission"
    );
    release.add_permits(12);
    let schemas = work.await.unwrap();
    assert_eq!(schemas.len(), 12);
    for (index, (name, schema)) in schemas.into_iter().enumerate() {
        assert_eq!(name.as_str(), format!("table_{index}"));
        assert_eq!(schema.unwrap().unwrap().field(0).name(), name.as_str());
    }
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(gate.try_write().is_ok());
}
