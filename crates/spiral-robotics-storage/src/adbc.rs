//! The one physical persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all
//! share this connection pool. Nothing below this module falls back to a
//! filesystem or a second database.

use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use adbc_core::error::Status as AdbcStatus;
use adbc_core::options::{
    IngestMode, ObjectDepth, OptionConnection, OptionDatabase, OptionStatement, OptionValue,
};
use adbc_core::{Connection as _, Database as _, Driver as _, Optionable as _, Statement as _};
use adbc_driver_spiraldb::{
    Connection as SpiralConnection, Database as SpiralDatabase, Driver as SpiralDriver,
    Statement as SpiralStatement,
};
use arrow_array::{Array as _, ListArray, RecordBatchReader, StringArray, StructArray};
use arrow_schema::Schema;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::settings::{SpiralConnectionSettings, SpiralCredentials};

const HOSTED_ADBC_CONNECTIONS: usize = 4;
const SPIRAL_UNAVAILABLE_PREFIX: &str = "SpiralDB is unavailable at ";
const TRANSPORT_FAILURE_MARKERS: [&str; 5] = [
    "error sending request for url",
    "connection refused",
    "failed to connect",
    "tcp connect error",
    "dns error",
];

#[derive(Clone)]
struct RuntimeConnectionConfig {
    project_id: String,
    endpoint: String,
    spfs_endpoint: String,
    credentials: Option<SpiralCredentials>,
}

impl RuntimeConnectionConfig {
    fn from_settings(settings: &RwLock<SpiralConnectionSettings>) -> Result<Self, String> {
        let settings = settings.read_unpoisoned();
        let project_id = settings.project_id().ok_or_else(|| {
            "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
        })?;
        Ok(Self {
            project_id: project_id.to_owned(),
            endpoint: settings.endpoint().to_owned(),
            spfs_endpoint: settings.spfs_endpoint().to_owned(),
            credentials: settings.credentials().cloned(),
        })
    }

    fn open_database(&self) -> Result<SpiralDatabase, String> {
        let mut driver = SpiralDriver;
        let database_uri = format!("spiraldb://?project={}", self.project_id);
        let mut options = vec![
            (OptionDatabase::Uri, database_uri.clone().into()),
            (
                OptionDatabase::Other("spiral.api_url".to_owned()),
                self.endpoint.clone().into(),
            ),
            (
                OptionDatabase::Other("spiral.spfs_url".to_owned()),
                self.spfs_endpoint.clone().into(),
            ),
        ];
        if let Some(credentials) = &self.credentials {
            options.push((
                OptionDatabase::Username,
                credentials.client_id().to_owned().into(),
            ));
            options.push((
                OptionDatabase::Password,
                credentials.client_secret().to_owned().into(),
            ));
        }
        driver
            .new_database_with_opts(options)
            .map_err(|error| format!("opening Spiral ADBC database {database_uri}: {error}"))
    }
}

trait BlockingSemaphore {
    fn acquire_blocking(self: Arc<Self>) -> Result<OwnedSemaphorePermit, String>;
}

impl BlockingSemaphore for Semaphore {
    fn acquire_blocking(self: Arc<Self>) -> Result<OwnedSemaphorePermit, String> {
        futures::executor::block_on(self.acquire_owned())
            .map_err(|_| "Spiral ADBC connection pool is shutting down".to_owned())
    }
}

/// The only connection type visible to Studio code.
///
/// It deliberately exposes the operations Studio needs rather than the raw
/// synchronous ADBC handle. Every instance is checked out through the same
/// bounded project_store pool; async jobs additionally run away from Tokio's
/// async worker threads.
pub struct StorageConnection {
    inner: SpiralConnection,
    reusable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogRelation {
    name: String,
    relation_type: String,
}

impl CatalogRelation {
    fn new(name: String, relation_type: String) -> Self {
        Self {
            name,
            relation_type,
        }
    }

    pub(crate) fn into_name(self) -> String {
        self.name
    }

