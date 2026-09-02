//! Native async Spiral implementation of the database traits.

use std::fmt;
use std::sync::Arc;

use arrow_array::cast::AsArray as _;
use arrow_array::{ArrayRef as ArrowArrayRef, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use parking_lot::RwLock;
use spiral_client::SpiralClientOptions;
use spiral_client::authn::ClientCredentials;
use spiral_client::catalog_client::lazy_catalog_client_with_client;
use spiral_client::lazy_client::LazySpiralClient;
use spiral_config::{ClientSettings, FragmentsCacheEnabled, MutableConfig as _};
use spiral_defaults::default_session_with_config;
use spiral_engine::catalog::{
    CatalogEntryRequest, RelationName as SpiralRelationName, RelationType,
};
use spiral_engine::exec::QueryCtx;
use spiral_engine::logical::{
    LogicalPlan, Source as LogicalSource, SourceProvider, SourceProviderRef, SourceScanPushdown,
    SourceScanRequest,
};
use spiral_engine::physical::{PhysicalNode, PhysicalNodeRef, PhysicalProps, Source};
use spiral_engine::relation::Relation;
use spiral_engine::sql::{CommandOutput, ConnectionCtx};
use spiral_engine::statement::CreateTable;
use spiral_engine::{EngineRuntime, Morsel, MorselSequence, RelationMeta};
use spiral_session::SpiralSession;
use spiraldb_control_plane::SpiralDbConnection;
use spiraldb_table::{SpiralDbConnectionServices, SpiralDbPlugin};
use tokio::sync::Mutex;
use url::Url;
use vortex_array::{ArrayRef, VortexSessionExecute as _};
use vortex_arrow::ArrowSessionExt as _;
use vortex_error::{VortexResult, vortex_bail, vortex_err};

use super::database::{
    BatchIngestion, Database, DatabaseCommand, DatabaseConnection, DatabaseError,
    DatabaseErrorKind, DatabaseFuture, DatabaseOutput, DatabaseResult, DatabaseSession,
    DatabaseTransaction, ExecutionSummary, IngestMode, RecordBatchStream, RelationKind,
    RelationMetadata, RelationName,
};
use crate::settings::SpiralConnectionSettings;

const QUERY_BATCH_CAPACITY: usize = 16;
const SPIRALDB_CATALOG: &str = "spiraldb";

/// Configured database entry point held by application state.
pub(super) struct SpiralDatabase {
    configured_session: RwLock<ConfiguredSpiralSession>,
}

impl SpiralDatabase {
    pub(super) fn new(settings: SpiralConnectionSettings) -> DatabaseResult<Self> {
        Ok(Self {
            configured_session: RwLock::new(ConfiguredSpiralSession::new(&settings)?),
        })
    }

    pub(super) fn configure(&self, settings: SpiralConnectionSettings) -> DatabaseResult<()> {
        let configured_session = ConfiguredSpiralSession::new(&settings)?;
        *self.configured_session.write() = configured_session;
        Ok(())
    }
}

#[derive(Clone)]
struct ConfiguredSpiralSession {
    session: SpiralSession,
    client_options: SpiralClientOptions,
    spfs_endpoint: Url,
    project: Option<String>,
}

impl ConfiguredSpiralSession {
    fn new(settings: &SpiralConnectionSettings) -> DatabaseResult<Self> {
        let mut client_settings = ClientSettings::load().map_err(DatabaseError::from_external)?;
        client_settings
            .set::<FragmentsCacheEnabled>(true)
            .map_err(DatabaseError::from_external)?;
        let api_url = Url::parse(settings.endpoint()).map_err(DatabaseError::from_external)?;
        let spfs_url =
            Url::parse(settings.spfs_endpoint()).map_err(DatabaseError::from_external)?;
        let mut client_options = SpiralClientOptions::new(client_settings)
            .with_api_url(api_url)
            .with_spfs_url(spfs_url.clone());
        if let Some(credentials) = settings.credentials() {
            client_options = client_options.with_client_credentials(ClientCredentials::new(
                credentials.client_id().to_owned(),
                credentials.client_secret().to_owned(),
            ));
        }
        let session = default_session_with_config(client_options.config().clone())
            .map_err(DatabaseError::from_external)?;
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let runtime = EngineRuntime::current(workers).map_err(DatabaseError::from_external)?;
        Ok(Self {
            session: runtime.install_io(session),
            client_options,
            spfs_endpoint: spfs_url,
            project: settings.project_id().map(str::to_owned),
        })
    }

    fn open_connection(&self) -> DatabaseResult<ConnectionCtx> {
        let client = LazySpiralClient::with_options(self.client_options.clone());
        let project_catalog = Arc::new(lazy_catalog_client_with_client(
            &self.session,
            client.clone(),
        ));
        let control_plane = SpiralDbConnection::new(Arc::new(client.clone()));
        let legacy: Arc<dyn spiraldb_catalog::LegacyResolverClient> = Arc::new(client);
        let connection = SpiralDbPlugin::default()
            .open_connection(
                &self.session,
                SpiralDbConnectionServices::new(
                    control_plane,
                    project_catalog,
                    legacy,
                    self.spfs_endpoint.clone(),
                ),
            )
            .map_err(DatabaseError::from_external)?;
        connection
            .relation_catalog_mut()
            .use_catalog(SPIRALDB_CATALOG)
            .map_err(DatabaseError::from_external)?;
        if let Some(project) = &self.project {
            connection
                .relation_catalog_mut()
                .use_project(project.clone());
        }
        Ok(connection)
    }
}

impl Database for SpiralDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            let connection = self.configured_session.read().open_connection()?;
            Ok(Box::new(SpiralConnection::new(connection)) as Box<dyn DatabaseConnection>)
        }
        .boxed()
    }
}

