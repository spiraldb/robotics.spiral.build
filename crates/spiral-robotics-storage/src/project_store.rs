//! The project-scoped persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all use
//! the same async database boundary. Nothing below this module falls back to
//! a filesystem or a second database.

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use arrow_schema::Schema;
use futures::FutureExt as _;

use crate::database::{
    Database, DatabaseCommand, DatabaseConnection, DatabaseError, DatabaseFuture, DatabaseOutput,
    DatabaseSession, DatabaseTransaction, RecordBatchStream, RelationName,
};
use crate::settings::{SpiralConnectionSettings, SpiralCredentials};
use crate::spiral_database::SpiralDatabase;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

const SPIRAL_UNAVAILABLE_PREFIX: &str = "SpiralDB is unavailable at ";
const TRANSPORT_FAILURE_MARKERS: [&str; 5] = [
    "error sending request for url",
    "connection refused",
    "failed to connect",
    "tcp connect error",
    "dns error",
];

/// One async, project-scoped logical connection.
///
/// The boxed state is always a `Send` proxy. The native Spiral connection stays
/// on its `LocalSet` owner for the complete lifetime of this value.
pub struct ProjectConnection {
    database: Arc<dyn Database>,
    state: ProjectConnectionState,
    _operation: ProjectOperationLease,
}

enum ProjectConnectionState {
    Autocommit(Box<dyn DatabaseConnection>),
    Transaction(Box<dyn DatabaseTransaction>),
    Closed,
}

enum ProjectOperationLease {
    Shared { _lease: OwnedRwLockReadGuard<()> },
    Exclusive { _lease: OwnedRwLockWriteGuard<()> },
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

impl ProjectConnection {
    async fn open(
        database: Arc<dyn Database>,
        operation: ProjectOperationLease,
    ) -> Result<Self, String> {
        let connection = database
            .connect()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            database,
            state: ProjectConnectionState::Autocommit(connection),
            _operation: operation,
        })
    }

    pub fn discard(&mut self) {
        self.state = ProjectConnectionState::Closed;
    }

    pub async fn execute_query(
        &mut self,
        sql: impl AsRef<str>,
    ) -> Result<RecordBatchStream, String> {
        self.execute(DatabaseCommand::statement(sql.as_ref()))
            .await
            .and_then(DatabaseOutput::into_records)
            .map_err(|error| error.to_string())
    }

    pub async fn relation_schema(&mut self, table: &str) -> Result<Option<Schema>, String> {
        let relation = RelationName::parse(table).map_err(|error| error.to_string())?;
        self.table_schema(relation)
            .await
            .map(|schema| schema.map(|schema| schema.as_ref().clone()))
            .map_err(|error| error.to_string())
    }

    /// Enumerate the durable relations advertised by the active Spiral
    /// project. Temporary query-session relations are intentionally absent
    /// from the native project's catalog.
    pub(crate) async fn list_catalog_relations(&mut self) -> Result<Vec<CatalogRelation>, String> {
        let mut relations = self
            .relations()
            .await
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
    pub async fn catalog_relation_names(&mut self) -> Result<Vec<String>, String> {
        self.list_catalog_relations().await.map(|relations| {
            relations
                .into_iter()
                .map(|relation| relation.name)
                .collect()
        })
    }

    pub async fn begin_transaction(&mut self) -> Result<(), String> {
        let state = std::mem::replace(&mut self.state, ProjectConnectionState::Closed);
        let connection = match state {
            ProjectConnectionState::Autocommit(connection) => connection,
            ProjectConnectionState::Transaction(transaction) => {
                self.state = ProjectConnectionState::Transaction(transaction);
                return Err("database connection is already in a transaction".to_owned());
            }
            ProjectConnectionState::Closed => {
                return Err("database connection is closed".to_owned());
            }
        };
        self.state = ProjectConnectionState::Transaction(
            connection
                .begin()
                .await
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    pub async fn commit(&mut self) -> Result<(), String> {
        self.complete_transaction(true).await
    }

    pub async fn rollback(&mut self) -> Result<(), String> {
        self.complete_transaction(false).await
    }

    pub(crate) async fn reconnect(&mut self) -> Result<(), String> {
        if !matches!(self.state, ProjectConnectionState::Closed) {
            return Err("database connection is already open".to_owned());
        }
        let connection = self
            .database
            .connect()
            .await
            .map_err(|error| error.to_string())?;
        self.state = ProjectConnectionState::Autocommit(connection);
        Ok(())
    }

    async fn complete_transaction(&mut self, commit: bool) -> Result<(), String> {
        let state = std::mem::replace(&mut self.state, ProjectConnectionState::Closed);
        let ProjectConnectionState::Transaction(transaction) = state else {
            self.state = state;
            return Err("database connection has no active transaction".to_owned());
        };
        if commit {
            transaction
                .commit()
                .await
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .rollback()
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl DatabaseSession for ProjectConnection {
    fn execute(&mut self, command: DatabaseCommand) -> DatabaseFuture<'_, DatabaseOutput> {
        match &mut self.state {
            ProjectConnectionState::Autocommit(connection) => connection.execute(command),
            ProjectConnectionState::Transaction(transaction) => transaction.execute(command),
            ProjectConnectionState::Closed => async {
                Err(DatabaseError::invalid_state(
                    "database connection is closed",
                ))
            }
            .boxed(),
        }
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        match &mut self.state {
            ProjectConnectionState::Autocommit(connection) => connection.table_schema(relation),
            ProjectConnectionState::Transaction(transaction) => transaction.table_schema(relation),
            ProjectConnectionState::Closed => async {
                Err(DatabaseError::invalid_state(
                    "database connection is closed",
                ))
            }
            .boxed(),
        }
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<crate::RelationMetadata>> {
        match &mut self.state {
            ProjectConnectionState::Autocommit(connection) => connection.relations(),
            ProjectConnectionState::Transaction(transaction) => transaction.relations(),
            ProjectConnectionState::Closed => async {
                Err(DatabaseError::invalid_state(
                    "database connection is closed",
                ))
            }
            .boxed(),
        }
    }
}

pub struct ProjectStore {
    connection_settings: Arc<RwLock<SpiralConnectionSettings>>,
    namespace: String,
    database: ProjectDatabase,
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
        *self.connection_settings.write_unpoisoned() = connection_settings;
        Ok(())
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub async fn connect(&self) -> Result<ProjectConnection, String> {
        let operation = self.async_operation_gate.clone().read_owned().await;
        self.open_connection(ProjectOperationLease::Shared { _lease: operation })
            .await
    }

    pub async fn connect_exclusive(&self) -> Result<ProjectConnection, String> {
        let operation = self.async_operation_gate.clone().write_owned().await;
        self.open_connection(ProjectOperationLease::Exclusive { _lease: operation })
            .await
    }

    async fn open_connection(
        &self,
        operation: ProjectOperationLease,
    ) -> Result<ProjectConnection, String> {
        let endpoint = {
            let settings = self.connection_settings.read_unpoisoned();
            settings.project_id().ok_or_else(|| {
                "Spiral project is not configured. Choose a project in Settings → SpiralDB."
                    .to_owned()
            })?;
            settings.endpoint().to_owned()
        };
        let database = self.database.interface();
        ProjectConnection::open(database, operation)
            .await
            .map_err(|error| Self::present_error(&endpoint, error))
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
            let error = project_store.connect().await.err().unwrap();
            assert!(error.contains("Settings"));
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
