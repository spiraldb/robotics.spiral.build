//! The project-scoped persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all use
//! the same async database boundary. Nothing below this module falls back to
//! a filesystem or a second database.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use arrow_array::RecordBatchReader;
use arrow_schema::{ArrowError, Schema};
use futures::StreamExt as _;

use crate::database::{
    BatchIngestion, Database, DatabaseCommand, DatabaseConnection, DatabaseOutput,
    DatabaseTransaction, IngestMode, RecordBatchStream, RelationName,
};
use crate::settings::{SpiralConnectionSettings, SpiralCredentials};
use crate::spiral_database::SpiralDatabase;
use tokio::sync::OwnedRwLockReadGuard;

const SPIRAL_UNAVAILABLE_PREFIX: &str = "SpiralDB is unavailable at ";
const TRANSPORT_FAILURE_MARKERS: [&str; 5] = [
    "error sending request for url",
    "connection refused",
    "failed to connect",
    "tcp connect error",
    "dns error",
];

/// The only connection type visible to Studio code.
///
/// It deliberately exposes storage operations rather than the native Spiral
/// connection. The boxed trait object is a `Send` proxy to the local executor.
pub struct StorageConnection {
    database: Arc<dyn Database>,
    state: StorageConnectionState,
}

enum StorageConnectionState {
    Autocommit(Box<dyn DatabaseConnection>),
    Transaction(Box<dyn DatabaseTransaction>),
    Transitioning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogRelation {
    name: String,
}

impl CatalogRelation {
    fn new(name: String) -> Self {
        Self { name }
    }

    pub(crate) fn into_name(self) -> String {
        self.name
    }
}

impl StorageConnection {
    fn open(database: Arc<dyn Database>) -> Result<Self, String> {
        let connection =
            futures::executor::block_on(database.connect()).map_err(|error| error.to_string())?;
        Ok(Self {
            database,
            state: StorageConnectionState::Autocommit(connection),
        })
    }

    /// Close the current logical connection instead of attempting reuse.
    pub fn discard(&mut self) {
        self.state = StorageConnectionState::Transitioning;
    }

    pub(crate) fn new_statement(&mut self) -> Result<StorageStatement<'_>, String> {
        Ok(StorageStatement::new(self))
    }

    fn execute_command(&mut self, command: DatabaseCommand) -> Result<DatabaseOutput, String> {
        let result = match &mut self.state {
            StorageConnectionState::Autocommit(connection) => {
                futures::executor::block_on(connection.execute(command))
            }
            StorageConnectionState::Transaction(transaction) => {
                futures::executor::block_on(transaction.execute(command))
            }
            StorageConnectionState::Transitioning => {
                return Err("database connection is transitioning or closed".to_owned());
            }
        };
        result.map_err(|error| error.to_string())
    }

    /// Execute one read query without exposing the mutable database statement
    /// lifecycle outside the storage crate.
    pub fn execute_query(
        &mut self,
        sql: impl AsRef<str>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'static>, String> {
        let output = self.execute_command(DatabaseCommand::statement(sql.as_ref()))?;
        let batches = output.into_records().map_err(|error| error.to_string())?;
        Ok(Box::new(BlockingRecordBatchReader::new(batches)))
    }

    pub fn table_schema(&mut self, table: &str) -> Result<Option<Schema>, String> {
        let relation = RelationName::parse(table).map_err(|error| error.to_string())?;
        let result = match &mut self.state {
            StorageConnectionState::Autocommit(connection) => {
                futures::executor::block_on(connection.table_schema(relation))
            }
            StorageConnectionState::Transaction(transaction) => {
                futures::executor::block_on(transaction.table_schema(relation))
            }
            StorageConnectionState::Transitioning => {
                return Err("database connection is transitioning or closed".to_owned());
            }
        };
        result
            .map(|schema| schema.map(|schema| schema.as_ref().clone()))
            .map_err(|error| error.to_string())
    }