struct SpiralConnection {
    context: ConnectionCtx,
}

impl SpiralConnection {
    fn new(context: ConnectionCtx) -> Self {
        Self { context }
    }

    async fn execute_command(
        &mut self,
        command: DatabaseCommand,
    ) -> DatabaseResult<DatabaseOutput> {
        match command {
            DatabaseCommand::Statement(statement) => {
                match self
                    .context
                    .execute_command(statement.as_str())
                    .await
                    .map_err(DatabaseError::from_external)?
                {
                    CommandOutput::Statement { .. } => {
                        Ok(DatabaseOutput::AffectedRows(ExecutionSummary))
                    }
                    CommandOutput::Query { relation, options } => {
                        Self::query_output(relation, options).await
                    }
                    CommandOutput::Report(report) => {
                        let rendered = report
                            .execute()
                            .await
                            .map_err(DatabaseError::from_external)?
                            .to_string();
                        Ok(DatabaseOutput::Records(Self::report_stream(rendered)?))
                    }
                }
            }
            DatabaseCommand::Ingest(ingestion) => {
                self.ingest(ingestion).await?;
                Ok(DatabaseOutput::AffectedRows(ExecutionSummary))
            }
        }
    }

    async fn query_output(
        relation: Relation,
        options: spiral_engine::QueryOptions,
    ) -> DatabaseResult<DatabaseOutput> {
        let session = relation.session().clone();
        let schema = Arc::new(
            session
                .vortex()
                .arrow()
                .to_arrow_schema(
                    relation
                        .logical_plan()
                        .map_err(DatabaseError::from_external)?
                        .meta()
                        .dtype(),
                )
                .map_err(DatabaseError::from_external)?,
        );
        let morsels = relation
            .collect_stream_with_options(QUERY_BATCH_CAPACITY, options)
            .await
            .map_err(DatabaseError::from_external)?;
        let handle = session.handle();
        let batches = morsels
            .map(move |result| {
                let session = session.clone();
                handle.spawn_cpu(move || {
                    let morsel = result.map_err(DatabaseError::from_external)?;
                    Self::record_batch(&session, morsel.into_parts().0)
                })
            })
            .buffered(QUERY_BATCH_CAPACITY)
            .boxed();
        Ok(DatabaseOutput::Records(RecordBatchStream::new(
            schema, batches,
        )))
    }

