//! The project-scoped persistence boundary used by Studio.
//!
//! Store facades for robots, episodes, training, and project bundles all use
//! the same async database boundary. Nothing below this module falls back to
//! a filesystem or a second database.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::RecordBatch;
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _};

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream,
    RelationName, SqlStatement,
};
use crate::LEGACY_PROJECT_ERROR;
use crate::settings::{SpiralConnectionSettings, SpiralCredentials};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

use super::schema::{self as spiral, Table, column as db};
use super::sql::SpiralSql;
use crate::transfer::ManagedRelationCatalog;

const SPIRAL_UNAVAILABLE_PREFIX: &str = "SpiralDB is unavailable at ";
const BLOB_EXTENSION_NAME: &str = "spql.io.Blob";

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
        self.list_catalog_relations().await.map(|relations| {
            relations
                .into_iter()
                .map(|relation| relation.name)
                .collect()
        })
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

    #[cfg(any(test, feature = "test-support"))]
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
        namespace: &str,
        table: Table,
    ) -> Result<(), String> {
        let batch = RecordBatch::new_empty(Arc::new(ManagedRelationCatalog::schema(table)));
        let target = table.qualified_name(namespace);
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

    /// Ensure the durable root relation that acts as the managed namespace.
    pub async fn ensure_dataset_namespace(&mut self, dataset: &str) -> Result<(), String> {
        match self.relation_schema(dataset).await {
            Ok(Some(schema))
                if schema
                    .field_with_name(spiral::DATASET_NAMESPACE_MARKER)
                    .is_ok() =>
            {
                return Ok(());
            }
            Ok(Some(_)) => {
                return Err(format!(
                    "SpiralDB namespace {dataset} exists but is not managed by Spiral Robotics; choose a new namespace or remove that resource"
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(format!("checking SpiralDB namespace {dataset}: {error}"));
            }
        }
        self.execute_update_sql(&SpiralSql::create_dataset_namespace(dataset))
            .await
            .map_err(|error| format!("creating SpiralDB namespace {dataset}: {error}"))
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
    namespace: String,
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
        namespace: String,
        database: Arc<dyn Database>,
    ) -> Result<Self, String> {
        if !super::schema::StorageSchema::is_identifier(&namespace) {
            return Err("Spiral namespace is not a safe SQL identifier".to_owned());
        }
        Ok(Self {
            connection_settings,
            namespace,
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
        let relation = RelationName::parse(&self.namespace).map_err(|error| error.to_string())?;
        let mut connection = self.connect().await?;
        let _ = connection
            .table_schema(relation)
            .await
            .map_err(|error| error.to_string())?;
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
            let database = Arc::new(crate::SpiralDatabase::new(settings.clone()));
            ProjectStore::new(settings, "spiral_robotics".to_owned(), database)
        }
        .unwrap();
        let second = {
            let settings = initial.clone().with_project("project_b").unwrap();
            let database = Arc::new(crate::SpiralDatabase::new(settings.clone()));
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
            let database = Arc::new(crate::SpiralDatabase::new(settings.clone()));
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
        ManagedRelationProvisioner::new(&mut connection, self.project_store.namespace())
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
            ManagedRelationProvisioner::new(&mut connection, self.project_store.namespace())
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
                provisioning.describe(self.project_store.namespace())
            ),
        )
    }
}

struct ManagedRelationProvisioning {
    namespace_created: bool,
    created_relations: Vec<Table>,
}

impl ManagedRelationProvisioning {
    fn describe(&self, namespace: &str) -> String {
        let count = self.created_relations.len();
        if count == 0 {
            if self.namespace_created {
                return format!(
                    "created namespace {namespace} · all {} managed relations present",
                    ManagedRelationCatalog::tables().len()
                );
            }
            return format!(
                "all {} managed relations present",
                ManagedRelationCatalog::tables().len()
            );
        }
        let relations = self
            .created_relations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        if self.namespace_created {
            format!("created namespace {namespace} plus {count} missing relations: {relations}")
        } else {
            format!("created {count} missing relations: {relations}")
        }
    }
}

struct ManagedRelationProvisioner<'a> {
    connection: &'a mut ProjectConnection,
    namespace: &'a str,
}

impl<'a> ManagedRelationProvisioner<'a> {
    const fn new(connection: &'a mut ProjectConnection, namespace: &'a str) -> Self {
        Self {
            connection,
            namespace,
        }
    }

    async fn provision(mut self) -> Result<ManagedRelationProvisioning, String> {
        let namespace_created = self.ensure_namespace().await?;
        let created_relations = self.relation_readiness().await?;
        for table in &created_relations {
            self.connection
                .create_managed_relation(self.namespace, *table)
                .await?;
        }
        self.connection
            .ensure_video_assets_blob_relation(self.namespace)
            .await?;
        Ok(ManagedRelationProvisioning {
            namespace_created,
            created_relations,
        })
    }

    async fn verify(mut self) -> Result<(), String> {
        self.verify_namespace().await?;
        let missing = self.relation_readiness().await?;
        if missing.is_empty() {
            self.connection
                .ensure_video_assets_blob_relation(self.namespace)
                .await?;
            return Ok(());
        }
        let relations = missing
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "SpiralDB namespace {} is missing managed relations: {relations}. Run Settings → SpiralDB → Test connection to provision them.",
            self.namespace
        ))
    }

    async fn ensure_namespace(&mut self) -> Result<bool, String> {
        let existed = self.namespace_is_ready().await?;
        if !existed {
            self.connection
                .ensure_dataset_namespace(self.namespace)
                .await?;
        }
        Ok(!existed)
    }

    async fn verify_namespace(&mut self) -> Result<(), String> {
        if self.namespace_is_ready().await? {
            return Ok(());
        }
        Err(format!(
            "SpiralDB namespace {} is missing. Run Settings → SpiralDB → Test connection to provision it and its {} managed relations.",
            self.namespace,
            ManagedRelationCatalog::tables().len(),
        ))
    }

    async fn namespace_is_ready(&mut self) -> Result<bool, String> {
        match self.connection.relation_schema(self.namespace).await {
            Ok(Some(schema))
                if schema
                    .field_with_name(spiral::DATASET_NAMESPACE_MARKER)
                    .is_ok() =>
            {
                Ok(true)
            }
            Ok(Some(_)) => Err(format!(
                "SpiralDB namespace {} exists but is not managed by Spiral Robotics; choose a new namespace or remove that resource",
                self.namespace
            )),
            Ok(None) => Ok(false),
            Err(error) => Err(format!(
                "checking SpiralDB namespace {}: {error}",
                self.namespace
            )),
        }
    }

    async fn relation_readiness(&mut self) -> Result<Vec<Table>, String> {
        let mut missing = Vec::new();
        for table in ManagedRelationCatalog::tables() {
            let relation = table.qualified_name(self.namespace);
            match self.connection.relation_schema(&relation).await {
                Ok(Some(schema)) => {
                    Self::validate_existing_relation(table, &relation, &schema)?;
                }
                Ok(None) => missing.push(table),
                Err(error) => {
                    return Err(format!("checking Spiral table {relation}: {error}"));
                }
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
        let schema = Self::portable_validation_schema(table, schema);
        ManagedRelationCatalog::validate(table, &schema, "SpiralDB destination")
            .map_err(|error| {
                format!(
                    "{error}; existing managed relation {relation} was not modified. Choose a fresh namespace or remove that relation, then run Settings → SpiralDB → Test connection."
                )
            })
    }

    fn is_known_legacy_schema(table: Table, schema: &Schema) -> bool {
        match table {
            Table::Episodes => schema.field_with_name("metadata_json").is_ok(),
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

    /// A portable project bundle stores video payloads as Arrow Binary, while
    /// a ready SpiralDB relation exposes the same field as its native Blob
    /// extension. Normalize only that declared semantic variance before using
    /// the shared portable-schema validator.
    fn portable_validation_schema(table: Table, schema: &Schema) -> Schema {
        if table != Table::VideoAssets {
            return schema.clone();
        }
        let expected = ManagedRelationCatalog::schema(table);
        let mut fields = schema
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        for field in &mut fields {
            let native_blob = field.name() == db::PAYLOAD
                && matches!(field.data_type(), DataType::Struct(_))
                && field
                    .metadata()
                    .get(EXTENSION_TYPE_NAME_KEY)
                    .is_some_and(|name| name == BLOB_EXTENSION_NAME);
            if native_blob {
                let expected_payload = expected
                    .field_with_name(db::PAYLOAD)
                    .expect("the managed video schema declares its payload");
                *field = Field::new(
                    expected_payload.name(),
                    expected_payload.data_type().clone(),
                    field.is_nullable(),
                );
            }
        }
        Schema::new_with_metadata(fields, schema.metadata().clone())
    }
}

#[cfg(test)]
mod managed_relation_tests {
    use super::*;
    use crate::episodes::EpisodeRelations;

    #[test]
    fn provisioning_summary_reports_present_and_created_relations() {
        let present = ManagedRelationProvisioning {
            namespace_created: false,
            created_relations: Vec::new(),
        };
        assert_eq!(
            present.describe("spiral_robotics"),
            "all 14 managed relations present"
        );

        let created = ManagedRelationProvisioning {
            namespace_created: true,
            created_relations: vec![Table::Datasets, Table::Episodes],
        };
        assert_eq!(
            created.describe("spiral_robotics"),
            "created namespace spiral_robotics plus 2 missing relations: datasets, episodes"
        );
    }

    /// The two relations whose native column types differ from the portable
    /// shape a bundle carries still have to satisfy readiness validation and
    /// agree on their column names.
    #[test]
    fn readiness_accepts_the_declared_native_schema_variances() {
        for (table, schema) in [
            (
                Table::TrainingCheckpoints,
                Arc::new(ManagedRelationCatalog::schema(Table::TrainingCheckpoints)),
            ),
            (Table::VideoAssets, EpisodeRelations::native_video_schema()),
        ] {
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
    fn readiness_accepts_legacy_video_binary_before_blob_migration() {
        let table = Table::VideoAssets;
        ManagedRelationProvisioner::validate_existing_relation(
            table,
            &table.qualified_name("spiral_robotics"),
            &ManagedRelationCatalog::schema(table),
        )
        .expect("portable Binary remains valid input to the readiness migration");
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
            (
                Table::Episodes,
                Schema::new(vec![Field::new("metadata_json", DataType::Utf8, false)]),
            ),
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