    /// Enumerate the durable relations advertised by the active Spiral
    /// project. Temporary query-session relations are intentionally absent
    /// from the native project's catalog.
    pub(crate) fn list_catalog_relations(
        &mut self,
        _project: &str,
    ) -> Result<Vec<CatalogRelation>, String> {
        let result = match &mut self.state {
            StorageConnectionState::Autocommit(connection) => {
                futures::executor::block_on(connection.relations())
            }
            StorageConnectionState::Transaction(transaction) => {
                futures::executor::block_on(transaction.relations())
            }
            StorageConnectionState::Transitioning => {
                return Err("database connection is transitioning or closed".to_owned());
            }
        };
        let mut relations = result
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|metadata| {
                let (name, _kind) = metadata.into_parts();
                CatalogRelation::new(name.into_string())
            })
            .collect::<Vec<_>>();
        relations.sort_by(|left, right| left.name.cmp(&right.name));
        relations.dedup_by(|left, right| left.name == right.name);
        Ok(relations)
    }

    /// List durable relation names without exposing the driver's catalog
    /// representation across the storage boundary.
    pub fn catalog_relation_names(&mut self, project: &str) -> Result<Vec<String>, String> {
        self.list_catalog_relations(project).map(|relations| {
            relations
                .into_iter()
                .map(|relation| relation.name)
                .collect()
        })
    }

    pub fn set_autocommit(&mut self, enabled: bool) -> Result<(), String> {
        if enabled {
            return Ok(());
        }
        let state = std::mem::replace(&mut self.state, StorageConnectionState::Transitioning);
        let StorageConnectionState::Autocommit(connection) = state else {
            self.state = state;
            return Err("database connection is already in a transaction".to_owned());
        };
        self.state = StorageConnectionState::Transaction(
            futures::executor::block_on(connection.begin()).map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), String> {
        self.complete_transaction(true)
    }

    pub fn rollback(&mut self) -> Result<(), String> {
        self.complete_transaction(false)
    }

    fn complete_transaction(&mut self, commit: bool) -> Result<(), String> {
        let state = std::mem::replace(&mut self.state, StorageConnectionState::Transitioning);
        let StorageConnectionState::Transaction(transaction) = state else {
            self.state = state;
            return Err("database connection has no active transaction".to_owned());
        };
        if commit {
            futures::executor::block_on(transaction.commit()).map_err(|error| error.to_string())?;
        } else {
            futures::executor::block_on(transaction.rollback())
                .map_err(|error| error.to_string())?;
        }
        let connection = futures::executor::block_on(self.database.connect())
            .map_err(|error| error.to_string())?;
        self.state = StorageConnectionState::Autocommit(connection);
        Ok(())
    }
}

struct BlockingRecordBatchReader {
    schema: Arc<Schema>,
    batches: RecordBatchStream,
}

impl BlockingRecordBatchReader {
    fn new(batches: RecordBatchStream) -> Self {
        Self {
            schema: Arc::clone(batches.schema()),
            batches,
        }
    }
}

impl Iterator for BlockingRecordBatchReader {
    type Item = Result<arrow_array::RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        futures::executor::block_on(self.batches.next())
            .map(|result| result.map_err(|error| ArrowError::ExternalError(Box::new(error))))
    }
}

impl RecordBatchReader for BlockingRecordBatchReader {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }
}

/// Synchronous compatibility wrapper used only by storage work that is
/// inherently hosted on a synchronous recording, import, or model worker.
pub(crate) struct StorageStatement<'a> {
    connection: &'a mut StorageConnection,
    sql: Option<String>,
    ingestion: Option<(RelationName, IngestMode)>,
    batches: Option<RecordBatchStream>,
}

impl<'a> StorageStatement<'a> {
    fn new(connection: &'a mut StorageConnection) -> Self {
        Self {
            connection,
            sql: None,
            ingestion: None,
            batches: None,
        }
    }

    pub(crate) fn set_sql_query(&mut self, sql: impl AsRef<str>) -> Result<(), String> {
        self.sql = Some(sql.as_ref().to_owned());
        Ok(())
    }

    pub(crate) fn configure_ingest(&mut self, target: &str) -> Result<(), String> {
        self.configure_ingest_mode(target, IngestMode::CreateOrAppend)
    }

    pub(crate) fn configure_replace(&mut self, target: &str) -> Result<(), String> {
        self.configure_ingest_mode(target, IngestMode::Replace)
    }

    fn configure_ingest_mode(&mut self, target: &str, mode: IngestMode) -> Result<(), String> {
        self.ingestion = Some((
            RelationName::parse(target).map_err(|error| error.to_string())?,
            mode,
        ));
        Ok(())
    }

    pub(crate) fn bind_stream(
        &mut self,
        reader: Box<dyn RecordBatchReader + Send>,
    ) -> Result<(), String> {
        let schema = reader.schema();
        let batches = futures::stream::iter(reader.map(|result| {
            result.map_err(|error| crate::DatabaseError::internal(error.to_string()))
        }))
        .boxed();
        self.batches = Some(RecordBatchStream::new(schema, batches));
        Ok(())
    }

