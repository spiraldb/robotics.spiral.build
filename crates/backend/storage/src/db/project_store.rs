//! The project-scoped persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all use
//! the same async database boundary. Nothing below this module falls back to
//! a filesystem or a second database.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use futures::{FutureExt as _, StreamExt as _};

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream,
    RelationName, SqlStatement,
};
use crate::LEGACY_PROJECT_ERROR;
use crate::settings::{SpiralConnectionSettings, SpiralCredentials};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

use super::schema::{Table, column as db};
use crate::transfer::ManagedRelationCatalog;

const SPIRAL_UNAVAILABLE_PREFIX: &str = "SpiralDB is unavailable at ";

pub(crate) struct StorageClock;

impl StorageClock {
    pub(crate) fn unix_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(i64::MAX)
    }
}

/// One async, project-scoped logical connection.
pub struct ProjectConnection {
    database: Arc<dyn Database>,
    state: ProjectConnectionState,
    operation: Arc<ProjectOperationLease>,
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

impl ProjectConnection {
    async fn open(
        database: Arc<dyn Database>,
        operation: ProjectOperationLease,
    ) -> DatabaseResult<Self> {
        let connection = database.connect().await?;
        Ok(Self {
            database,
            state: ProjectConnectionState::Autocommit(connection),
            operation: Arc::new(operation),
        })
    }

    pub fn discard(&mut self) {
        self.state = ProjectConnectionState::Closed;
    }

    pub async fn execute_query(
        &mut self,
        sql: impl AsRef<str>,
    ) -> Result<RecordBatchStream, String> {
        self.query(sql.as_ref().into())
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn relation_schema(&mut self, table: &str) -> Result<Option<Schema>, String> {
        let relation = RelationName::parse(table).map_err(|error| error.to_string())?;
        self.table_schema(relation)
            .await
            .map(|schema| schema.map(|schema| schema.as_ref().clone()))
            .map_err(|error| error.to_string())
    }

    /// Read schemas concurrently on independent connections while retaining this
    /// operation's project lock. Never escape an explicit transaction's snapshot.
    pub(crate) async fn relation_schemas(
        &mut self,
        names: Vec<RelationName>,
    ) -> DatabaseResult<Vec<(RelationName, DatabaseResult<Option<Arc<Schema>>>)>> {
        if !matches!(self.state, ProjectConnectionState::Autocommit(_)) {
            return Err(DatabaseError::invalid_state(
                "parallel schema reads require an autocommit connection",
            ));
        }
        let database = self.database.clone();
        let operation = self.operation.clone();
        Ok(futures::stream::iter(names.into_iter().map(move |name| {
            let database = database.clone();
            let operation = operation.clone();
            async move {
                let _operation = operation;
                let schema = async {
                    let mut connection = database.connect().await?;
                    connection.table_schema(name.clone()).await
                }
                .await;
                (name, schema)
            }
        }))
        .buffered(8)
        .collect()
        .await)
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
    #[cfg(any(test, feature = "test-support"))]
    /// List durable relation names without exposing the driver's catalog
    /// representation across the storage boundary.
    pub async fn catalog_relation_names(&mut self) -> Result<Vec<String>, String> {
        let mut names = self
            .relations()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|entry| entry.into_parts().0.into_string())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        Ok(names)
    }
}

impl ProjectConnection {
    /// Materialize a whole bounded, storage-owned relation.
    pub(crate) async fn read_batches(&mut self, sql: &str) -> Result<Vec<RecordBatch>, String> {
        let mut records = self
            .query(sql.into())
            .await
            .map_err(|error| format!("reading Spiral rows: {error}"))?;
        let mut batches = Vec::new();
        while let Some(batch) = records.next().await {
            batches.push(batch.map_err(|error| format!("reading Spiral rows: {error}"))?);
        }
        Ok(batches)
    }

    pub(crate) async fn relation_rows<T>(
        &mut self,
        relation: &str,
        decode: fn(&RecordBatch, usize) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        let mut rows = Vec::new();
        for batch in self
            .read_batches(&format!("SELECT * FROM {relation}"))
            .await?
        {
            for row in 0..batch.num_rows() {
                rows.push(decode(&batch, row)?);
            }
        }
        Ok(rows)
    }

    /// Commit a mutation, or roll it back and keep both failures visible.
    pub(crate) async fn finish_transaction(
        &mut self,
        mutation: Result<(), String>,
        label: &str,
    ) -> Result<(), String> {
        match mutation {
            Ok(()) => self.commit().await,
            Err(error) => match self.rollback().await {
                Ok(()) => Err(error),
                Err(rollback) => {
                    self.discard();
                    Err(format!(
                        "{error}; rolling back {label} also failed: {rollback}; connection discarded"
                    ))
                }
            },
        }
    }

