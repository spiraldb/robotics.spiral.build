//! Spiral through its SDK: statements planned by DataFusion over the
//! project's tables on `main`, writes published through SDK transactions.
//! The SDK reads and writes through Spiral's data-plane runtime, a shared
//! library built from the same SpiralDB commit as the SDK, loaded when the
//! first connection opens.

use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::Arc;

use arrow_schema::Schema;
use datafusion::catalog::TableProvider as _;
use datafusion::prelude::SessionConfig;
use futures::FutureExt as _;
use spiral::auth::{CredentialError, Token};
use spiral::{Client, CommitOutcome, EmbeddedConfig};
use spiral_datafusion::{SpiralCatalogProvider, SpiralTableProvider};
use tokio::sync::OnceCell;

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream,
    RelationName, SqlStatement,
};
use super::staged_writes::{StagedWrite, StagedWrites};
use crate::SpiralConnection;

/// One Spiral project as the signed-in user reaches it. No SDK work runs in
/// `new`.
pub struct SpiralSdkDatabase {
    connection: SpiralConnection,
    project: OnceCell<SpiralProject>,
}

impl SpiralSdkDatabase {
    pub fn new(connection: SpiralConnection) -> Self {
        Self {
            connection,
            project: OnceCell::new(),
        }
    }

    /// The project every connection works in, its client built by the first.
    async fn project(&self) -> DatabaseResult<&SpiralProject> {
        self.project
            .get_or_try_init(|| async { SpiralProject::open(&self.connection) })
            .await
    }
}

impl Database for SpiralSdkDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            let project = self.project().await?.clone();
            Ok(Box::new(SpiralSdkConnection { project }) as Box<dyn DatabaseConnection>)
        }
        .boxed()
    }
}

/// The project's tables as statements and writes reach them: the latest
/// commit on `main` each time.
#[derive(Clone)]
struct SpiralProject {
    project: spiral::Project,
}

impl SpiralProject {
    const BRANCH: &str = "main";
    /// Where the data-plane runtime is looked for when no path is given.
    const RUNTIME: &str = "spiral_data_plane_runtime";

    /// A client authenticating every request with the user's token as
    /// Studio holds it then, so long work outlives each token.
    fn open(connection: &SpiralConnection) -> DatabaseResult<Self> {
        let settings = connection.settings();
        let project = settings
            .project_id()
            .ok_or_else(|| DatabaseError::invalid_input("Select a Spiral project"))?
            .to_owned();
        let access = connection.access().clone();
        let tokens = spiral::auth::auth_token_provider_fn(move || {
            let token = access.token();
            async move {
                token
                    .map(|token| Some(Token::from(token)))
                    .map_err(|message| {
                        CredentialError::InvalidArgument(message.into(), Backtrace::capture())
                    })
            }
            .boxed()
        });
        let storage =
            EmbeddedConfig::spfs(settings.spfs_endpoint()).map_err(DatabaseError::from_spiral)?;
        let client = Client::builder()
            .api_url(settings.endpoint())
            .auth_token_provider(tokens)
            .embedded_library(Self::runtime_library()?, storage)
            .build()
            .map_err(DatabaseError::from_spiral)?;
        Ok(Self {
            project: client
                .project(project)
                .map_err(DatabaseError::from_spiral)?,
        })
    }