    fn record_batch(session: &SpiralSession, array: ArrayRef) -> DatabaseResult<RecordBatch> {
        let schema = session
            .vortex()
            .arrow()
            .to_arrow_schema(array.dtype())
            .map_err(DatabaseError::from_external)?;
        let target = Field::new_struct("", schema.fields().clone(), false);
        let mut context = session.vortex().create_execution_ctx();
        let arrow = session
            .vortex()
            .arrow()
            .execute_arrow(array, Some(&target), &mut context)
            .map_err(DatabaseError::from_external)?;
        Ok(RecordBatch::from(arrow.as_struct().clone()))
    }

    fn report_stream(report: String) -> DatabaseResult<RecordBatchStream> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "report",
            DataType::Utf8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(StringArray::from(vec![report])) as ArrowArrayRef],
        )
        .map_err(DatabaseError::from_external)?;
        Ok(RecordBatchStream::new(
            schema,
            futures::stream::once(async move { Ok(batch) }).boxed(),
        ))
    }

    async fn fetch_table_schema(
        &mut self,
        relation: RelationName,
    ) -> DatabaseResult<Option<Arc<Schema>>> {
        let target = Self::relation_name(&relation)?;
        let catalog = self.context.relation_catalog().clone();
        let session = self.context.session().clone();
        catalog
            .bind_relation_opt(&target, &session)
            .await
            .map_err(DatabaseError::from_external)?
            .map(|definition| {
                session
                    .vortex()
                    .arrow()
                    .to_arrow_schema(definition.definition().dtype())
                    .map(Arc::new)
                    .map_err(DatabaseError::from_external)
            })
            .transpose()
    }

    async fn list_relations(&mut self) -> DatabaseResult<Vec<RelationMetadata>> {
        let request = CatalogEntryRequest::default()
            .with_relation_types(vec![RelationType::Table, RelationType::View]);
        let catalog = self.context.relation_catalog().clone();
        let mut entries = catalog
            .list_entries(self.context.session(), request)
            .map_err(DatabaseError::from_external)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(DatabaseError::from_external)?;
        entries.sort_by_key(|entry| entry.name().dotted().to_string());
        entries
            .into_iter()
            .map(|entry| {
                let name = RelationName::parse(entry.name().dotted().to_string())?;
                let kind = match entry.relation_type() {
                    RelationType::Table => RelationKind::Table,
                    RelationType::View => RelationKind::View,
                };
                Ok(RelationMetadata::new(name, kind))
            })
            .collect()
    }

    async fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseResult<()> {
        let (relation, mode, batches) = ingestion.into_parts();
        let target = Self::relation_name(&relation)?;
        let session = self.context.session().clone();
        let catalog = self.context.relation_catalog().clone();
        let exists = catalog
            .bind_relation_opt(&target, &session)
            .await
            .map_err(DatabaseError::from_external)?
            .is_some();
        if matches!(mode, IngestMode::Append | IngestMode::ReplaceExisting) && !exists {
            return Err(DatabaseError::new(
                DatabaseErrorKind::NotFound,
                format!("relation {:?} does not exist", relation.as_str()),
            ));
        }
        let plan = ArrowStreamSource::plan(session.clone(), batches)?;
        let options = self.context.query_options();
        match mode {
            IngestMode::Append => {
                let insert = Relation::new(session, plan)
                    .insert_into(target)
                    .map_err(DatabaseError::from_external)?;
                self.context
                    .execute_insert(&insert, options)
                    .await
                    .map(|_| ())
                    .map_err(DatabaseError::from_external)
            }
            IngestMode::CreateOrAppend if exists => {
                let insert = Relation::new(session, plan)
                    .insert_into(target)
                    .map_err(DatabaseError::from_external)?;
                self.context
                    .execute_insert(&insert, options)
                    .await
                    .map(|_| ())
                    .map_err(DatabaseError::from_external)
            }
            IngestMode::CreateOrAppend => {
                self.create_table(TableCreation::new(target, plan, false), options)
                    .await
            }
            IngestMode::ReplaceExisting | IngestMode::ReplaceOrCreate => {
                self.create_table(TableCreation::new(target, plan, true), options)
                    .await
            }
        }
    }

    async fn create_table(
        &self,
        creation: TableCreation,
        options: spiral_engine::QueryOptions,
    ) -> DatabaseResult<()> {
        let (target, plan, replace) = creation.into_parts();
        let create = CreateTable::new(target, plan, [], false, replace, false);
        self.context
            .execute_create_table(&create, options)
            .await
            .map(|_| ())
            .map_err(DatabaseError::from_external)
    }

    fn relation_name(relation: &RelationName) -> DatabaseResult<SpiralRelationName> {
        Ok(SpiralRelationName::new(
            relation.as_str().split('.').map(str::to_owned),
        ))
    }
}

