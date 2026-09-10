use std::sync::Arc;

use adbc_core::options::{
    IngestMode as AdbcIngestMode, ObjectDepth, OptionConnection, OptionStatement,
};
use adbc_core::{Connection as _, Optionable as _, Statement as _};
use arrow_array::{Array as _, ListArray, StringArray, StructArray};
use arrow_schema::Schema;

use super::streaming::IngestionReader;
use super::worker::ConnectionState;
use super::{DatabaseError, DatabaseResult, ExecutionSummary, RelationMetadata, RelationName};
use crate::db::{DatabaseErrorKind, IngestMode, RelationKind};

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

    pub(super) fn relations(&mut self) -> DatabaseResult<Vec<RelationMetadata>> {
        let catalog = self
            .connection()
            .get_option_string(OptionConnection::CurrentCatalog)?;
        let project = match self
            .connection()
            .get_option_string(OptionConnection::CurrentSchema)
        {
            Ok(project) => Some(project),
            Err(error) if error.status == adbc_core::error::Status::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let reader = self.connection().get_objects(
            ObjectDepth::Tables,
            Some(&catalog),
            project.as_deref(),
            None,
            Some(vec!["TABLE", "VIEW"]),
            None,
        )?;
        let objects = CatalogObjects { project };
        let mut relations = Vec::new();
        for batch in reader {
            let batch = batch.map_err(DatabaseError::from_arrow)?;
            let schemas = batch
                .column_by_name("catalog_db_schemas")
                .and_then(|column| column.as_any().downcast_ref::<ListArray>())
                .ok_or_else(CatalogObjects::invalid_layout)?;
            for row in 0..schemas.len() {
                if !schemas.is_null(row) {
                    objects.append_schemas(&schemas.value(row), &mut relations)?;
                }
            }
        }
        relations.sort_by_cached_key(|relation| relation.clone().into_parts().0);
        Ok(relations)
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
        if mode == IngestMode::ReplaceExisting && state.table_schema(&relation)?.is_none() {
            return Err(DatabaseError::new(
                DatabaseErrorKind::NotFound,
                format!(
                    "relation {relation_name} does not exist",
                    relation_name = relation.as_str()
                ),
            ));
        }
        let temporary = mode == IngestMode::ReplaceTemporary;
        let mode = match mode {
            IngestMode::Append => AdbcIngestMode::Append,
            IngestMode::CreateOrAppend => AdbcIngestMode::CreateAppend,
            IngestMode::ReplaceExisting
            | IngestMode::ReplaceOrCreate
            | IngestMode::ReplaceTemporary => AdbcIngestMode::Replace,
        };
        let mut statement = state.connection().new_statement()?;
        statement.set_option(OptionStatement::TargetTable, relation.as_str().into())?;
        statement.set_option(OptionStatement::IngestMode, mode.into())?;
        statement.set_option(OptionStatement::Temporary, temporary.to_string().into())?;
        statement.bind_stream(Box::new(reader))?;
        statement.execute_update()?;
        Ok(ExecutionSummary)
    }
}

struct CatalogObjects {
    project: Option<String>,
}

impl CatalogObjects {
    fn append_schemas(
        &self,
        schemas: &dyn arrow_array::Array,
        relations: &mut Vec<RelationMetadata>,
    ) -> DatabaseResult<()> {
        let schemas = schemas
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(Self::invalid_layout)?;
        let names = schemas
            .column_by_name("db_schema_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or_else(Self::invalid_layout)?;
        let tables = schemas
            .column_by_name("db_schema_tables")
            .and_then(|column| column.as_any().downcast_ref::<ListArray>())
            .ok_or_else(Self::invalid_layout)?;
        for row in 0..tables.len() {
            // ADBC filters use SQL patterns; an underscore in a project ID
            // must not admit a different project's identically named tables.
            let selected = self
                .project
                .as_deref()
                .is_none_or(|project| !names.is_null(row) && names.value(row) == project);
            if selected && !schemas.is_null(row) && !tables.is_null(row) {
                Self::append_tables(&tables.value(row), relations)?;
            }
        }
        Ok(())
    }

    fn append_tables(
        tables: &dyn arrow_array::Array,
        relations: &mut Vec<RelationMetadata>,
    ) -> DatabaseResult<()> {
        let tables = tables
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(Self::invalid_layout)?;
        let names = tables
            .column_by_name("table_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or_else(Self::invalid_layout)?;
        let kinds = tables
            .column_by_name("table_type")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or_else(Self::invalid_layout)?;
        for row in 0..tables.len() {
            if tables.is_null(row) || names.is_null(row) || kinds.is_null(row) {
                continue;
            }
            let kind = match kinds.value(row) {
                "TABLE" => RelationKind::Table,
                "VIEW" => RelationKind::View,
                _ => continue,
            };
            relations.push(RelationMetadata::new(
                RelationName::parse(names.value(row))?,
                kind,
            ));
        }
        Ok(())
    }

    fn invalid_layout() -> DatabaseError {
        DatabaseError::new(
            DatabaseErrorKind::Internal,
            "ADBC get_objects returned an invalid metadata layout",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::ArrayRef;
    use arrow_buffer::{OffsetBuffer, ScalarBuffer};
    use arrow_schema::{DataType, Field};

    #[test]
    fn project_pattern_matches_cannot_leak_other_projects_relations() {
        let tables = StructArray::from(vec![
            (
                Arc::new(Field::new("table_name", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["selected", "foreign"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("table_type", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["TABLE", "TABLE"])) as ArrayRef,
            ),
        ]);
        let tables = ListArray::new(
            Arc::new(Field::new_list_field(tables.data_type().clone(), true)),
            OffsetBuffer::new(ScalarBuffer::from(vec![0, 1, 2])),
            Arc::new(tables),
            None,
        );
        let schemas = StructArray::from(vec![
            (
                Arc::new(Field::new("db_schema_name", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["project_1", "projectX1"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new(
                    "db_schema_tables",
                    tables.data_type().clone(),
                    true,
                )),
                Arc::new(tables) as ArrayRef,
            ),
        ]);
        let mut relations = Vec::new();
        CatalogObjects {
            project: Some("project_1".to_owned()),
        }
        .append_schemas(&schemas, &mut relations)
        .unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations.pop().unwrap().into_parts().0.as_str(), "selected");
    }
}