    pub async fn ingest_batch(&mut self, target: &str, batch: RecordBatch) -> Result<(), String> {
        self.ingest_batches(target, vec![batch]).await
    }

    pub(crate) async fn ingest_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::Append)
            .save(self)
            .await
    }

    pub async fn create_or_append_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::CreateOrAppend)
            .save(self)
            .await
    }

    /// Atomically replace one complete, already provisioned relation.
    pub async fn replace_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::ReplaceExisting)
            .save(self)
            .await
    }

    /// Replace one derived relation, creating it when necessary.
    pub async fn replace_or_create_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::ReplaceOrCreate)
            .save(self)
            .await
    }

    pub(crate) async fn create_managed_relation(
        &mut self,
        table_prefix: &str,
        table: Table,
    ) -> Result<(), String> {
        let batch = RecordBatch::new_empty(Arc::new(ManagedRelationCatalog::schema(table)));
        let target = table.qualified_name(table_prefix);
        BatchWrite::new(&target, vec![batch], BatchWriteMode::CreateOrAppend)
            .save(self)
            .await
    }

    pub async fn table_exists(&mut self, target: &str) -> Result<bool, String> {
        self.relation_schema(target)
            .await
            .map(|schema| schema.is_some())
            .map_err(|error| format!("checking Spiral table {target}: {error}"))
    }

    pub async fn execute_update_sql(&mut self, sql: &str) -> Result<(), String> {
        self.execute_update(sql.into())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy)]
enum BatchWriteMode {
    Append,
    CreateOrAppend,
    ReplaceExisting,
    ReplaceOrCreate,
}

struct BatchWrite<'a> {
    target: &'a str,
    batches: Vec<RecordBatch>,
    mode: BatchWriteMode,
}

impl<'a> BatchWrite<'a> {
    fn new(target: &'a str, batches: Vec<RecordBatch>, mode: BatchWriteMode) -> Self {
        Self {
            target,
            batches,
            mode,
        }
    }

    async fn save(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let Some(schema) = self.batches.first().map(RecordBatch::schema) else {
            return Ok(());
        };
        let relation = RelationName::parse(self.target)
            .map_err(|error| format!("configuring native ingest for {}: {error}", self.target))?;
        let mode = match self.mode {
            BatchWriteMode::Append => crate::IngestMode::Append,
            BatchWriteMode::CreateOrAppend => crate::IngestMode::CreateOrAppend,
            BatchWriteMode::ReplaceExisting => crate::IngestMode::ReplaceExisting,
            BatchWriteMode::ReplaceOrCreate => crate::IngestMode::ReplaceOrCreate,
        };
        let batches = futures::stream::iter(self.batches.into_iter().map(Ok)).boxed();
        let ingestion = crate::BatchIngestion::new(
            relation,
            mode,
            crate::RecordBatchStream::new(schema, batches),
        );
        connection.ingest(ingestion).await.map_err(|error| {
            if matches!(
                self.mode,
                BatchWriteMode::ReplaceExisting | BatchWriteMode::ReplaceOrCreate
            ) {
                format!("replacing Spiral table {}: {error}", self.target)
            } else if matches!(self.mode, BatchWriteMode::CreateOrAppend) {
                format!("creating Spiral table {}: {error}", self.target)
            } else {
                format!("ingesting Spiral table {}: {error}", self.target)
            }
        })?;
        Ok(())
    }
}

impl ProjectConnectionState {
    fn session(&mut self) -> Result<&mut dyn DatabaseSession, DatabaseError> {
        match self {
            Self::Autocommit(connection) => Ok(connection.as_mut()),
            Self::Transaction(transaction) => Ok(transaction.as_mut()),
            Self::Closed => Err(DatabaseError::invalid_state(
                "database connection is closed",
            )),
        }
    }
}

impl DatabaseSession for ProjectConnection {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move {
            let records = self.state.session()?.query(sql).await?;
            // The project lock must cover consumption, including streams returned
            // from helpers whose local ProjectConnection has already been dropped.
            Ok(records
                .retain(self.operation.clone())
                .retain(self.database.clone()))
        }
        .boxed()
    }

    fn execute_update(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        async move { self.state.session()?.execute_update(sql).await }.boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move { self.state.session()?.ingest(ingestion).await }.boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move { self.state.session()?.table_schema(relation).await }.boxed()
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<crate::RelationMetadata>> {
        async move { self.state.session()?.relations().await }.boxed()
    }
}

