//! SQL operators over named Arrow inputs. Spiral owns SQL planning and execution;
//! temporary input relations belong to one connection and never enter the catalog.

use crate::db::RecordBatchStream;
use crate::{ProjectConnection, ProjectStore};
use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use futures::StreamExt as _;
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode, WorkflowSql};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

/// A source has no inputs. A transform names its inputs with `{{node:id}}`;
/// the same contract supports projections, filters, aggregates, and joins.
pub struct WorkflowSqlNode {
    sql: String,
    inputs: BTreeSet<String>,
}

impl WorkflowSqlNode {
    pub fn new(sql: impl Into<String>) -> Result<Self, String> {
        let sql = sql.into();
        ReadOnlyQuery::new(&sql, ReadOnlyQueryMode::Training)
            .authorize()
            .map_err(|error| error.to_string())?;
        let inputs = WorkflowSql::from(sql.as_str())
            .node_placeholders()
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect();
        Ok(Self { sql, inputs })
    }

    /// Repeated references need one shared Arrow binding, even with one consumer.
    pub fn can_inline_input(&self, name: &str) -> bool {
        self.sql.matches(&format!("{{{{node:{name}}}}}")).count() == 1
    }

    /// Fuse a single-consumer SQL input into this SQL operator. Shared graph
    /// inputs remain bound Arrow relations, so fusion never replays a source.
    pub fn inline_input(&mut self, name: &str, input: &Self) -> Result<(), String> {
        if !self.can_inline_input(name) || !self.inputs.remove(name) {
            return Err(format!("SQL has no input {name:?}"));
        }
        self.sql = self
            .sql
            .replace(&format!("{{{{node:{name}}}}}"), &format!("({})", input.sql));
        self.inputs.extend(input.inputs.iter().cloned());
        Ok(())
    }

    /// Native planning checks field references and expression types without
    /// consuming upstream data or running inference.
    pub async fn plan(
        &self,
        store: &ProjectStore,
        inputs: BTreeMap<String, SchemaRef>,
    ) -> Result<SchemaRef, String> {
        let mut connection = store.connect().await?;
        self.plan_on(&mut connection, inputs).await
    }

    async fn plan_on(
        &self,
        connection: &mut ProjectConnection,
        inputs: BTreeMap<String, SchemaRef>,
    ) -> Result<SchemaRef, String> {
        let inputs = inputs
            .into_iter()
            .map(|(name, schema)| {
                let batch = RecordBatch::new_empty(schema.clone());
                (
                    name,
                    RecordBatchStream::new(schema, futures::stream::iter([Ok(batch)]).boxed()),
                )
            })
            .collect();
        let sql = self.bind(connection, inputs).await?;
        connection.plan_query(sql).await
    }

    /// Execute an operator whose input/output schemas the graph already checked.
    pub async fn execute(
        &self,
        store: &ProjectStore,
        inputs: BTreeMap<String, RecordBatchStream>,
    ) -> Result<RecordBatchStream, String> {
        let mut connection = store.connect().await?;
        let sql = self.bind(&mut connection, inputs).await?;
        connection.execute_query(sql).await
    }

    async fn bind(
        &self,
        connection: &mut ProjectConnection,
        inputs: BTreeMap<String, RecordBatchStream>,
    ) -> Result<String, String> {
        let names = inputs.keys().cloned().collect::<BTreeSet<_>>();
        if names != self.inputs {
            return Err(format!(
                "SQL input connections {names:?} do not match its named inputs {:?}",
                self.inputs
            ));
        }
        let mut sql = self.sql.clone();
        let invocation = uuid::Uuid::new_v4().simple();
        for (index, (name, stream)) in inputs.into_iter().enumerate() {
            let relation = format!("workflow_input_{invocation}_{index}");
            connection.bind_arrow_input(&relation, stream).await?;
            sql = sql.replace(&format!("{{{{node:{name}}}}}"), &relation);
        }
        Ok(sql)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ProjectStore {
    pub fn workflow_test() -> Self {
        let settings = crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("contract")
        .unwrap();
        Self::new(
            settings,
            "contract".into(),
            std::sync::Arc::new(crate::AdbcDatabase::memory()),
        )
        .unwrap()
    }
}
