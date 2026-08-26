//! Native async Spiral implementation of the database traits.
//!
//! The public proxies are `Send`; every `ConnectionCtx` remains owned by one
//! Tokio `LocalSet` task. Spiral query pipelines are submitted back to the
//! caller's multithreaded Tokio runtime through `EngineRuntime::current`.

use std::fmt;
use std::sync::Arc;

use arrow_array::cast::AsArray as _;
use arrow_array::{ArrayRef as ArrowArrayRef, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use spiral_client::SpiralClientOptions;
use spiral_client::authn::ClientCredentials;
use spiral_config::ClientSettings;
use spiral_defaults::default_session_with_client_options;
use spiral_engine::catalog::{
    CatalogEntryRequest, RelationCatalogSessionExt as _, RelationName as SpiralRelationName,
    RelationType,
};
use spiral_engine::exec::QueryCtx;
use spiral_engine::logical::{
    LogicalPlan, Source as LogicalSource, SourceProvider, SourceProviderRef, SourceScanPushdown,
    SourceScanRequest,
};
use spiral_engine::physical::{DynSource, PhysicalNode, PhysicalNodeRef, PhysicalProps, Source};
use spiral_engine::relation::Relation;
use spiral_engine::sql::{CommandOutput, ConnectionCtx};
use spiral_engine::statement::CreateTable;
use spiral_engine::{EngineRuntime, Morsel, RelationMeta};
use spiral_session::SpiralSession;
use tokio::sync::{Mutex, mpsc, oneshot};
use url::Url;
use vortex_array::{ArrayRef, VortexSessionExecute as _};
use vortex_arrow::ArrowSessionExt as _;
use vortex_error::{VortexResult, vortex_bail, vortex_err};

use crate::database::{
    BatchIngestion, Database, DatabaseCommand, DatabaseConnection, DatabaseError,
    DatabaseErrorKind, DatabaseFuture, DatabaseOutput, DatabaseResult, DatabaseSession,
    DatabaseTransaction, ExecutionSummary, IngestMode, RecordBatchStream, RelationKind,
    RelationMetadata, RelationName,
};
use crate::settings::SpiralConnectionSettings;

const DATABASE_CHANNEL_CAPACITY: usize = 64;
const CONNECTION_CHANNEL_CAPACITY: usize = 16;
const QUERY_BATCH_CAPACITY: usize = 16;
const SPIRALDB_CATALOG: &str = "spiraldb";

/// `Send + Sync` entry point held by application state.
#[derive(Clone)]
pub(crate) struct SpiralDatabase {
    sender: mpsc::Sender<DatabaseServiceRequest>,
}

impl SpiralDatabase {
    /// Start the connection owner on the current [`tokio::task::LocalSet`].
    pub(crate) fn start(settings: SpiralConnectionSettings) -> DatabaseResult<Self> {
        let (sender, receiver) = mpsc::channel(DATABASE_CHANNEL_CAPACITY);
        let service = SpiralDatabaseService::new(settings, receiver)?;
        tokio::task::spawn_local(service.run());
        Ok(Self { sender })
    }

    pub(crate) fn configure(&self, settings: SpiralConnectionSettings) -> DatabaseFuture<'_, ()> {
        let sender = self.sender.clone();
        async move {
            let (response, reply) = oneshot::channel();
            sender
                .send(DatabaseServiceRequest::Configure { settings, response })
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)?
        }
        .boxed()
    }

    fn request_connection(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        let sender = self.sender.clone();
        async move {
            let (response, reply) = oneshot::channel();
            sender
                .send(DatabaseServiceRequest::Open(OpenConnectionRequest {
                    response,
                }))
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)?
        }
        .boxed()
    }
}

impl Database for SpiralDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        self.request_connection()
    }
}

struct SpiralConnectionProxy {
    active: bool,
    sender: mpsc::Sender<ConnectionRequest>,
}