/// Storage for one immutable destination. Changing projects creates another store;
/// an existing request can never follow a later change to the active project.
pub struct ProjectStore {
    connection_settings: SpiralConnectionSettings,
    table_prefix: String,
    database: Arc<dyn Database>,
    async_operation_gate: Arc<tokio::sync::RwLock<()>>,
}

impl ProjectStore {
    /// Identify the stable operator-facing form so delivery adapters can use
    /// an availability status instead of misclassifying the failure as input.
    pub fn is_unavailable_error(message: &str) -> bool {
        message.starts_with(SPIRAL_UNAVAILABLE_PREFIX)
    }

    fn present_error(endpoint: &str, error: DatabaseError) -> String {
        if error.kind() != DatabaseErrorKind::Unavailable {
            return error.to_string();
        }
        tracing::warn!(spiral_endpoint = endpoint, detail = %error, "SpiralDB transport failed");
        format!(
            "{SPIRAL_UNAVAILABLE_PREFIX}{endpoint}. Start the service or update Settings → SpiralDB, then try again."
        )
    }

    pub fn new(
        connection_settings: SpiralConnectionSettings,
        table_prefix: String,
        database: Arc<dyn Database>,
    ) -> Result<Self, String> {
        if !super::schema::StorageSchema::is_identifier(&table_prefix) {
            return Err("Studio table prefix is not a safe SQL identifier".to_owned());
        }
        Ok(Self {
            connection_settings,
            table_prefix,
            database,
            async_operation_gate: Arc::new(tokio::sync::RwLock::new(())),
        })
    }

    pub(crate) fn project_id(&self) -> String {
        self.connection_settings
            .project_id()
            .unwrap_or_default()
            .to_owned()
    }

    pub(crate) fn connection_settings(&self) -> SpiralConnectionSettings {
        self.connection_settings.clone()
    }

    pub(crate) fn endpoint(&self) -> String {
        self.connection_settings.endpoint().to_owned()
    }

    pub(crate) fn spfs_endpoint(&self) -> String {
        self.connection_settings.spfs_endpoint().to_owned()
    }

    pub(crate) fn credentials(&self) -> Option<SpiralCredentials> {
        self.connection_settings.credentials().cloned()
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.connection_settings.project_id().is_some()
    }

