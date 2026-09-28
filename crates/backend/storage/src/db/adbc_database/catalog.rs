use std::sync::Arc;

use adbc_core::options::{IngestMode as AdbcIngestMode, OptionStatement};
use adbc_core::{Connection as _, Optionable as _, Statement as _};
use arrow_schema::Schema;

use super::streaming::IngestionReader;
use super::worker::ConnectionState;
use super::{DatabaseResult, ExecutionSummary, RelationName};
use crate::db::IngestMode;

impl ConnectionState {
    pub(super) fn table_schema(
        &mut self,
        relation: &RelationName,
    ) -> DatabaseResult<Option<Arc<Schema>>> {
        match self
            .connection()
            .get_table_schema(None, None, relation.as_str())
        {
            Ok(schema) => Ok(Some(Arc::new(schema))),
            Err(error) if error.status == adbc_core::error::Status::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// Destination and mode stay together while an ingestion crosses the worker.
pub(super) struct IngestionOperation {
    relation: RelationName,
    mode: IngestMode,
    reader: IngestionReader,
}

impl IngestionOperation {
    pub(super) fn new(relation: RelationName, mode: IngestMode, reader: IngestionReader) -> Self {
        Self {
            relation,
            mode,
            reader,
        }
    }

    pub(super) fn execute(self, state: &mut ConnectionState) -> DatabaseResult<ExecutionSummary> {
        let Self {
            relation,
            mode,
            reader,
        } = self;
        let mode = match mode {
            IngestMode::CreateOrAppend => AdbcIngestMode::CreateAppend,
            IngestMode::ReplaceOrCreate => AdbcIngestMode::Replace,
        };
        let mut statement = state.connection().new_statement()?;
        statement.set_option(OptionStatement::TargetTable, relation.as_str().into())?;
        statement.set_option(OptionStatement::IngestMode, mode.into())?;
        statement.bind_stream(Box::new(reader))?;
        statement.execute_update()?;
        Ok(ExecutionSummary)
    }
}