    pub(crate) fn into_parts(self) -> (String, String) {
        (self.name, self.relation_type)
    }
}

impl StorageConnection {
    fn into_reusable_inner(self) -> Option<SpiralConnection> {
        self.reusable.then_some(self.inner)
    }

    /// Prevent a connection with uncertain session state from returning to
    /// the shared pool. Dropping the wrapper then closes that handle.
    pub fn discard(&mut self) {
        self.reusable = false;
    }

    pub(crate) fn new_statement(&mut self) -> Result<StorageStatement, String> {
        self.inner
            .new_statement()
            .map(|inner| StorageStatement { inner })
            .map_err(|error| error.to_string())
    }

    /// Execute one read query without exposing the mutable ADBC statement
    /// lifecycle outside the storage crate.
    pub fn execute_query(
        &mut self,
        sql: impl AsRef<str>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'static>, String> {
        let mut statement = self.new_statement()?;
        statement.set_sql_query(sql)?;
        statement.execute()
    }

    pub fn table_schema(&self, table: &str) -> Result<Option<Schema>, String> {
        match self.inner.get_table_schema(None, None, table) {
            Ok(schema) => Ok(Some(schema)),
            Err(error) if error.status == AdbcStatus::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) fn check_catalog_table(&self, project: &str, table: &str) -> Result<(), String> {
        let reader = self
            .inner
            .get_objects(
                ObjectDepth::Tables,
                Some("spiraldb"),
                Some(project),
                Some(table),
                Some(vec!["TABLE"]),
                None,
            )
            .map_err(|error| error.to_string())?;
        for batch in reader {
            batch.map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Enumerate the durable relations advertised by the active Spiral
    /// project. Temporary query-session relations are intentionally absent
    /// from the driver's ADBC catalog.
    pub(crate) fn list_catalog_relations(
        &self,
        project: &str,
    ) -> Result<Vec<CatalogRelation>, String> {
        let reader = self
            .inner
            .get_objects(
                ObjectDepth::Tables,
                Some("spiraldb"),
                Some(project),
                None,
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
        let mut relations = Vec::new();
        for batch in reader {
            let batch = batch.map_err(|error| error.to_string())?;
            let schema_lists = batch
                .column(1)
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| "ADBC catalog returned a non-list schema column".to_owned())?;
            for catalog_row in 0..schema_lists.len() {
                if schema_lists.is_null(catalog_row) {
                    continue;
                }
                let schemas = schema_lists.value(catalog_row);
                let schemas = schemas
                    .as_any()
                    .downcast_ref::<StructArray>()
                    .ok_or_else(|| "ADBC catalog returned malformed schemas".to_owned())?;
                let table_lists = schemas
                    .column(1)
                    .as_any()
                    .downcast_ref::<ListArray>()
                    .ok_or_else(|| "ADBC catalog returned a non-list tables column".to_owned())?;
                for schema_row in 0..table_lists.len() {
                    if table_lists.is_null(schema_row) {
                        continue;
                    }
                    let tables = table_lists.value(schema_row);
                    let tables = tables
                        .as_any()
                        .downcast_ref::<StructArray>()
                        .ok_or_else(|| "ADBC catalog returned malformed tables".to_owned())?;
                    let names = tables
                        .column(0)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| "ADBC catalog returned malformed table names".to_owned())?;
                    let types = tables
                        .column(1)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| "ADBC catalog returned malformed table types".to_owned())?;
                    for table_row in 0..tables.len() {
                        if names.is_null(table_row) || types.is_null(table_row) {
                            continue;
                        }
                        relations.push(CatalogRelation::new(
                            names.value(table_row).to_owned(),
                            types.value(table_row).to_owned(),
                        ));
                    }
                }
            }
        }
        relations.sort_by(|left, right| left.name.cmp(&right.name));
        relations.dedup_by(|left, right| left.name == right.name);
        Ok(relations)
    }

    /// List durable relation names without exposing the driver's catalog
    /// representation across the storage boundary.
    pub fn catalog_relation_names(&self, project: &str) -> Result<Vec<String>, String> {
        self.list_catalog_relations(project).map(|relations| {
            relations
                .into_iter()
                .map(|relation| relation.name)
                .collect()
        })
    }

    pub fn set_autocommit(&mut self, enabled: bool) -> Result<(), String> {
        self.inner
            .set_option(OptionConnection::AutoCommit, OptionValue::from(enabled))
            .map_err(|error| error.to_string())
    }

    pub fn commit(&mut self) -> Result<(), String> {
        self.inner.commit().map_err(|error| error.to_string())
    }

    pub fn rollback(&mut self) -> Result<(), String> {
        self.inner.rollback().map_err(|error| error.to_string())
    }
}

/// A statement whose synchronous ADBC implementation remains private to this
/// adapter. Callers can compose project_store work without importing ADBC.
pub(crate) struct StorageStatement {
    inner: SpiralStatement,
}

impl StorageStatement {
    pub(crate) fn set_sql_query(&mut self, sql: impl AsRef<str>) -> Result<(), String> {
        self.inner
            .set_sql_query(sql)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn configure_ingest(&mut self, target: &str) -> Result<(), String> {
        self.configure_ingest_mode(target, IngestMode::CreateAppend)
    }

    pub(crate) fn configure_replace(&mut self, target: &str) -> Result<(), String> {
        self.configure_ingest_mode(target, IngestMode::Replace)
    }

    fn configure_ingest_mode(&mut self, target: &str, mode: IngestMode) -> Result<(), String> {
        self.inner
            .set_option(OptionStatement::TargetTable, target.into())
            .map_err(|error| error.to_string())?;
        self.inner
            .set_option(OptionStatement::IngestMode, mode.into())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn bind_stream(
        &mut self,
        reader: Box<dyn RecordBatchReader + Send>,
    ) -> Result<(), String> {
        self.inner
            .bind_stream(reader)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn execute(
        &mut self,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'static>, String> {
        self.inner.execute().map_err(|error| error.to_string())
    }

    pub(crate) fn execute_update(&mut self) -> Result<Option<i64>, String> {
        self.inner
            .execute_update()
            .map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct ConnectionPool {
    database: Option<SpiralDatabase>,
    available: Vec<SpiralConnection>,
}

impl ConnectionPool {
    fn checkout(&mut self, config: &RuntimeConnectionConfig) -> Result<SpiralConnection, String> {
        if let Some(connection) = self.available.pop() {
            return Ok(connection);
        }
        if self.database.is_none() {
            self.database = Some(config.open_database()?);
        }
        self.database
            .as_ref()
            .expect("an initialized ADBC pool has one database")
            .new_connection_with_opts([(
                OptionConnection::CurrentSchema,
                config.project_id.clone().into(),
            )])
            .map_err(|error| format!("connecting Spiral ADBC database: {error}"))
    }

    fn checkin(&mut self, connection: SpiralConnection) {
        debug_assert!(
            self.available.len() < HOSTED_ADBC_CONNECTIONS,
            "the bounded checkout gate prevents an oversized idle pool"
        );
        self.available.push(connection);
    }

    fn reset(&mut self) {
        self.available.clear();
        self.database = None;
    }
}

pub struct ProjectStore {
    connection_settings: Arc<RwLock<SpiralConnectionSettings>>,
    namespace: String,
    hosted: Arc<Mutex<ConnectionPool>>,
    hosted_slots: Arc<Semaphore>,
    operation_gate: Arc<RwLock<()>>,
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

    fn acquire_blocking_slot(&self) -> Result<OwnedSemaphorePermit, String> {
        self.hosted_slots.clone().acquire_blocking()
    }

    pub fn new(
        connection_settings: SpiralConnectionSettings,
        namespace: String,
    ) -> Result<Self, String> {
        if !crate::schema::StorageSchema::is_identifier(&namespace) {
            return Err("Spiral namespace is not a safe SQL identifier".to_owned());
        }
        Ok(Self {
            connection_settings: Arc::new(RwLock::new(connection_settings)),
            namespace,
            hosted: Arc::new(Mutex::new(ConnectionPool::default())),
            hosted_slots: Arc::new(Semaphore::new(HOSTED_ADBC_CONNECTIONS)),
            operation_gate: Arc::new(RwLock::new(())),
        })
    }

    pub fn project_id(&self) -> String {
        self.connection_settings
            .read_unpoisoned()
            .project_id()
            .unwrap_or_default()
            .to_owned()
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

    /// Initialize the shared driver/database and force one authenticated
    /// project-catalog lookup without mutating project data.
    pub(crate) async fn prewarm(&self) -> Result<(), String> {
        let namespace = self.namespace.clone();
        self.run(move |connection| {
            let _ = connection.table_schema(&namespace)?;
            Ok(())
        })
        .await
    }

    /// Change the bootstrap pointer without introducing another data store.
    /// The exclusive gate drains active project_store work before stale pooled
    /// connections are discarded.
    #[cfg(test)]
    pub fn configure_project(&self, project_id: &str) -> Result<(), String> {
        let _operation = self.operation_gate.write_unpoisoned();
        let settings = self
            .connection_settings
            .read_unpoisoned()
            .clone()
            .with_project(project_id)?;
        *self.connection_settings.write_unpoisoned() = settings;
        self.hosted.lock_unpoisoned().reset();
        Ok(())
    }

    /// Atomically switch the complete per-database destination. Active work is
    /// drained before the old pool is discarded; the next checkout opens a
    /// database with the new scoped endpoint and credential options.
    pub(crate) fn configure_connection(
        &self,
        connection_settings: SpiralConnectionSettings,
    ) -> Result<(), String> {
        if connection_settings.project_id().is_none() {
            return Err(
                "Spiral project is not configured. Choose a project in Settings → SpiralDB."
                    .to_owned(),
            );
        }
        let _operation = self.operation_gate.write_unpoisoned();
        *self.connection_settings.write_unpoisoned() = connection_settings;
        self.hosted.lock_unpoisoned().reset();
        Ok(())
    }

    /// Drop idle read snapshots after another process commits to this project.
    /// Active calls finish normally; their connections are discarded when the
    /// worker-owned checkpoint event reaches Studio.
    pub fn invalidate_idle_connections(&self) {
        let _operation = self.operation_gate.write_unpoisoned();
        self.hosted.lock_unpoisoned().available.clear();
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) async fn run<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut StorageConnection) -> Result<T, String> + Send + 'static,
    {
        let permit = self
            .hosted_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Spiral ADBC connection pool is shutting down".to_owned())?;
        let hosted = self.hosted.clone();
        let operation_gate = self.operation_gate.clone();
        let connection_settings = self.connection_settings.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = operation_gate.read_unpoisoned();
            let connection = RuntimeConnectionConfig::from_settings(&connection_settings)?;
            let display_endpoint = connection.endpoint.clone();
            let connection = hosted
                .lock_unpoisoned()
                .checkout(&connection)
                .map_err(|error| ProjectStore::present_error(&display_endpoint, error))?;
            let mut connection = StorageConnection {
                inner: connection,
                reusable: true,
            };
            let result = operation(&mut connection);
            if result.is_ok()
                && let Some(connection) = connection.into_reusable_inner()
            {
                hosted.lock_unpoisoned().checkin(connection);
            }
            drop(permit);
            result.map_err(|error| ProjectStore::present_error(&display_endpoint, error))
        })
        .await
        .map_err(|error| format!("Spiral ADBC task failed: {error}"))?
    }

    /// Synchronous access for bounded workers owned by the hardware recording
    /// path. Callers must never invoke this on an async executor thread.
    pub fn run_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = self.acquire_blocking_slot()?;
        let _operation = self.operation_gate.read_unpoisoned();
        let connection = RuntimeConnectionConfig::from_settings(&self.connection_settings)?;
        let endpoint = connection.endpoint.clone();
        let connection = self
            .hosted
            .lock_unpoisoned()
            .checkout(&connection)
            .map_err(|error| Self::present_error(&endpoint, error))?;
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        if result.is_ok()
            && let Some(connection) = connection.into_reusable_inner()
        {
            self.hosted.lock_unpoisoned().checkin(connection);
        }
        result.map_err(|error| Self::present_error(&endpoint, error))
    }

    /// Run a durable mutation after draining project_store readers, with no
    /// pooled connection surviving across the write.
    ///
    /// Spiral connections can retain a read snapshot across statements. A
    /// later setup poll on one of those connections would otherwise keep
    /// projecting the robot document from before a successful write until the
    /// server recreated its pool. Mutations whose result is immediately
    /// exposed through Studio state use this path to provide read-your-writes
    /// behavior without giving up pooling for ordinary reads.
    pub fn run_invalidating_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = self.acquire_blocking_slot()?;
        let _operation = self.operation_gate.write_unpoisoned();
        let connection = RuntimeConnectionConfig::from_settings(&self.connection_settings)?;
        let endpoint = connection.endpoint.clone();
        self.hosted.lock_unpoisoned().available.clear();
        let connection = self
            .hosted
            .lock_unpoisoned()
            .checkout(&connection)
            .map_err(|error| Self::present_error(&endpoint, error))?;
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        result.map_err(|error| Self::present_error(&endpoint, error))
    }

    /// Run a namespace replacement without allowing another project_store call to
    /// observe its intermediate state.
    pub fn run_exclusive_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = self.acquire_blocking_slot()?;
        let _operation = self.operation_gate.write_unpoisoned();
        let connection = RuntimeConnectionConfig::from_settings(&self.connection_settings)?;
        let endpoint = connection.endpoint.clone();
        let connection = {
            let mut hosted = self.hosted.lock_unpoisoned();
            // The shared ADBC database pins project catalog state as well as
            // its connections. Once the exclusive gate is held there can be
            // no live project_store readers, so reset both layers and start the
            // replacement from the latest project version.
            hosted.reset();
            hosted
                .checkout(&connection)
                .map_err(|error| Self::present_error(&endpoint, error))?
        };
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        if result.is_ok()
            && let Some(connection) = connection.into_reusable_inner()
        {
            self.hosted.lock_unpoisoned().checkin(connection);
        }
        result.map_err(|error| Self::present_error(&endpoint, error))
    }
}

trait UnpoisonedMutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T> UnpoisonedMutex<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    use super::*;