impl SpiralConnectionProxy {
    fn new(sender: mpsc::Sender<ConnectionRequest>) -> Self {
        Self {
            active: true,
            sender,
        }
    }

    fn ensure_active(&self) -> DatabaseResult<()> {
        self.active
            .then_some(())
            .ok_or_else(|| DatabaseError::invalid_state("database connection is closed"))
    }

    fn take_sender(&mut self) -> DatabaseResult<mpsc::Sender<ConnectionRequest>> {
        self.ensure_active()?;
        self.active = false;
        Ok(self.sender.clone())
    }
}

impl DatabaseSession for SpiralConnectionProxy {
    fn execute(&mut self, command: DatabaseCommand) -> DatabaseFuture<'_, DatabaseOutput> {
        let active = self.ensure_active();
        let sender = self.sender.clone();
        async move {
            active?;
            let (response, reply) = oneshot::channel();
            sender
                .send(ConnectionRequest::Execute(ExecuteRequest {
                    command,
                    response,
                }))
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)?
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        let active = self.ensure_active();
        let sender = self.sender.clone();
        async move {
            active?;
            let (response, reply) = oneshot::channel();
            sender
                .send(ConnectionRequest::TableSchema(TableSchemaRequest {
                    relation,
                    response,
                }))
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)?
        }
        .boxed()
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        let active = self.ensure_active();
        let sender = self.sender.clone();
        async move {
            active?;
            let (response, reply) = oneshot::channel();
            sender
                .send(ConnectionRequest::Relations { response })
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)?
        }
        .boxed()
    }
}