    pub(crate) fn execute(
        &mut self,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'static>, String> {
        let sql = self
            .sql
            .take()
            .ok_or_else(|| "database statement has no SQL text".to_owned())?;
        self.connection.execute_query(sql)
    }

    pub(crate) fn execute_update(&mut self) -> Result<Option<i64>, String> {
        let output = if let Some((relation, mode)) = self.ingestion.take() {
            let batches = self
                .batches
                .take()
                .ok_or_else(|| "batch ingestion has no bound Arrow stream".to_owned())?;
            self.connection
                .execute_command(DatabaseCommand::ingest(BatchIngestion::new(
                    relation, mode, batches,
                )))?
        } else {
            let sql = self
                .sql
                .take()
                .ok_or_else(|| "database statement has no SQL text".to_owned())?;
            self.connection
                .execute_command(DatabaseCommand::statement(sql))?
        };
        let summary = output.into_summary().map_err(|error| error.to_string())?;
        summary
            .affected_rows()
            .map(|rows| {
                i64::try_from(rows).map_err(|_| "affected row count exceeds i64".to_owned())
            })
            .transpose()
    }
}

pub struct ProjectStore {
    connection_settings: Arc<RwLock<SpiralConnectionSettings>>,
    namespace: String,
    database: ProjectDatabase,
    operation_gate: Arc<RwLock<()>>,
    async_operation_gate: Arc<tokio::sync::RwLock<()>>,
}

#[derive(Clone)]
struct ProjectDatabase {
    native: SpiralDatabase,
    interface: Arc<dyn Database>,
}

impl ProjectDatabase {
    fn start(settings: SpiralConnectionSettings) -> Result<Self, String> {
        let native = SpiralDatabase::start(settings).map_err(|error| error.to_string())?;
        let interface = Arc::new(native.clone()) as Arc<dyn Database>;
        Ok(Self { native, interface })
    }

    async fn configure(&self, settings: SpiralConnectionSettings) -> Result<(), String> {
        self.native
            .configure(settings)
            .await
            .map_err(|error| error.to_string())
    }

    fn interface(&self) -> Arc<dyn Database> {
        Arc::clone(&self.interface)
    }
}

pub(crate) struct ProjectConnection {
    connection: Box<dyn DatabaseConnection>,
    _operation: OwnedRwLockReadGuard<()>,
}

impl Deref for ProjectConnection {
    type Target = dyn DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref()
    }
}

impl DerefMut for ProjectConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut()
    }
}

impl ProjectStore {
    /// Identify the stable operator-facing form so delivery adapters can use
    /// an availability status instead of misclassifying the failure as input.
    pub fn is_unavailable_error(message: &str) -> bool {
        message.starts_with(SPIRAL_UNAVAILABLE_PREFIX)
    }

    fn present_error(endpoint: &str, error: String) -> String {
        if Self::is_unavailable_error(&error) {
            return error;
        }
        let normalized = error.to_ascii_lowercase();
        if !TRANSPORT_FAILURE_MARKERS
            .iter()
            .any(|marker| normalized.contains(marker))
        {
            return error;
        }
        tracing::warn!(spiral_endpoint = endpoint, detail = %error, "SpiralDB transport failed");
        format!(
            "{SPIRAL_UNAVAILABLE_PREFIX}{endpoint}. Start the service or update Settings → SpiralDB, then try again."
        )
    }

    /// Start the database service on the current Tokio [`tokio::task::LocalSet`].
    /// The server installs that local executor beside Axum before constructing
    /// the project store.
    pub fn new(
        connection_settings: SpiralConnectionSettings,
        namespace: String,
    ) -> Result<Self, String> {
        if !crate::schema::StorageSchema::is_identifier(&namespace) {
            return Err("Spiral namespace is not a safe SQL identifier".to_owned());
        }
        let database = ProjectDatabase::start(connection_settings.clone())?;
        Ok(Self {
            connection_settings: Arc::new(RwLock::new(connection_settings)),
            namespace,
            database,
            operation_gate: Arc::new(RwLock::new(())),
            async_operation_gate: Arc::new(tokio::sync::RwLock::new(())),
        })
    }

    pub fn project_id(&self) -> String {
        self.connection_settings
            .read_unpoisoned()
            .project_id()
            .unwrap_or_default()
            .to_owned()
    }