    #[test]
    fn project_store_can_start_unconfigured_then_select_one_project() {
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
        project_store.configure_project(" project-123 ").unwrap();
        assert!(project_store.is_configured());
        assert_eq!(project_store.project_id(), "project-123");
    }

    #[test]
    fn invalid_project_selection_does_not_configure_the_project_store() {
        let project_store = ProjectStore::new(
            SpiralConnectionSettings::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap(),
            "spiral_robotics".to_owned(),
        )
        .unwrap();

        assert!(project_store.configure_project("not/a/project").is_err());
        assert!(!project_store.is_configured());
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

    #[test]
    fn blocking_and_async_checkouts_share_one_capacity_gate() {
        use std::sync::mpsc;
        use std::time::Duration;

        let semaphore = Arc::new(Semaphore::new(1));
        let async_permit = semaphore
            .clone()
            .try_acquire_owned()
            .expect("the async checkout takes the only slot");
        let (finished, receiver) = mpsc::sync_channel(1);
        let waiting = semaphore.clone();
        let worker = std::thread::spawn(move || {
            let permit = waiting
                .acquire_blocking()
                .expect("the shared pool remains open");
            finished.send(()).unwrap();
            drop(permit);
        });

        assert!(receiver.recv_timeout(Duration::from_millis(25)).is_err());
        drop(async_permit);
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("releasing the async slot wakes the blocking checkout");
        worker.join().unwrap();
    }
}
