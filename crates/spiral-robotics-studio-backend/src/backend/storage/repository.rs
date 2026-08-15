//! The one physical persistence boundary used by Studio.
//!
//! Repository facades for robots, episodes, training, and project bundles all
//! share this connection pool. Nothing below this module falls back to a
//! filesystem or a second database.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use adbc_core::options::{AdbcVersion, OptionConnection, OptionDatabase};
use adbc_core::{Database as _, Driver as _};
use adbc_driver_manager::{ManagedConnection, ManagedDatabase, ManagedDriver};

const HOSTED_ADBC_CONNECTIONS: usize = 4;

#[derive(Clone)]
struct ConnectionConfig {
    driver_path: PathBuf,
    project_id: String,
}

struct CachedConnection {
    _driver: ManagedDriver,
    _database: ManagedDatabase,
    connection: ManagedConnection,
}

pub(crate) struct SpiralRepository {
    driver_path: PathBuf,
    endpoint: String,
    spfs_endpoint: String,
    project_id: Arc<RwLock<Option<String>>>,
    namespace: String,
    hosted: Arc<Mutex<Vec<CachedConnection>>>,
    hosted_slots: Arc<tokio::sync::Semaphore>,
    operation_gate: Arc<RwLock<()>>,
}

impl SpiralRepository {
    pub(crate) fn new(
        driver_path: PathBuf,
        endpoint: String,
        spfs_endpoint: String,
        project_id: Option<String>,
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
            endpoint,
            spfs_endpoint,
            project_id: Arc::new(RwLock::new(project_id)),
            namespace,
            hosted: Arc::new(Mutex::new(Vec::new())),
            hosted_slots: Arc::new(tokio::sync::Semaphore::new(HOSTED_ADBC_CONNECTIONS)),
            operation_gate: Arc::new(RwLock::new(())),
        })
    }

    pub(crate) fn project_id(&self) -> String {
        read(&self.project_id).clone().unwrap_or_default()
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }

    pub(crate) fn is_configured(&self) -> bool {
        read(&self.project_id).is_some()
    }

    /// Change the bootstrap pointer without introducing another data store.
    /// The exclusive gate drains active repository work before stale pooled
    /// connections are discarded.
    pub(crate) fn configure_project(&self, project_id: &str) -> Result<(), String> {
        let project_id = validate_project_id(project_id)?;
        let _operation = write(&self.operation_gate);
        *write(&self.project_id) = Some(project_id);
        lock(&self.hosted).clear();
        Ok(())
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) async fn run<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut ManagedConnection) -> Result<T, String> + Send + 'static,
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
        tokio::task::spawn_blocking(move || {
            let _operation = read(&operation_gate);
            let connection = connection_config(driver_path, &project_id)?;
            let mut connection = {
                let mut available = lock(&hosted);
                available.pop()
            }
            .map(Ok)
            .unwrap_or_else(|| open(connection))?;
            let result = operation(&mut connection.connection);
            if result.is_ok() {
                lock(&hosted).push(connection);
            }
            drop(permit);
            result
        })
        .await
        .map_err(|error| format!("Spiral ADBC task failed: {error}"))?
    }

    /// Use a dedicated connection for a long-lived stream such as training.
    pub(crate) async fn run_dedicated<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut ManagedConnection) -> Result<T, String> + Send + 'static,
    {
        let driver_path = self.driver_path.clone();
        let project_id = self.project_id.clone();
        let operation_gate = self.operation_gate.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = read(&operation_gate);
            let connection = connection_config(driver_path, &project_id)?;
            let mut connection = open(connection)?;
            operation(&mut connection.connection)
        })
        .await
        .map_err(|error| format!("Spiral dedicated ADBC task failed: {error}"))?
    }

    /// Synchronous access for bounded workers owned by the hardware recording
    /// path. Callers must never invoke this on an async executor thread.
    pub(crate) fn run_blocking<T>(
        &self,
        operation: impl FnOnce(&mut ManagedConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _operation = read(&self.operation_gate);
        let connection = connection_config(self.driver_path.clone(), &self.project_id)?;
        let mut connection = {
            let mut available = lock(&self.hosted);
            available.pop()
        }
        .map(Ok)
        .unwrap_or_else(|| open(connection))?;
        let result = operation(&mut connection.connection);
        if result.is_ok() {
            lock(&self.hosted).push(connection);
        }
        result
    }

    /// Run a namespace replacement without allowing another repository call to
    /// observe its intermediate state.
    pub(crate) fn run_exclusive_blocking<T>(
        &self,
        operation: impl FnOnce(&mut ManagedConnection) -> Result<T, String>,
    ) -> Result<T, String> {
        let _operation = write(&self.operation_gate);
        let connection = connection_config(self.driver_path.clone(), &self.project_id)?;
        let mut connection = {
            let mut available = lock(&self.hosted);
            available.pop()
        }
        .map(Ok)
        .unwrap_or_else(|| open(connection))?;
        let result = operation(&mut connection.connection);
        if result.is_ok() {
            lock(&self.hosted).push(connection);
        }
        result
    }
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
) -> Result<ConnectionConfig, String> {
    let project_id = read(project_id).clone().ok_or_else(|| {
        "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
    })?;
    Ok(ConnectionConfig {
        driver_path,
        project_id,
    })
}

fn open(config: ConnectionConfig) -> Result<CachedConnection, String> {
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
    let database = driver
        .new_database_with_opts([(OptionDatabase::Uri, database_uri.clone().into())])
        .map_err(|error| format!("opening Spiral ADBC database {database_uri}: {error}"))?;
    let connection = database
        .new_connection_with_opts([(OptionConnection::CurrentSchema, config.project_id.into())])
        .map_err(|error| format!("connecting Spiral ADBC database: {error}"))?;
    Ok(CachedConnection {
        _driver: driver,
        _database: database,
        connection,
    })
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
            "spiral_robotics".to_owned(),
        )
        .unwrap();

        assert!(repository.configure_project("not/a/project").is_err());
        assert!(!repository.is_configured());
    }
}