    pub(crate) fn connection_settings(&self) -> SpiralConnectionSettings {
        self.connection_settings.read_unpoisoned().clone()
    }

    pub(crate) fn endpoint(&self) -> String {
        self.connection_settings
            .read_unpoisoned()
            .endpoint()
            .to_owned()
    }

    pub(crate) fn spfs_endpoint(&self) -> String {
        self.connection_settings
            .read_unpoisoned()
            .spfs_endpoint()
            .to_owned()
    }

    pub(crate) fn credentials(&self) -> Option<SpiralCredentials> {
        self.connection_settings
            .read_unpoisoned()
            .credentials()
            .cloned()
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.connection_settings
            .read_unpoisoned()
            .project_id()
            .is_some()
    }

    /// Initialize the native database adapter and force one authenticated
    /// project-catalog lookup without mutating project data.
    pub(crate) async fn prewarm(&self) -> Result<(), String> {
        let relation = RelationName::parse(&self.namespace).map_err(|error| error.to_string())?;
        let mut connection = self.connect().await?;
        let _ = connection
            .table_schema(relation)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Change the bootstrap pointer without introducing another data store.
    /// The exclusive gate drains active project-store work before the local
    /// database owner installs its replacement session.
    #[cfg(test)]
    pub async fn configure_project(&self, project_id: &str) -> Result<(), String> {
        let _async_operation = self.async_operation_gate.write().await;
        let settings = self
            .connection_settings
            .read_unpoisoned()
            .clone()
            .with_project(project_id)?;
        self.database.configure(settings.clone()).await?;
        let _operation = self.operation_gate.write_unpoisoned();
        *self.connection_settings.write_unpoisoned() = settings;
        Ok(())
    }

    /// Atomically replace the complete per-database destination. Active work
    /// is drained before the local owner installs the new session. The next
    /// operation opens a connection with the new scoped endpoint and credential
    /// options. An unconfigured value is accepted so a failed first-time setup
    /// can restore the bootstrap state exactly.
    pub(crate) async fn configure_connection(
        &self,
        connection_settings: SpiralConnectionSettings,
    ) -> Result<(), String> {
        let _async_operation = self.async_operation_gate.write().await;
        self.database.configure(connection_settings.clone()).await?;
        let _operation = self.operation_gate.write_unpoisoned();
        *self.connection_settings.write_unpoisoned() = connection_settings;
        Ok(())
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) async fn connect(&self) -> Result<ProjectConnection, String> {
        let operation = self.async_operation_gate.clone().read_owned().await;
        let endpoint = {
            let _synchronous_operation = self.operation_gate.read_unpoisoned();
            let settings = self.connection_settings.read_unpoisoned();
            settings.project_id().ok_or_else(|| {
                "Spiral project is not configured. Choose a project in Settings → SpiralDB."
                    .to_owned()
            })?;
            settings.endpoint().to_owned()
        };
        let connection = self
            .database
            .interface()
            .connect()
            .await
            .map_err(|error| Self::present_error(&endpoint, error.to_string()))?;
        Ok(ProjectConnection {
            connection,
            _operation: operation,
        })
    }

    /// Synchronous access for recording, import, and model workers. Async
    /// feature paths use [`ProjectStore::connect`] instead.
    pub fn run_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _async_operation =
            futures::executor::block_on(self.async_operation_gate.clone().read_owned());
        let _operation = self.operation_gate.read_unpoisoned();
        let settings = self.connection_settings.read_unpoisoned();
        settings.project_id().ok_or_else(|| {
            "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
        })?;
        let endpoint = settings.endpoint().to_owned();
        drop(settings);
        let database = self.database.interface();
        let mut connection = StorageConnection::open(database)
            .map_err(|error| Self::present_error(&endpoint, error))?;
        operation(&mut connection).map_err(|error| Self::present_error(&endpoint, error))
    }

    /// Run a durable mutation after draining async and synchronous project
    /// readers. Its native connection is request-scoped, so the next read
    /// observes a newly opened logical connection.
    pub fn run_invalidating_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _async_operation =
            futures::executor::block_on(self.async_operation_gate.clone().write_owned());
        let _operation = self.operation_gate.write_unpoisoned();
        self.run_with_gate_held(operation)
    }

    /// Run a namespace replacement without allowing another project_store call to
    /// observe its intermediate state.
    pub fn run_exclusive_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _async_operation =
            futures::executor::block_on(self.async_operation_gate.clone().write_owned());
        let _operation = self.operation_gate.write_unpoisoned();
        self.run_with_gate_held(operation)
    }

    fn run_with_gate_held<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let settings = self.connection_settings.read_unpoisoned();
        settings.project_id().ok_or_else(|| {
            "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
        })?;
        let endpoint = settings.endpoint().to_owned();
        drop(settings);
        let database = self.database.interface();
        let mut connection = StorageConnection::open(database)
            .map_err(|error| Self::present_error(&endpoint, error))?;
        operation(&mut connection).map_err(|error| Self::present_error(&endpoint, error))
    }
}