impl DatabaseSession for SpiralConnection {
    fn execute(&mut self, command: DatabaseCommand) -> DatabaseFuture<'_, DatabaseOutput> {
        async move { self.execute_command(command).await }.boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move { self.fetch_table_schema(relation).await }.boxed()
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        async move { self.list_relations().await }.boxed()
    }
}

impl DatabaseConnection for SpiralConnection {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move {
            let connection = *self;
            connection
                .context
                .set_autocommit(false)
                .await
                .map_err(DatabaseError::from_external)?;
            Ok(Box::new(SpiralTransaction { connection }) as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}

struct SpiralTransaction {
    connection: SpiralConnection,
}

impl DatabaseSession for SpiralTransaction {
    fn execute(&mut self, command: DatabaseCommand) -> DatabaseFuture<'_, DatabaseOutput> {
        self.connection.execute(command)
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        self.connection.table_schema(relation)
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        self.connection.relations()
    }
}

impl DatabaseTransaction for SpiralTransaction {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        async move {
            self.connection
                .context
                .commit()
                .await
                .map(|_| ExecutionSummary)
                .map_err(DatabaseError::from_external)
        }
        .boxed()
    }

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async move {
            self.connection
                .context
                .rollback()
                .await
                .map_err(DatabaseError::from_external)
        }
        .boxed()
    }
}

struct TableCreation {
    target: SpiralRelationName,
    plan: LogicalPlan,
    replace: bool,
}

impl TableCreation {
    fn new(target: SpiralRelationName, plan: LogicalPlan, replace: bool) -> Self {
        Self {
            target,
            plan,
            replace,
        }
    }

    fn into_parts(self) -> (SpiralRelationName, LogicalPlan, bool) {
        (self.target, self.plan, self.replace)
    }
}

impl DatabaseError {
    fn from_external(error: impl fmt::Display) -> Self {
        let message = error.to_string();
        let normalized = message.to_ascii_lowercase();
        let kind = if [
            "connection refused",
            "failed to connect",
            "error sending request",
            "tcp connect error",
            "dns error",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
        {
            DatabaseErrorKind::Unavailable
        } else {
            DatabaseErrorKind::Internal
        };
        Self::new(kind, message)
    }
}

struct ArrowStreamSource {
    session: SpiralSession,
    schema: Arc<Schema>,
    metadata: RelationMeta,
    batches: Arc<Mutex<Option<RecordBatchStream>>>,
}

impl ArrowStreamSource {
    fn plan(session: SpiralSession, batches: RecordBatchStream) -> DatabaseResult<LogicalPlan> {
        let schema = Arc::clone(batches.schema());
        let dtype = session
            .vortex()
            .arrow()
            .from_arrow_schema(schema.as_ref())
            .map_err(DatabaseError::from_external)?;
        let provider: SourceProviderRef = Arc::new(Self {
            session,
            schema,
            metadata: RelationMeta::unordered(dtype),
            batches: Arc::new(Mutex::new(Some(batches))),
        });
        Ok(LogicalPlan::source(
            LogicalSource::new(provider).map_err(DatabaseError::from_external)?,
        ))
    }
}

impl fmt::Debug for ArrowStreamSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArrowStreamSource")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl SourceProvider for ArrowStreamSource {
    fn name(&self) -> &str {
        "robotics_arrow_stream"
    }

