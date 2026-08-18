//! The one physical persistence boundary used by Studio.
//!
//! Repository facades for robots, episodes, training, and project bundles all
//! share this connection pool. Nothing below this module falls back to a
//! filesystem or a second database.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use adbc_core::error::Status as AdbcStatus;
use adbc_core::options::{
    AdbcVersion, IngestMode, ObjectDepth, OptionConnection, OptionDatabase, OptionStatement,
    OptionValue,
};
use adbc_core::{Connection as _, Database as _, Driver as _, Optionable as _, Statement as _};
use adbc_driver_manager::{ManagedConnection, ManagedDatabase, ManagedDriver, ManagedStatement};
use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::Schema;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::backend::storage::settings::SpiralCredentials;

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
    driver_path: PathBuf,
    project_id: String,
    endpoint: String,
    spfs_endpoint: String,
    credentials: Option<SpiralCredentials>,
}

/// The only connection type visible to Studio code.
///
/// It deliberately exposes the operations Studio needs rather than the raw
/// synchronous ADBC handle. Every instance is checked out through the same
/// bounded repository pool; async jobs additionally run away from Tokio's
/// async worker threads.
pub(crate) struct RepositoryConnection {
    inner: ManagedConnection,
    reusable: bool,
}

impl RepositoryConnection {
    fn into_reusable_inner(self) -> Option<ManagedConnection> {
        self.reusable.then_some(self.inner)
    }

    /// Prevent a connection with uncertain session state from returning to
    /// the shared pool. Dropping the wrapper then closes that handle.
    pub(crate) fn discard(&mut self) {
        self.reusable = false;
    }

    pub(crate) fn new_statement(&mut self) -> Result<RepositoryStatement, String> {
        self.inner
            .new_statement()
            .map(|inner| RepositoryStatement { inner })
            .map_err(|error| error.to_string())
    }

