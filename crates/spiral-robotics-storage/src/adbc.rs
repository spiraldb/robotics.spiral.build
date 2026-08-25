//! The one physical persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all
//! share this connection pool. Nothing below this module falls back to a
//! filesystem or a second database.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};

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

use crate::settings::SpiralCredentials;

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
struct ConnectionConfig {
    project_id: String,
    endpoint: String,
    spfs_endpoint: String,
    credentials: Option<SpiralCredentials>,
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
pub struct CatalogRelation {
    pub name: String,
    pub relation_type: String,
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

    pub fn new_statement(&mut self) -> Result<StorageStatement, String> {
        self.inner
            .new_statement()
            .map(|inner| StorageStatement { inner })
            .map_err(|error| error.to_string())
    }

    pub fn table_schema(&self, table: &str) -> Result<Option<Schema>, String> {
        match self.inner.get_table_schema(None, None, table) {
            Ok(schema) => Ok(Some(schema)),
            Err(error) if error.status == AdbcStatus::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn check_catalog_table(&self, project: &str, table: &str) -> Result<(), String> {
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
    pub fn list_catalog_relations(&self, project: &str) -> Result<Vec<CatalogRelation>, String> {
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
                        relations.push(CatalogRelation {
                            name: names.value(table_row).to_owned(),
                            relation_type: types.value(table_row).to_owned(),
                        });
                    }
                }
            }
        }
        relations.sort_by(|left, right| left.name.cmp(&right.name));
        relations.dedup_by(|left, right| left.name == right.name);
        Ok(relations)
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
pub struct StorageStatement {
    inner: SpiralStatement,
}

impl StorageStatement {
    pub fn set_sql_query(&mut self, sql: impl AsRef<str>) -> Result<(), String> {
        self.inner
            .set_sql_query(sql)
            .map_err(|error| error.to_string())
    }

    pub fn configure_ingest(&mut self, target: &str) -> Result<(), String> {
        self.configure_ingest_mode(target, IngestMode::CreateAppend)
    }

    pub fn configure_replace(&mut self, target: &str) -> Result<(), String> {
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

    pub fn bind_stream(&mut self, reader: Box<dyn RecordBatchReader + Send>) -> Result<(), String> {
        self.inner
            .bind_stream(reader)
            .map_err(|error| error.to_string())
    }

    pub fn execute(&mut self) -> Result<Box<dyn RecordBatchReader + Send + 'static>, String> {
        self.inner.execute().map_err(|error| error.to_string())
    }

