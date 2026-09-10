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

    /// Each execution owns its input bindings. An upstream query may still be
    /// streaming on another connection while Spiral consumes its Arrow batches.
    pub async fn execute(
        &self,
        store: &ProjectStore,
        inputs: BTreeMap<String, RecordBatchStream>,
    ) -> Result<RecordBatchStream, String> {
        let mut connection = store.connect().await?;
        // Validate transforms before consuming upstream streams. A source has
        // no upstream work to protect: execute_query already binds and checks
        // its SQL, so preplanning it would repeat remote catalog resolution.
        if !inputs.is_empty() {
            self.plan_on(
                &mut connection,
                inputs
                    .iter()
                    .map(|(name, stream)| (name.clone(), stream.schema().clone()))
                    .collect(),
            )
            .await?;
        }
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