    /// Initialize the configured database adapter and force one authenticated
    /// project-catalog lookup without mutating project data.
    pub(crate) async fn prewarm(&self) -> Result<(), String> {
        let relation =
            RelationName::parse(Table::StudioSettings.qualified_name(&self.table_prefix))
                .map_err(|error| error.to_string())?;
        let mut connection = self.connect().await?;
        let _ = connection
            .table_schema(relation)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn table_prefix(&self) -> &str {
        &self.table_prefix
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
            let settings = &self.connection_settings;
            settings.project_id().ok_or_else(|| {
                "Spiral project is not configured. Choose a project in Settings → SpiralDB."
                    .to_owned()
            })?;
            settings.endpoint().to_owned()
        };
        let database = Arc::clone(&self.database);
        ProjectConnection::open(database, operation)
            .await
            .map_err(|error| Self::present_error(&endpoint, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn separate_project_stores_keep_their_original_destinations() {
        let initial = SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
        )
        .unwrap();
        let first = {
            let settings = initial.clone().with_project("project_a").unwrap();
            let database = Arc::new(crate::AdbcDatabase::new(settings.clone()));
            ProjectStore::new(settings, "spiral_robotics".to_owned(), database)
        }
        .unwrap();
        let second = {
            let settings = initial.clone().with_project("project_b").unwrap();
            let database = Arc::new(crate::AdbcDatabase::new(settings.clone()));
            ProjectStore::new(settings, "spiral_robotics".to_owned(), database)
        }
        .unwrap();
        assert_eq!(first.project_id(), "project_a");
        assert_eq!(second.project_id(), "project_b");
        let _changed_copy = first
            .connection_settings()
            .with_project("project_c")
            .unwrap();
        assert_eq!(first.project_id(), "project_a");
        let unconfigured = {
            let settings = initial;
            let database = Arc::new(crate::AdbcDatabase::new(settings.clone()));
            ProjectStore::new(settings, "spiral_robotics".to_owned(), database)
        }
        .unwrap();
        assert!(!unconfigured.is_configured());
        assert!(
            unconfigured
                .connect()
                .await
                .err()
                .unwrap()
                .contains("Settings")
        );
    }

    #[test]
    fn transport_failures_become_actionable_without_rewriting_validation() {
        let unavailable = ProjectStore::present_error(
            "http://127.0.0.1:4279",
            DatabaseError::new(DatabaseErrorKind::Unavailable, "driver transport failed"),
        );
        assert_eq!(
            unavailable,
            "SpiralDB is unavailable at http://127.0.0.1:4279. Start the service or update Settings → SpiralDB, then try again."
        );
        assert!(ProjectStore::is_unavailable_error(&unavailable));

        let invalid = "episode bundle has the wrong format".to_owned();
        assert_eq!(
            ProjectStore::present_error(
                "http://127.0.0.1:4279",
                DatabaseError::invalid_input(invalid.clone())
            ),
            invalid
        );
    }
}

/// Connection configuration and managed-relation provisioning.
pub(crate) struct ProjectDatabasePersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> ProjectDatabasePersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn prewarm(&self) -> Result<(), String> {
        self.project_store.prewarm().await?;
        let mut connection = self.project_store.connect_exclusive().await?;
        ManagedRelationProvisioner::new(&mut connection, self.project_store.table_prefix())
            .verify()
            .await
    }

    pub(crate) async fn test(&self) -> (bool, String) {
        let endpoint = self.project_store.endpoint();
        let project = self.project_store.project_id();
        if project.is_empty() {
            return (
                false,
                "Choose a Spiral project in Settings before testing the connection.".to_owned(),
            );
        }
        let provisioning = async {
            let mut connection = self.project_store.connect_exclusive().await?;
            ManagedRelationProvisioner::new(&mut connection, self.project_store.table_prefix())
                .provision()
                .await
        }
        .await;
        let provisioning = match provisioning {
            Ok(provisioning) => provisioning,
            Err(error) => return (false, error),
        };
        (
            true,
            format!(
                "Connected to {endpoint} · project {project} · {}",
                provisioning.describe()
            ),
        )
    }
}

struct ManagedRelationProvisioning {
    created_relations: Vec<Table>,
}

impl ManagedRelationProvisioning {
    fn describe(&self) -> String {
        let count = self.created_relations.len();
        if count == 0 {
            return format!(
                "all {} managed relations present",
                ManagedRelationProvisioner::tables().len()
            );
        }
        let relations = self
            .created_relations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("created {count} missing relations: {relations}")
    }
}

struct ManagedRelationProvisioner<'a> {
    connection: &'a mut ProjectConnection,
    table_prefix: &'a str,
}

impl<'a> ManagedRelationProvisioner<'a> {
    fn tables() -> Vec<Table> {
        ManagedRelationCatalog::tables()
            .into_iter()
            .filter(|table| *table != Table::RecordingSources)
            .collect()
    }

    const fn new(connection: &'a mut ProjectConnection, table_prefix: &'a str) -> Self {
        Self {
            connection,
            table_prefix,
        }
    }

    async fn provision(mut self) -> Result<ManagedRelationProvisioning, String> {
        let created_relations = self.relation_readiness().await?;
        for table in &created_relations {
            self.connection
                .create_managed_relation(self.table_prefix, *table)
                .await?;
        }
        Ok(ManagedRelationProvisioning { created_relations })
    }