    pub fn execute_update(&mut self) -> Result<Option<i64>, String> {
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
    fn checkout(&mut self, config: &ConnectionConfig) -> Result<SpiralConnection, String> {
        if let Some(connection) = self.available.pop() {
            return Ok(connection);
        }
        if self.database.is_none() {
            self.database = Some(open_database(config)?);
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
    endpoint: Arc<RwLock<String>>,
    spfs_endpoint: Arc<RwLock<String>>,
    credentials: Arc<RwLock<Option<SpiralCredentials>>>,
    project_id: Arc<RwLock<Option<String>>>,
    namespace: String,
    hosted: Arc<Mutex<ConnectionPool>>,
    hosted_slots: Arc<Semaphore>,
    operation_gate: Arc<RwLock<()>>,
}

impl ProjectStore {
    pub fn new(
        endpoint: String,
        spfs_endpoint: String,
        project_id: Option<String>,
        credentials: Option<SpiralCredentials>,
        namespace: String,
    ) -> Result<Self, String> {
        let project_id = project_id
            .map(|value| validate_project_id(&value))
            .transpose()?;
        if !crate::schema::is_identifier(&namespace) {
            return Err("Spiral namespace is not a safe SQL identifier".to_owned());
        }
        Ok(Self {
            endpoint: Arc::new(RwLock::new(endpoint)),
            spfs_endpoint: Arc::new(RwLock::new(spfs_endpoint)),
            credentials: Arc::new(RwLock::new(credentials)),
            project_id: Arc::new(RwLock::new(project_id)),
            namespace,
            hosted: Arc::new(Mutex::new(ConnectionPool::default())),
            hosted_slots: Arc::new(Semaphore::new(HOSTED_ADBC_CONNECTIONS)),
            operation_gate: Arc::new(RwLock::new(())),
        })
    }

    pub fn project_id(&self) -> String {
        read(&self.project_id).clone().unwrap_or_default()
    }

    pub fn endpoint(&self) -> String {
        read(&self.endpoint).clone()
    }

    pub fn spfs_endpoint(&self) -> String {
        read(&self.spfs_endpoint).clone()
    }

    pub fn credentials(&self) -> Option<SpiralCredentials> {
        read(&self.credentials).clone()
    }

    pub fn is_configured(&self) -> bool {
        read(&self.project_id).is_some()
    }

    /// Initialize the shared driver/database and force one authenticated
    /// project-catalog lookup without mutating project data.
    pub async fn prewarm(&self) -> Result<(), String> {
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
        let project_id = validate_project_id(project_id)?;
        let _operation = write(&self.operation_gate);
        *write(&self.project_id) = Some(project_id);
        lock(&self.hosted).reset();
        Ok(())
    }

    /// Atomically switch the complete per-database destination. Active work is
    /// drained before the old pool is discarded; the next checkout opens a
    /// database with the new scoped endpoint and credential options.
    pub fn configure_connection(
        &self,
        endpoint: String,
        spfs_endpoint: String,
        project_id: &str,
        credentials: Option<SpiralCredentials>,
    ) -> Result<(), String> {
        let project_id = validate_project_id(project_id)?;
        let _operation = write(&self.operation_gate);
        *write(&self.endpoint) = endpoint;
        *write(&self.spfs_endpoint) = spfs_endpoint;
        *write(&self.project_id) = Some(project_id);
        *write(&self.credentials) = credentials;
        lock(&self.hosted).reset();
        Ok(())
    }

    /// Drop idle read snapshots after another process commits to this project.
    /// Active calls finish normally; their connections are discarded when the
    /// worker-owned checkpoint event reaches Studio.
    pub fn invalidate_idle_connections(&self) {
        let _operation = write(&self.operation_gate);
        lock(&self.hosted).available.clear();
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub async fn run<T, F>(&self, operation: F) -> Result<T, String>
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
        let project_id = self.project_id.clone();
        let endpoint = self.endpoint.clone();
        let spfs_endpoint = self.spfs_endpoint.clone();
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = read(&operation_gate);
            let connection =
                connection_config(&project_id, &endpoint, &spfs_endpoint, &credentials)?;
            let display_endpoint = connection.endpoint.clone();
            let connection = lock(&hosted)
                .checkout(&connection)
                .map_err(|error| present_error(&display_endpoint, error))?;
            let mut connection = StorageConnection {
                inner: connection,
                reusable: true,
            };
            let result = operation(&mut connection);
            if result.is_ok()
                && let Some(connection) = connection.into_reusable_inner()
            {
                lock(&hosted).checkin(connection);
            }
            drop(permit);
            result.map_err(|error| present_error(&display_endpoint, error))
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
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = read(&self.operation_gate);
        let connection = connection_config(
            &self.project_id,
            &self.endpoint,
            &self.spfs_endpoint,
            &self.credentials,
        )?;
        let endpoint = connection.endpoint.clone();
        let connection = lock(&self.hosted)
            .checkout(&connection)
            .map_err(|error| present_error(&endpoint, error))?;
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        if result.is_ok()
            && let Some(connection) = connection.into_reusable_inner()
        {
            lock(&self.hosted).checkin(connection);
        }
        result.map_err(|error| present_error(&endpoint, error))
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
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = write(&self.operation_gate);
        let connection = connection_config(
            &self.project_id,
            &self.endpoint,
            &self.spfs_endpoint,
            &self.credentials,
        )?;
        let endpoint = connection.endpoint.clone();
        lock(&self.hosted).available.clear();
        let connection = lock(&self.hosted)
            .checkout(&connection)
            .map_err(|error| present_error(&endpoint, error))?;
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        result.map_err(|error| present_error(&endpoint, error))
    }

    /// Run a namespace replacement without allowing another project_store call to
    /// observe its intermediate state.
    pub fn run_exclusive_blocking<T>(
        &self,
        operation: impl FnOnce(&mut StorageConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = write(&self.operation_gate);
        let connection = connection_config(
            &self.project_id,
            &self.endpoint,
            &self.spfs_endpoint,
            &self.credentials,
        )?;
        let endpoint = connection.endpoint.clone();
        let connection = {
            let mut hosted = lock(&self.hosted);
            // The shared ADBC database pins project catalog state as well as
            // its connections. Once the exclusive gate is held there can be
            // no live project_store readers, so reset both layers and start the
            // replacement from the latest project version.
            hosted.reset();
            hosted
                .checkout(&connection)
                .map_err(|error| present_error(&endpoint, error))?
        };
        let mut connection = StorageConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        if result.is_ok()
            && let Some(connection) = connection.into_reusable_inner()
        {
            lock(&self.hosted).checkin(connection);
        }
        result.map_err(|error| present_error(&endpoint, error))
    }
}

/// Identify the stable operator-facing form so delivery adapters can use an
/// availability status instead of misclassifying the failure as bad input.
pub fn is_unavailable_error(message: &str) -> bool {
    message.starts_with(SPIRAL_UNAVAILABLE_PREFIX)
}

fn present_error(endpoint: &str, error: String) -> String {
    if is_unavailable_error(&error) {
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

fn validate_project_id(project_id: &str) -> Result<String, String> {
    let project_id = project_id.trim();
    if project_id.is_empty()
        || !project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Spiral project must contain only letters, numbers, '_' or '-'".to_owned());
    }
    Ok(project_id.to_owned())
}

fn connection_config(
    project_id: &RwLock<Option<String>>,
    endpoint: &RwLock<String>,
    spfs_endpoint: &RwLock<String>,
    credentials: &RwLock<Option<SpiralCredentials>>,
) -> Result<ConnectionConfig, String> {
    let project_id = read(project_id).clone().ok_or_else(|| {
        "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
    })?;
    Ok(ConnectionConfig {
        project_id,
        endpoint: read(endpoint).clone(),
        spfs_endpoint: read(spfs_endpoint).clone(),
        credentials: read(credentials).clone(),
    })
}

fn open_database(config: &ConnectionConfig) -> Result<SpiralDatabase, String> {
    let mut driver = SpiralDriver;
    let database_uri = format!("spiraldb://?project={}", config.project_id);
    let mut options = vec![
        (OptionDatabase::Uri, database_uri.clone().into()),
        (
            OptionDatabase::Other("spiral.api_url".to_owned()),
            config.endpoint.clone().into(),
        ),
        (
            OptionDatabase::Other("spiral.spfs_url".to_owned()),
            config.spfs_endpoint.clone().into(),
        ),
    ];
    if let Some(credentials) = &config.credentials {
        options.push((
            OptionDatabase::Username,
            credentials.client_id.clone().into(),
        ));
        options.push((
            OptionDatabase::Password,
            credentials.client_secret.clone().into(),
        ));
    }
    let database = driver
        .new_database_with_opts(options)
        .map_err(|error| format!("opening Spiral ADBC database {database_uri}: {error}"))?;
    Ok(database)
}

fn acquire_blocking(semaphore: Arc<Semaphore>) -> Result<OwnedSemaphorePermit, String> {
    futures::executor::block_on(semaphore.acquire_owned())
        .map_err(|_| "Spiral ADBC connection pool is shutting down".to_owned())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_store_can_start_unconfigured_then_select_one_project() {
        let project_store = ProjectStore::new(
            "https://api.spiraldb.dev".to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
            None,
            None,
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
            "https://api.spiraldb.dev".to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
            None,
            None,
            "spiral_robotics".to_owned(),
        )
        .unwrap();

        assert!(project_store.configure_project("not/a/project").is_err());
        assert!(!project_store.is_configured());
    }

    #[test]
    fn transport_failures_become_actionable_without_rewriting_validation() {
        let unavailable = present_error(
            "http://127.0.0.1:4279",
            "pinning project failed: error sending request for url".to_owned(),
        );
        assert_eq!(
            unavailable,
            "SpiralDB is unavailable at http://127.0.0.1:4279. Start the service or update Settings → SpiralDB, then try again."
        );
        assert!(is_unavailable_error(&unavailable));

        let invalid = "episode bundle has the wrong format".to_owned();
        assert_eq!(
            present_error("http://127.0.0.1:4279", invalid.clone()),
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
            let permit = acquire_blocking(waiting).expect("the shared pool remains open");
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