trait UnpoisonedRwLock<T> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, T>;
    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> UnpoisonedRwLock<T> for RwLock<T> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, T> {
        self.read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, T> {
        self.write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use super::*;

    struct LocalDatabaseTest;

    impl LocalDatabaseTest {
        fn run(assertions: impl Future<Output = ()>) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let database_tasks = tokio::task::LocalSet::new();
            runtime.block_on(database_tasks.run_until(assertions));
        }
    }

    #[test]
    fn project_store_can_start_unconfigured_then_select_one_project() {
        LocalDatabaseTest::run(async {
            let project_store = ProjectStore::new(
                SpiralConnectionSettings::new(
                    "https://api.spiraldb.dev".to_owned(),
                    "https://spfs.spiraldb.dev".to_owned(),
                )
                .unwrap(),
                "spiral_robotics".to_owned(),
            )
            .unwrap();

            assert!(!project_store.is_configured());
            assert!(
                project_store
                    .run_blocking(|_| Ok(()))
                    .unwrap_err()
                    .contains("Settings")
            );
            project_store
                .configure_project(" project-123 ")
                .await
                .unwrap();
            assert!(project_store.is_configured());
            assert_eq!(project_store.project_id(), "project-123");
        });
    }

    #[test]
    fn complete_connection_replacement_can_restore_unconfigured_bootstrap_state() {
        LocalDatabaseTest::run(async {
            let unconfigured = SpiralConnectionSettings::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap();
            let project_store = Arc::new(
                ProjectStore::new(unconfigured.clone(), "spiral_robotics".to_owned()).unwrap(),
            );
            let configured = unconfigured.clone().with_project("project-123").unwrap();

            let configuring_store = Arc::clone(&project_store);
            tokio::spawn(async move { configuring_store.configure_connection(configured).await })
                .await
                .unwrap()
                .unwrap();
            assert!(project_store.is_configured());
            let restoring_store = Arc::clone(&project_store);
            tokio::spawn(async move { restoring_store.configure_connection(unconfigured).await })
                .await
                .unwrap()
                .unwrap();
            assert!(!project_store.is_configured());
        });
    }

    #[test]
    fn invalid_project_selection_does_not_configure_the_project_store() {
        LocalDatabaseTest::run(async {
            let project_store = ProjectStore::new(
                SpiralConnectionSettings::new(
                    "https://api.spiraldb.dev".to_owned(),
                    "https://spfs.spiraldb.dev".to_owned(),
                )
                .unwrap(),
                "spiral_robotics".to_owned(),
            )
            .unwrap();

            assert!(
                project_store
                    .configure_project("not/a/project")
                    .await
                    .is_err()
            );
            assert!(!project_store.is_configured());
        });
    }

    #[test]
    fn native_connections_are_independent_without_a_capacity_gate() {
        LocalDatabaseTest::run(async {
            let settings = SpiralConnectionSettings::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .and_then(|settings| settings.with_project("project-123"))
            .unwrap();
            let project_store = ProjectStore::new(settings, "spiral_robotics".to_owned()).unwrap();

            let first = project_store.connect().await.unwrap();
            let second = project_store.connect().await.unwrap();

            drop(first);
            drop(second);
        });
    }

    #[test]
    fn transport_failures_become_actionable_without_rewriting_validation() {
        let unavailable = ProjectStore::present_error(
            "http://127.0.0.1:4279",
            "pinning project failed: error sending request for url".to_owned(),
        );
        assert_eq!(
            unavailable,
            "SpiralDB is unavailable at http://127.0.0.1:4279. Start the service or update Settings → SpiralDB, then try again."
        );
        assert!(ProjectStore::is_unavailable_error(&unavailable));

        let invalid = "episode bundle has the wrong format".to_owned();
        assert_eq!(
            ProjectStore::present_error("http://127.0.0.1:4279", invalid.clone()),
            invalid
        );
    }
}