    async fn verify(mut self) -> Result<(), String> {
        let missing = self.relation_readiness().await?;
        if missing.is_empty() {
            return Ok(());
        }
        let relations = missing
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "Studio table prefix {} is missing managed relations: {relations}. Run Settings → SpiralDB → Test connection to provision them.",
            self.table_prefix
        ))
    }

    async fn relation_readiness(&mut self) -> Result<Vec<Table>, String> {
        let tables = Self::tables();
        let names = tables
            .iter()
            .map(|table| RelationName::parse(table.qualified_name(self.table_prefix)))
            .collect::<DatabaseResult<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        let schemas = self
            .connection
            .relation_schemas(names)
            .await
            .map_err(|error| error.to_string())?;
        let mut missing = Vec::new();
        for (table, (relation, schema)) in tables.into_iter().zip(schemas) {
            match schema.map_err(|error| {
                format!(
                    "checking Spiral table {relation}: {error}",
                    relation = relation.as_str()
                )
            })? {
                Some(schema) => {
                    Self::validate_existing_relation(table, relation.as_str(), &schema)?
                }
                None => missing.push(table),
            }
        }
        Ok(missing)
    }

    fn validate_existing_relation(
        table: Table,
        relation: &str,
        schema: &Schema,
    ) -> Result<(), String> {
        if Self::is_known_legacy_schema(table, schema) {
            return Err(LEGACY_PROJECT_ERROR.to_owned());
        }
        ManagedRelationCatalog::validate(table, schema, "SpiralDB destination")
            .map_err(|error| {
                format!(
                    "{error}; existing managed relation {relation} was not modified. Choose a fresh table prefix or remove that relation, then run Settings → SpiralDB → Test connection."
                )
            })
    }

    fn is_known_legacy_schema(table: Table, schema: &Schema) -> bool {
        match table {
            Table::TrainingRuns => {
                schema.field_with_name(db::SNAPSHOT_JSON).is_ok()
                    && schema.field_with_name(db::ORDINAL).is_err()
            }
            Table::RolloutRuns => {
                schema.field_with_name(db::SNAPSHOT_JSON).is_ok()
                    && schema.field_with_name(db::SESSION_ID).is_err()
            }
            Table::WorkflowRuns => {
                schema.fields().len() == 3 && schema.field_with_name(db::DOCUMENT_VERSION).is_err()
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod managed_relation_tests {
    use super::*;
    use arrow_schema::{DataType, Field};

    #[test]
    fn startup_provisions_application_state_without_recreating_recordings() {
        let tables = ManagedRelationProvisioner::tables();
        assert_eq!(tables.len(), 9);
        assert!(tables.contains(&Table::Robots));
        assert!(tables.contains(&Table::TrainingRuns));
        assert!(!tables.contains(&Table::RecordingSources));
    }

    #[test]
    fn provisioning_summary_reports_present_and_created_relations() {
        let present = ManagedRelationProvisioning {
            created_relations: Vec::new(),
        };
        assert_eq!(present.describe(), "all 9 managed relations present");

        let created = ManagedRelationProvisioning {
            created_relations: vec![Table::Robots, Table::StudioSettings],
        };
        assert_eq!(
            created.describe(),
            "created 2 missing relations: robots, studio_settings"
        );
    }

    /// The two relations whose native column types differ from the portable
    /// shape a bundle carries still have to satisfy readiness validation and
    /// agree on their column names.
    #[test]
    fn readiness_accepts_the_declared_native_schema_variances() {
        for (table, schema) in [(
            Table::TrainingCheckpoints,
            Arc::new(ManagedRelationCatalog::schema(Table::TrainingCheckpoints)),
        )] {
            let native = schema
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect::<Vec<_>>();
            let managed = ManagedRelationCatalog::schema(table)
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect::<Vec<_>>();
            assert_eq!(native, managed, "{table}");
            ManagedRelationProvisioner::validate_existing_relation(
                table,
                &table.qualified_name("spiral_robotics"),
                schema.as_ref(),
            )
            .unwrap_or_else(|error| panic!("{table}: {error}"));
        }
    }

    #[test]
    fn readiness_refuses_known_legacy_relation_shapes_with_project_guidance() {
        let legacy_document = || {
            Schema::new(vec![
                Field::new(db::RUN_ID, DataType::Utf8, false),
                Field::new(db::SNAPSHOT_JSON, DataType::Utf8, false),
                Field::new(db::UPDATED_MS, DataType::Int64, false),
            ])
        };
        let legacy_relations = [
            (Table::TrainingRuns, legacy_document()),
            (Table::RolloutRuns, legacy_document()),
            (Table::WorkflowRuns, legacy_document()),
        ];

        for (table, schema) in legacy_relations {
            let error = ManagedRelationProvisioner::validate_existing_relation(
                table,
                &table.qualified_name("spiral_robotics"),
                &schema,
            )
            .unwrap_err();
            assert_eq!(error, LEGACY_PROJECT_ERROR, "{table}");
        }
    }

    #[test]
    fn readiness_reports_actionable_schema_mismatches_without_modification() {
        let table = Table::StudioSettings;
        let relation = table.qualified_name("spiral_robotics");
        let mut fields = ManagedRelationCatalog::schema(table)
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[1] = Field::new(db::VALUE_JSON, DataType::UInt64, false);

        let error = ManagedRelationProvisioner::validate_existing_relation(
            table,
            &relation,
            &Schema::new(fields),
        )
        .unwrap_err();

        assert!(error.contains(&relation), "{error}");
        assert!(error.contains("was not modified"), "{error}");
        assert!(
            error.contains("Settings → SpiralDB → Test connection"),
            "{error}"
        );
        assert!(!error.contains(LEGACY_PROJECT_ERROR), "{error}");
    }
}

#[cfg(test)]
mod schema_tests;