impl DatabaseConnection for SpiralConnectionProxy {
    fn begin(mut self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        let sender = self.take_sender();
        async move {
            let sender = sender?;
            let (response, reply) = oneshot::channel();
            sender
                .send(ConnectionRequest::Begin { response })
                .await
                .map_err(DatabaseError::closed)?;
            reply.await.map_err(DatabaseError::closed)??;
            Ok(Box::new(SpiralTransactionProxy::new(sender)) as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}

struct SpiralTransactionProxy {
    connection: SpiralConnectionProxy,
}

impl SpiralTransactionProxy {
    fn new(sender: mpsc::Sender<ConnectionRequest>) -> Self {
        Self {
            connection: SpiralConnectionProxy::new(sender),
        }
    }
}

impl DatabaseSession for SpiralTransactionProxy {
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

impl DatabaseTransaction for SpiralTransactionProxy {
    fn commit(mut self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        let sender = self.connection.take_sender();
        async move { TransactionTerminal::Commit.finish(sender?).await }.boxed()
    }

    fn rollback(mut self: Box<Self>) -> DatabaseFuture<'static, ()> {
        let sender = self.connection.take_sender();
        async move {
            TransactionTerminal::Rollback
                .finish(sender?)
                .await
                .map(|_| ())
        }
        .boxed()
    }
}

enum TransactionTerminal {
    Commit,
    Rollback,
}

impl TransactionTerminal {
    async fn finish(
        self,
        sender: mpsc::Sender<ConnectionRequest>,
    ) -> DatabaseResult<ExecutionSummary> {
        let (response, reply) = oneshot::channel();
        sender
            .send(ConnectionRequest::FinishTransaction(
                FinishTransactionRequest {
                    terminal: self,
                    response,
                },
            ))
            .await
            .map_err(DatabaseError::closed)?;
        reply.await.map_err(DatabaseError::closed)?
    }
}

struct OpenConnectionRequest {
    response: oneshot::Sender<DatabaseResult<Box<dyn DatabaseConnection>>>,
}

enum DatabaseServiceRequest {
    Open(OpenConnectionRequest),
    Configure {
        settings: SpiralConnectionSettings,
        response: oneshot::Sender<DatabaseResult<()>>,
    },
}

enum ConnectionRequest {
    Execute(ExecuteRequest),
    TableSchema(TableSchemaRequest),
    Relations {
        response: oneshot::Sender<DatabaseResult<Vec<RelationMetadata>>>,
    },
    Begin {
        response: oneshot::Sender<DatabaseResult<()>>,
    },
    FinishTransaction(FinishTransactionRequest),
}

struct ExecuteRequest {
    command: DatabaseCommand,
    response: oneshot::Sender<DatabaseResult<DatabaseOutput>>,
}

struct TableSchemaRequest {
    relation: RelationName,
    response: oneshot::Sender<DatabaseResult<Option<Arc<Schema>>>>,
}

struct FinishTransactionRequest {
    terminal: TransactionTerminal,
    response: oneshot::Sender<DatabaseResult<ExecutionSummary>>,
}

struct SpiralDatabaseService {
    session: SpiralSession,
    receiver: mpsc::Receiver<DatabaseServiceRequest>,
}

impl SpiralDatabaseService {
    fn new(
        settings: SpiralConnectionSettings,
        receiver: mpsc::Receiver<DatabaseServiceRequest>,
    ) -> DatabaseResult<Self> {
        Ok(Self {
            session: Self::create_session(&settings)?,
            receiver,
        })
    }

    fn create_session(settings: &SpiralConnectionSettings) -> DatabaseResult<SpiralSession> {
        let client_settings = ClientSettings::load().map_err(DatabaseError::from_external)?;
        let api_url = Url::parse(settings.endpoint()).map_err(DatabaseError::from_external)?;
        let spfs_url =
            Url::parse(settings.spfs_endpoint()).map_err(DatabaseError::from_external)?;
        let mut client_options = SpiralClientOptions::new(client_settings)
            .with_api_url(api_url)
            .with_spfs_url(spfs_url);
        if let Some(credentials) = settings.credentials() {
            client_options = client_options.with_client_credentials(ClientCredentials::new(
                credentials.client_id().to_owned(),
                credentials.client_secret().to_owned(),
            ));
        }
        let session = default_session_with_client_options(client_options)
            .map_err(DatabaseError::from_external)?;
        session
            .relation_catalog_mut()
            .use_catalog(SPIRALDB_CATALOG)
            .map_err(DatabaseError::from_external)?;
        if let Some(project) = settings.project_id() {
            session
                .relation_catalog_mut()
                .use_project(project.to_owned());
        }
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let runtime = EngineRuntime::current(workers).map_err(DatabaseError::from_external)?;
        Ok(runtime.install_io(session))
    }

    async fn run(mut self) {
        while let Some(request) = self.receiver.recv().await {
            match request {
                DatabaseServiceRequest::Open(request) => self.open(request),
                DatabaseServiceRequest::Configure { settings, response } => {
                    let result = Self::create_session(&settings).map(|session| {
                        self.session = session;
                    });
                    let _ = response.send(result);
                }
            }
        }
    }

    fn open(&self, request: OpenConnectionRequest) {
        let (sender, receiver) = mpsc::channel(CONNECTION_CHANNEL_CAPACITY);
        let service = SpiralConnectionService::new(self.session.clone(), receiver);
        tokio::task::spawn_local(service.run());
        let connection =
            Box::new(SpiralConnectionProxy::new(sender)) as Box<dyn DatabaseConnection>;
        let _ = request.response.send(Ok(connection));
    }
}

struct SpiralConnectionService {
    session: SpiralSession,
    connection: Option<ConnectionCtx>,
    receiver: mpsc::Receiver<ConnectionRequest>,
}

impl SpiralConnectionService {
    fn new(session: SpiralSession, receiver: mpsc::Receiver<ConnectionRequest>) -> Self {
        Self {
            connection: Some(ConnectionCtx::new(session.clone())),
            session,
            receiver,
        }
    }

    async fn run(mut self) {
        while let Some(request) = self.receiver.recv().await {
            self.handle(request).await;
        }
    }

    async fn handle(&mut self, request: ConnectionRequest) {
        match request {
            ConnectionRequest::Execute(request) => self.execute(request).await,
            ConnectionRequest::TableSchema(request) => self.table_schema(request).await,
            ConnectionRequest::Relations { response } => {
                let result = self.relations().await;
                let _ = response.send(result);
            }
            ConnectionRequest::Begin { response } => {
                let result = self.begin().await;
                let _ = response.send(result);
            }
            ConnectionRequest::FinishTransaction(request) => {
                self.finish_transaction(request).await;
            }
        }
    }

    fn connection(&mut self) -> DatabaseResult<&mut ConnectionCtx> {
        self.connection
            .as_mut()
            .ok_or_else(|| DatabaseError::invalid_state("database connection is closed"))
    }

    async fn execute(&mut self, request: ExecuteRequest) {
        let ExecuteRequest { command, response } = request;
        let session = self.session.clone();
        let result = match self.connection() {
            Ok(connection) => Self::execute_command(connection, session, command).await,
            Err(error) => Err(error),
        };
        let _ = response.send(result);
    }

    async fn execute_command(
        connection: &mut ConnectionCtx,
        session: SpiralSession,
        command: DatabaseCommand,
    ) -> DatabaseResult<DatabaseOutput> {
        match command {
            DatabaseCommand::Statement(statement) => {
                match connection
                    .execute_command(statement.as_str())
                    .await
                    .map_err(DatabaseError::from_external)?
                {
                    CommandOutput::Statement { rows } => Ok(DatabaseOutput::AffectedRows(
                        ExecutionSummary::new(Some(rows)),
                    )),
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
                let rows = Self::ingest(connection, session, ingestion).await?;
                Ok(DatabaseOutput::AffectedRows(ExecutionSummary::new(Some(
                    rows,
                ))))
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

    async fn table_schema(&mut self, request: TableSchemaRequest) {
        let TableSchemaRequest { relation, response } = request;
        let result = match self.connection() {
            Ok(connection) => {
                let target = Self::relation_name(&relation);
                match target {
                    Ok(target) => {
                        let catalog = connection.relation_catalog().clone();
                        let session = connection.session().clone();
                        catalog
                            .bind_relation_opt(&target, &session)
                            .await
                            .map_err(DatabaseError::from_external)
                            .and_then(|definition| {
                                definition
                                    .map(|definition| {
                                        session
                                            .vortex()
                                            .arrow()
                                            .to_arrow_schema(definition.definition().dtype())
                                            .map(Arc::new)
                                            .map_err(DatabaseError::from_external)
                                    })
                                    .transpose()
                            })
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        let _ = response.send(result);
    }

    async fn relations(&mut self) -> DatabaseResult<Vec<RelationMetadata>> {
        let connection = self.connection()?;
        let request = CatalogEntryRequest::default()
            .with_relation_types(vec![RelationType::Table, RelationType::View]);
        let catalog = connection.relation_catalog().clone();
        let mut entries = catalog
            .list_entries(connection.session(), request)
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

    async fn begin(&mut self) -> DatabaseResult<()> {
        let result = self
            .connection()?
            .set_autocommit(false)
            .await
            .map_err(DatabaseError::from_external);
        if result.is_err() {
            self.connection = None;
        }
        result
    }

    async fn finish_transaction(&mut self, request: FinishTransactionRequest) {
        let FinishTransactionRequest { terminal, response } = request;
        let result = match self.connection.take() {
            Some(connection) => match terminal {
                TransactionTerminal::Commit => connection
                    .commit()
                    .await
                    .map(|_| ExecutionSummary::default())
                    .map_err(DatabaseError::from_external),
                TransactionTerminal::Rollback => connection
                    .rollback()
                    .await
                    .map(|_| ExecutionSummary::default())
                    .map_err(DatabaseError::from_external),
            },
            None => Err(DatabaseError::invalid_state(
                "database transaction is closed",
            )),
        };
        let _ = response.send(result);
    }

    async fn ingest(
        connection: &mut ConnectionCtx,
        session: SpiralSession,
        ingestion: BatchIngestion,
    ) -> DatabaseResult<u64> {
        let (relation, mode, batches) = ingestion.into_parts();
        let target = Self::relation_name(&relation)?;
        let catalog = connection.relation_catalog().clone();
        let exists = catalog
            .bind_relation_opt(&target, &session)
            .await
            .map_err(DatabaseError::from_external)?
            .is_some();
        if mode == IngestMode::Append && !exists {
            return Err(DatabaseError::new(
                DatabaseErrorKind::NotFound,
                format!("relation {:?} does not exist", relation.as_str()),
            ));
        }
        let plan = ArrowStreamSource::plan(session.clone(), batches)?;
        let options = connection.query_options();
        match mode {
            IngestMode::Append => {
                let insert = Relation::new(session, plan)
                    .insert_into(target)
                    .map_err(DatabaseError::from_external)?;
                connection
                    .execute_insert(&insert, options)
                    .await
                    .map(|summary| summary.rows())
                    .map_err(DatabaseError::from_external)
            }
            IngestMode::CreateOrAppend if exists => {
                let insert = Relation::new(session, plan)
                    .insert_into(target)
                    .map_err(DatabaseError::from_external)?;
                connection
                    .execute_insert(&insert, options)
                    .await
                    .map(|summary| summary.rows())
                    .map_err(DatabaseError::from_external)
            }
            IngestMode::CreateOrAppend => {
                Self::create_table(connection, TableCreation::new(target, plan, false), options)
                    .await
            }
            IngestMode::Replace => {
                Self::create_table(connection, TableCreation::new(target, plan, true), options)
                    .await
            }
        }
    }

    async fn create_table(
        connection: &ConnectionCtx,
        creation: TableCreation,
        options: spiral_engine::QueryOptions,
    ) -> DatabaseResult<u64> {
        let (target, plan, replace) = creation.into_parts();
        let create = CreateTable::new(target, plan, [], false, replace, false);
        connection
            .execute_create_table(&create, options)
            .await
            .map(|summary| summary.rows())
            .map_err(DatabaseError::from_external)
    }

    fn relation_name(relation: &RelationName) -> DatabaseResult<SpiralRelationName> {
        Ok(SpiralRelationName::new(
            relation.as_str().split('.').map(str::to_owned),
        ))
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
    fn closed(error: impl fmt::Display) -> Self {
        Self::new(
            DatabaseErrorKind::Unavailable,
            format!("database service is unavailable: {error}"),
        )
    }

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
            metadata: RelationMeta::ordinal(dtype),
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
    ordinal: u64,
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

    fn init_global(&self, _context: &QueryCtx) -> VortexResult<Self::Global> {
        let batches = self
            .batches
            .try_lock()
            .map_err(|_| vortex_err!("Arrow input stream is already being initialized"))?
            .take()
            .ok_or_else(|| vortex_err!("Arrow input stream was already consumed"))?;
        Ok(ArrowStreamGlobal {
            state: Mutex::new(ArrowStreamState {
                batches,
                ordinal: 0,
            }),
        })
    }

    fn as_source(&self) -> Option<&dyn DynSource> {
        Some(self)
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
        let row_count = batch.num_rows() as u64;
        let arrow_struct = StructArray::from(batch);
        let arrow_array: ArrowArrayRef = Arc::new(arrow_struct);
        let field = Field::new_struct("", self.schema.fields().clone(), false);
        let array = self
            .session
            .vortex()
            .arrow()
            .from_arrow_array(arrow_array, &field)?;
        let start = state.ordinal;
        state.ordinal = state.ordinal.saturating_add(row_count);
        Ok(Some(Morsel::new(array, start..state.ordinal)))
    }
}