    fn base_meta(&self) -> RelationMeta {
        self.metadata.clone()
    }

    fn base_dtype(&self) -> &vortex_array::dtype::DType {
        self.metadata.dtype()
    }

    fn scan(&self, request: SourceScanRequest<'_>) -> VortexResult<PhysicalNodeRef> {
        request.wrap_residuals(
            Arc::new(ArrowStreamNode {
                session: self.session.clone(),
                schema: Arc::clone(&self.schema),
                metadata: self.metadata.clone(),
                properties: PhysicalProps::default(),
                batches: Arc::clone(&self.batches),
            }),
            SourceScanPushdown::none(),
        )
    }
}

struct ArrowStreamNode {
    session: SpiralSession,
    schema: Arc<Schema>,
    metadata: RelationMeta,
    properties: PhysicalProps,
    batches: Arc<Mutex<Option<RecordBatchStream>>>,
}

impl fmt::Debug for ArrowStreamNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArrowStreamNode")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

struct ArrowStreamGlobal {
    state: Mutex<ArrowStreamState>,
}

struct ArrowStreamState {
    batches: RecordBatchStream,
    next_sequence: u64,
}

impl PhysicalNode for ArrowStreamNode {
    type Global = ArrowStreamGlobal;

    fn name(&self) -> &str {
        "robotics_arrow_stream"
    }

    fn meta(&self) -> &RelationMeta {
        &self.metadata
    }

    fn children(&self) -> &[PhysicalNodeRef] {
        &[]
    }

    fn output_props(&self) -> &PhysicalProps {
        &self.properties
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<PhysicalNodeRef>,
    ) -> VortexResult<PhysicalNodeRef> {
        if !children.is_empty() {
            vortex_bail!("Arrow input stream is a leaf node");
        }
        Ok(self)
    }

    async fn init_global(&self, _context: &QueryCtx) -> VortexResult<Self::Global> {
        let batches = self
            .batches
            .try_lock()
            .map_err(|_| vortex_err!("Arrow input stream is already being initialized"))?
            .take()
            .ok_or_else(|| vortex_err!("Arrow input stream was already consumed"))?;
        Ok(ArrowStreamGlobal {
            state: Mutex::new(ArrowStreamState {
                batches,
                next_sequence: 0,
            }),
        })
    }

    fn runtime_source_factory(
        self: Arc<Self>,
    ) -> Option<Arc<dyn spiral_engine::exec::RuntimeSourceFactory>> {
        Some(spiral_engine::exec::typed_source_factory(self))
    }
}

impl Source for ArrowStreamNode {
    type Local = ();

    fn max_workers(&self, _global: &Self::Global) -> VortexResult<usize> {
        Ok(1)
    }

    fn init_local(&self, _global: &Self::Global, _context: &QueryCtx) -> VortexResult<Self::Local> {
        Ok(())
    }

    async fn next_morsel(
        &self,
        global: &Self::Global,
        _local: &mut Self::Local,
        _context: &QueryCtx,
    ) -> VortexResult<Option<Morsel>> {
        let mut state = global.state.lock().await;
        let Some(batch) = state.batches.next().await else {
            return Ok(None);
        };
        let batch = batch.map_err(|error| vortex_err!("{error}"))?;
        let arrow_struct = StructArray::from(batch);
        let arrow_array: ArrowArrayRef = Arc::new(arrow_struct);
        let field = Field::new_struct("", self.schema.fields().clone(), false);
        let array = self
            .session
            .vortex()
            .arrow()
            .from_arrow_array(arrow_array, &field)?;
        let sequence = MorselSequence::new(state.next_sequence);
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| vortex_err!("Arrow input stream work sequence overflowed u64"))?;
        Ok(Some(Morsel::new(array, sequence)))
    }
}