    /// `SPIRAL_DATA_PLANE_LIBRARY`, an absolute path, or the platform's
    /// library beside the server executable.
    fn runtime_library() -> DatabaseResult<PathBuf> {
        if let Some(path) = std::env::var_os("SPIRAL_DATA_PLANE_LIBRARY") {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(DatabaseError::invalid_input(
                    "SPIRAL_DATA_PLANE_LIBRARY must be an absolute library path",
                ));
            }
            return Ok(path);
        }
        let executable = std::env::current_exe().map_err(|error| {
            DatabaseError::new(DatabaseErrorKind::Unavailable, error.to_string())
        })?;
        Ok(executable.with_file_name(format!(
            "{}{}{}",
            std::env::consts::DLL_PREFIX,
            Self::RUNTIME,
            std::env::consts::DLL_SUFFIX
        )))
    }

    async fn query(&self, sql: SqlStatement) -> DatabaseResult<RecordBatchStream> {
        let tables = self
            .project
            .table_session(Self::BRANCH)
            .await
            .map_err(DatabaseError::from_spiral)?;
        let context = SpiralCatalogProvider::new(tables).session(SessionConfig::new());
        sql.stream_on(&context).await
    }

    /// The table named `relation` on `main` now, if there is one.
    async fn table(
        &self,
        snapshot: &spiral::Snapshot,
        relation: &RelationName,
    ) -> DatabaseResult<Option<spiral::Table>> {
        match snapshot.table(relation.as_str()).await {
            Ok(table) => Ok(Some(table)),
            Err(spiral::Error::NotFound(_)) => Ok(None),
            Err(error) => Err(DatabaseError::from_spiral(error)),
        }
    }

    async fn committed_schema(
        &self,
        relation: &RelationName,
    ) -> DatabaseResult<Option<Arc<Schema>>> {
        let snapshot = self.snapshot().await?;
        let Some(table) = self.table(&snapshot, relation).await? else {
            return Ok(None);
        };
        let provider = SpiralTableProvider::try_new(table)
            .await
            .map_err(DatabaseError::from_datafusion)?;
        Ok(Some(provider.schema()))
    }

    async fn snapshot(&self) -> DatabaseResult<spiral::Snapshot> {
        self.project
            .snapshot(Self::BRANCH)
            .await
            .map_err(DatabaseError::from_spiral)
    }

    /// Publish `writes` in one commit on top of `main` as it is now; a
    /// conflict with another writer publishes nothing.
    async fn publish(&self, writes: StagedWrites) -> DatabaseResult<ExecutionSummary> {
        if writes.is_empty() {
            return Ok(ExecutionSummary);
        }
        let snapshot = self.snapshot().await?;
        let mut transaction = snapshot.transaction().map_err(DatabaseError::from_spiral)?;
        for (relation, write) in writes.into_writes() {
            let existing = self.table(&snapshot, &relation).await?;
            let staged = match (write, existing) {
                (StagedWrite::Append(rows), Some(_)) if rows.is_empty() => Ok(()),
                (StagedWrite::Append(rows), Some(table)) => transaction
                    .append(&table, rows.into_parts())
                    .await
                    .map(drop),
                (StagedWrite::Replace(rows), Some(table)) => transaction
                    .overwrite(&table, rows.into_parts())
                    .await
                    .map(drop),
                (StagedWrite::Append(rows) | StagedWrite::Replace(rows), None) => transaction
                    .create_table(relation.as_str(), rows.into_parts())
                    .await
                    .map(drop),
                (StagedWrite::Drop, None) => Ok(()),
                (StagedWrite::Drop, Some(table)) => transaction.drop_table(&table),
            };
            staged.map_err(DatabaseError::from_spiral)?;
        }
        match transaction
            .commit()
            .await
            .map_err(DatabaseError::from_spiral)?
        {
            CommitOutcome::Committed(_) => Ok(ExecutionSummary),
            CommitOutcome::Conflict(_) => Err(DatabaseError::new(
                DatabaseErrorKind::Conflict,
                "the Spiral project changed while writing; nothing was written",
            )),
        }
    }
}

/// An autocommit connection: every write is its own commit.
struct SpiralSdkConnection {
    project: SpiralProject,
}

impl DatabaseSession for SpiralSdkConnection {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move { self.project.query(sql).await }.boxed()
    }

    fn drop_table(&mut self, relation: RelationName) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let mut writes = StagedWrites::default();
            writes.drop_table(relation);
            self.project.publish(writes).await
        }
        .boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let mut writes = StagedWrites::default();
            writes.stage(ingestion).await?;
            self.project.publish(writes).await
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move { self.project.committed_schema(&relation).await }.boxed()
    }
}

impl DatabaseConnection for SpiralSdkConnection {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move {
            Ok(Box::new(SpiralSdkTransaction {
                project: self.project,
                writes: StagedWrites::default(),
            }) as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}

/// A transaction: its writes are held, and published in one commit.
struct SpiralSdkTransaction {
    project: SpiralProject,
    writes: StagedWrites,
}

impl DatabaseSession for SpiralSdkTransaction {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move { self.project.query(sql).await }.boxed()
    }

    fn drop_table(&mut self, relation: RelationName) -> DatabaseFuture<'_, ExecutionSummary> {
        self.writes.drop_table(relation);
        async { Ok(ExecutionSummary) }.boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            self.writes.stage(ingestion).await?;
            Ok(ExecutionSummary)
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move {
            match self.writes.schema(&relation) {
                Some(held) => Ok(held),
                None => self.project.committed_schema(&relation).await,
            }
        }
        .boxed()
    }
}

impl DatabaseTransaction for SpiralSdkTransaction {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        async move { self.project.publish(self.writes).await }.boxed()
    }

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async { Ok(()) }.boxed()
    }
}

impl DatabaseError {
    fn from_spiral(error: spiral::Error) -> Self {
        let kind = match &error {
            spiral::Error::NotFound(_) => DatabaseErrorKind::NotFound,
            spiral::Error::InvalidConfiguration(_) => DatabaseErrorKind::InvalidInput,
            spiral::Error::CredentialsNotFound => DatabaseErrorKind::InvalidState,
            spiral::Error::Protocol(_) | spiral::Error::Arrow(_) => DatabaseErrorKind::Internal,
            _ => DatabaseErrorKind::Unavailable,
        };
        Self::new(kind, error.to_string())
    }
}