    pub(crate) fn table_schema(&self, table: &str) -> Result<Option<Schema>, String> {
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

    pub(crate) fn set_autocommit(&mut self, enabled: bool) -> Result<(), String> {
        self.inner
            .set_option(OptionConnection::AutoCommit, OptionValue::from(enabled))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn commit(&mut self) -> Result<(), String> {
        self.inner.commit().map_err(|error| error.to_string())
    }

    pub(crate) fn rollback(&mut self) -> Result<(), String> {
        self.inner.rollback().map_err(|error| error.to_string())
    }
}

/// A statement whose synchronous ADBC implementation remains private to this
/// adapter. Callers can compose repository work without importing ADBC.
pub(crate) struct RepositoryStatement {
    inner: ManagedStatement,
}

impl RepositoryStatement {
    pub(crate) fn set_sql_query(&mut self, sql: impl AsRef<str>) -> Result<(), String> {
        self.inner
            .set_sql_query(sql)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn configure_ingest(&mut self, target: &str) -> Result<(), String> {
        self.inner
            .set_option(OptionStatement::TargetTable, target.into())
            .map_err(|error| error.to_string())?;
        self.inner
            .set_option(OptionStatement::IngestMode, IngestMode::CreateAppend.into())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn bind(&mut self, batch: RecordBatch) -> Result<(), String> {
        self.inner.bind(batch).map_err(|error| error.to_string())
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
    driver: Option<ManagedDriver>,
    database: Option<ManagedDatabase>,
    available: Vec<ManagedConnection>,
}

impl ConnectionPool {
    fn checkout(&mut self, config: &ConnectionConfig) -> Result<ManagedConnection, String> {
        if let Some(connection) = self.available.pop() {
            return Ok(connection);
        }
        if self.database.is_none() {
            let (driver, database) = open_database(config)?;
            self.driver = Some(driver);
            self.database = Some(database);
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

    fn checkin(&mut self, connection: ManagedConnection) {
        debug_assert!(
            self.available.len() < HOSTED_ADBC_CONNECTIONS,
            "the bounded checkout gate prevents an oversized idle pool"
        );
        self.available.push(connection);
    }

    fn reset(&mut self) {
        self.available.clear();
        self.database = None;
        self.driver = None;
    }
}

pub(crate) struct SpiralRepository {
    driver_path: PathBuf,
    endpoint: Arc<RwLock<String>>,
    spfs_endpoint: Arc<RwLock<String>>,
    credentials: Arc<RwLock<Option<SpiralCredentials>>>,
    project_id: Arc<RwLock<Option<String>>>,
    namespace: String,
    hosted: Arc<Mutex<ConnectionPool>>,
    hosted_slots: Arc<Semaphore>,
    operation_gate: Arc<RwLock<()>>,
}

impl SpiralRepository {
    pub(crate) fn new(
        driver_path: PathBuf,
        endpoint: String,
        spfs_endpoint: String,
        project_id: Option<String>,
        credentials: Option<SpiralCredentials>,
        namespace: String,
    ) -> Result<Self, String> {
        if !driver_path.is_absolute() || !driver_path.is_file() {
            return Err("Spiral ADBC driver must be an existing absolute file path".to_owned());
        }
        let project_id = project_id
            .map(|value| validate_project_id(&value))
            .transpose()?;
        if !spiral_robotics_contracts::spiral::is_identifier(&namespace) {
            return Err("Spiral namespace is not a safe SQL identifier".to_owned());
        }
        Ok(Self {
            driver_path,
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

    pub(crate) fn project_id(&self) -> String {
        read(&self.project_id).clone().unwrap_or_default()
    }

    pub(crate) fn endpoint(&self) -> String {
        read(&self.endpoint).clone()
    }

    pub(crate) fn spfs_endpoint(&self) -> String {
        read(&self.spfs_endpoint).clone()
    }

    pub(crate) fn credentials(&self) -> Option<SpiralCredentials> {
        read(&self.credentials).clone()
    }

    pub(crate) fn is_configured(&self) -> bool {
        read(&self.project_id).is_some()
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
    /// The exclusive gate drains active repository work before stale pooled
    /// connections are discarded.
    #[cfg(test)]
    pub(crate) fn configure_project(&self, project_id: &str) -> Result<(), String> {
        let project_id = validate_project_id(project_id)?;
        let _operation = write(&self.operation_gate);
        *write(&self.project_id) = Some(project_id);
        lock(&self.hosted).reset();
        Ok(())
    }

    /// Atomically switch the complete per-database destination. Active work is
    /// drained before the old pool is discarded; the next checkout opens a
    /// database with the new scoped endpoint and credential options.
    pub(crate) fn configure_connection(
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
    pub(crate) fn invalidate_idle_connections(&self) {
        let _operation = write(&self.operation_gate);
        lock(&self.hosted).available.clear();
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) async fn run<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut RepositoryConnection) -> Result<T, String> + Send + 'static,
    {
        let permit = self
            .hosted_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Spiral ADBC connection pool is shutting down".to_owned())?;
        let hosted = self.hosted.clone();
        let operation_gate = self.operation_gate.clone();
        let driver_path = self.driver_path.clone();
        let project_id = self.project_id.clone();
        let endpoint = self.endpoint.clone();
        let spfs_endpoint = self.spfs_endpoint.clone();
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = read(&operation_gate);
            let connection = connection_config(
                driver_path,
                &project_id,
                &endpoint,
                &spfs_endpoint,
                &credentials,
            )?;
            let display_endpoint = connection.endpoint.clone();
            let connection = lock(&hosted)
                .checkout(&connection)
                .map_err(|error| present_error(&display_endpoint, error))?;
            let mut connection = RepositoryConnection {
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
    pub(crate) fn run_blocking<T>(
        &self,
        operation: impl FnOnce(&mut RepositoryConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = read(&self.operation_gate);
        let connection = connection_config(
            self.driver_path.clone(),
            &self.project_id,
            &self.endpoint,
            &self.spfs_endpoint,
            &self.credentials,
        )?;
        let endpoint = connection.endpoint.clone();
        let connection = lock(&self.hosted)
            .checkout(&connection)
            .map_err(|error| present_error(&endpoint, error))?;
        let mut connection = RepositoryConnection {
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

    /// Run a durable mutation after draining repository readers, with no
    /// pooled connection surviving across the write.
    ///
    /// Spiral connections can retain a read snapshot across statements. A
    /// later setup poll on one of those connections would otherwise keep
    /// projecting the robot document from before a successful write until the
    /// server recreated its pool. Mutations whose result is immediately
    /// exposed through Studio state use this path to provide read-your-writes
    /// behavior without giving up pooling for ordinary reads.
    pub(crate) fn run_invalidating_blocking<T>(
        &self,
        operation: impl FnOnce(&mut RepositoryConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = write(&self.operation_gate);
        let connection = connection_config(
            self.driver_path.clone(),
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
        let mut connection = RepositoryConnection {
            inner: connection,
            reusable: true,
        };
        let result = operation(&mut connection);
        result.map_err(|error| present_error(&endpoint, error))
    }

    /// Run a namespace replacement without allowing another repository call to
    /// observe its intermediate state.
    pub(crate) fn run_exclusive_blocking<T>(
        &self,
        operation: impl FnOnce(&mut RepositoryConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _permit = acquire_blocking(self.hosted_slots.clone())?;
        let _operation = write(&self.operation_gate);
        let connection = connection_config(
            self.driver_path.clone(),
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
            // no live repository readers, so reset both layers and start the
            // replacement from the latest project version.
            hosted.reset();
            hosted
                .checkout(&connection)
                .map_err(|error| present_error(&endpoint, error))?
        };
        let mut connection = RepositoryConnection {
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
pub(crate) fn is_unavailable_error(message: &str) -> bool {
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
    driver_path: PathBuf,
    project_id: &RwLock<Option<String>>,
    endpoint: &RwLock<String>,
    spfs_endpoint: &RwLock<String>,
    credentials: &RwLock<Option<SpiralCredentials>>,
) -> Result<ConnectionConfig, String> {
    let project_id = read(project_id).clone().ok_or_else(|| {
        "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
    })?;
    Ok(ConnectionConfig {
        driver_path,
        project_id,
        endpoint: read(endpoint).clone(),
        spfs_endpoint: read(spfs_endpoint).clone(),
        credentials: read(credentials).clone(),
    })
}

fn open_database(config: &ConnectionConfig) -> Result<(ManagedDriver, ManagedDatabase), String> {
    let mut driver = ManagedDriver::load_dynamic_from_filename(
        &config.driver_path,
        Some(b"AdbcDriverSpiraldbInit"),
        AdbcVersion::V110,
    )
    .map_err(|error| {
        format!(
            "loading Spiral ADBC driver {}: {error}",
            config.driver_path.display()
        )
    })?;
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
    Ok((driver, database))
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
    fn repository_can_start_unconfigured_then_select_one_project() {
        let driver = tempfile::NamedTempFile::new().unwrap();
        let repository = SpiralRepository::new(
            driver.path().to_owned(),
            "https://api.spiraldb.dev".to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
            None,
            None,
            "spiral_robotics".to_owned(),
        )
        .unwrap();

        assert!(!repository.is_configured());
        assert!(
            repository
                .run_blocking(|_| Ok(()))
                .unwrap_err()
                .contains("Settings")
        );
        repository.configure_project(" project-123 ").unwrap();
        assert!(repository.is_configured());
        assert_eq!(repository.project_id(), "project-123");
    }

    #[test]
    fn invalid_project_selection_does_not_configure_the_repository() {
        let driver = tempfile::NamedTempFile::new().unwrap();
        let repository = SpiralRepository::new(
            driver.path().to_owned(),
            "https://api.spiraldb.dev".to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
            None,
            None,
            "spiral_robotics".to_owned(),
        )
        .unwrap();

        assert!(repository.configure_project("not/a/project").is_err());
        assert!(!repository.is_configured());
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
