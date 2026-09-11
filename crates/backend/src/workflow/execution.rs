//! One schema-checked Arrow DAG. SQL planning belongs to Spiral; ML only sees batches.
use super::model::ModelOperator;
use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use futures::{StreamExt as _, TryStreamExt as _};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{WorkflowNodeConfig, WorkflowSpec},
};
use spiral_robotics_storage::{
    DatabaseError, DatabaseErrorKind, ProjectStore, RecordBatchStream,
    workflow_sql::WorkflowSqlNode,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) enum NodeEvent {
    Started(String),
    FusedComplete(String),
    Failed {
        id: String,
        error: String,
    },
    Complete {
        id: String,
        rows: usize,
        elapsed_ms: u64,
    },
}

enum Operator {
    Sql(WorkflowSqlNode),
    Model(Box<ModelOperator>),
    Result,
}

struct PlannedNode {
    id: String,
    operator: Operator,
    schema: SchemaRef,
    parents: Vec<String>,
    children: Vec<String>,
    preview_rows: usize,
    fused: Vec<String>,
}

pub(super) struct GraphExecution {
    store: Arc<ProjectStore>,
    cancel: CancellationToken,
    events: mpsc::UnboundedSender<NodeEvent>,
}

impl GraphExecution {
    /// Authoring checks the same native SQL and Model schemas as Run, without
    /// consuming source batches, loading model weights, or publishing run state.
    pub(super) async fn validate_schema(
        store: Arc<ProjectStore>,
        workflow: &WorkflowSpec,
    ) -> Result<(), String> {
        let (events, _) = mpsc::unbounded_channel();
        Self::new(store, CancellationToken::new(), events)
            .plan(workflow)
            .await
            .map(|_| ())
    }

    pub(super) fn new(
        store: Arc<ProjectStore>,
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<NodeEvent>,
    ) -> Self {
        Self {
            store,
            cancel,
            events,
        }
    }

    async fn plan(&self, workflow: &WorkflowSpec) -> Result<Vec<PlannedNode>, String> {
        let mut schemas = BTreeMap::<String, SchemaRef>::new();
        let mut plan: Vec<PlannedNode> = Vec::new();
        for id in workflow.topological_order()? {
            self.check_cancelled()?;
            let node = workflow
                .nodes()
                .iter()
                .find(|n| n.id() == id)
                .ok_or("Missing graph node")?;
            let mut parents = workflow
                .edges()
                .iter()
                .filter(|e| e.target() == id)
                .map(|e| e.source().to_owned())
                .collect::<Vec<_>>();
            let children = workflow
                .edges()
                .iter()
                .filter(|e| e.source() == id)
                .map(|e| e.target().to_owned())
                .collect::<Vec<_>>();
            let inputs = parents
                .iter()
                .map(|id| {
                    Ok((
                        id.clone(),
                        schemas.get(id).cloned().ok_or("Missing input schema")?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let mut operator = match node.config() {
                WorkflowNodeConfig::Table { relation } => {
                    Operator::Sql(WorkflowSqlNode::new(format!("SELECT * FROM {relation}"))?)
                }
                WorkflowNodeConfig::Query { sql, .. } => Operator::Sql(WorkflowSqlNode::new(sql)?),
                WorkflowNodeConfig::Model {
                    image_column,
                    features,
                } => Operator::Model(Box::new(ModelOperator::new(
                    image_column.clone(),
                    features.clone(),
                ))),
                WorkflowNodeConfig::Result => Operator::Result,
            };
            let schema = match &operator {
                Operator::Sql(sql) => sql.plan(&self.store, inputs).await,
                Operator::Model(model) => {
                    model.plan(inputs.values().next().ok_or("Model needs an input")?)
                }
                Operator::Result => inputs
                    .values()
                    .next()
                    .cloned()
                    .ok_or_else(|| "Result needs an input".into()),
            }
            .inspect_err(|error| {
                let _ = self.events.send(NodeEvent::Failed {
                    id: id.clone(),
                    error: error.clone(),
                });
            })
            .map_err(|e| format!("Node {id}: {e}"))?;
            schemas.insert(id.clone(), schema.clone());
            let preview_rows = parents
                .first()
                .and_then(|parent| workflow.nodes().iter().find(|n| n.id() == parent))
                .and_then(|parent| match parent.config() {
                    WorkflowNodeConfig::Query { max_rows, .. } => Some(*max_rows),
                    _ => None,
                })
                .unwrap_or(100);
            let mut fused = Vec::new();
            if let Operator::Sql(sql) = &mut operator {
                // Only inspect the original immediate parents. A shared ancestor
                // must retain its single Arrow binding even if its branches fuse.
                for parent in parents.clone() {
                    if !sql.can_inline_input(&parent) {
                        continue;
                    }
                    let Some(index) = plan.iter().position(|node| {
                        node.id == parent
                            && node.children == [id.clone()]
                            && matches!(node.operator, Operator::Sql(_))
                    }) else {
                        continue;
                    };
                    let upstream = plan.remove(index);
                    let Operator::Sql(input) = upstream.operator else {
                        unreachable!()
                    };
                    sql.inline_input(&parent, &input)?;
                    parents.retain(|name| name != &parent);
                    parents.extend(upstream.parents);
                    fused.extend(upstream.fused);
                    fused.push(parent.clone());
                    for node in &mut plan {
                        for child in &mut node.children {
                            if *child == parent {
                                *child = id.clone();
                            }
                        }
                        node.children.sort();
                        node.children.dedup();
                    }
                }
                parents.sort();
                parents.dedup();
            }
            plan.push(PlannedNode {
                id,
                operator,
                schema,
                parents,
                children,
                preview_rows,
                fused,
            });
        }
        Ok(plan)
    }

    pub(super) async fn run(&self, workflow: &WorkflowSpec) -> Result<SpiralQuerySnapshot, String> {
        // Finish all schema checks before polling sources or loading a model.
        let plan = self.plan(workflow).await?;
        let mut edges = BTreeMap::<(String, String), RecordBatchStream>::new();
        for node in plan {
            self.check_cancelled()?;
            let inputs = node
                .parents
                .iter()
                .map(|parent| {
                    edges
                        .remove(&(parent.clone(), node.id.clone()))
                        .map(|stream| (parent.clone(), stream))
                        .ok_or_else(|| "Missing Arrow input".to_owned())
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            for id in node.fused.iter().chain(std::iter::once(&node.id)) {
                let _ = self.events.send(NodeEvent::Started(id.clone()));
            }
            let started = Instant::now();
            let stream = match node.operator {
                Operator::Sql(sql) => {
                    sql.execute(&self.store, inputs)
                        .await
                        .inspect_err(|error| {
                            let _ = self.events.send(NodeEvent::Failed {
                                id: node.id.clone(),
                                error: error.clone(),
                            });
                        })?
                }
                Operator::Model(model) => model.execute(
                    inputs.into_values().next().ok_or("Model needs an input")?,
                    self.cancel.clone(),
                )?,
                Operator::Result => {
                    let input = inputs.into_values().next().ok_or("Result needs an input")?;
                    let result = input.into_preview_drained(node.preview_rows).await?;
                    let _ = self.events.send(NodeEvent::Complete {
                        id: node.id,
                        rows: result.rows().len(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                    return Ok(result);
                }
            };
            if stream.schema() != &node.schema {
                return Err(format!("Node {} changed schema after planning", node.id));
            }
            let mut output =
                NodeOutput::new(node.id.clone(), self.events.clone(), self.cancel.clone());
            output.fused = node.fused;
            let stream = output.observe(stream, started);
            match node.children.as_slice() {
                [child] => {
                    edges.insert((node.id, child.clone()), stream);
                }
                children => {
                    // A fork executes once. Arrow clones share buffers, and each branch
                    // owns an independent cursor. Materializing forks avoids deadlocks
                    // when native SQL consumes join inputs sequentially.
                    let schema = stream.schema().clone();
                    let batches = stream
                        .try_collect::<Vec<RecordBatch>>()
                        .await
                        .map_err(|e| e.to_string())?;
                    for child in children {
                        edges.insert(
                            (node.id.clone(), child.clone()),
                            RecordBatchStream::new(
                                schema.clone(),
                                futures::stream::iter(batches.clone().into_iter().map(Ok)).boxed(),
                            ),
                        );
                    }
                }
            }
        }
        Err("Workflow has no Result".into())
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            Err("Workflow run cancelled.".into())
        } else {
            Ok(())
        }
    }
}

struct NodeOutput {
    id: String,
    events: mpsc::UnboundedSender<NodeEvent>,
    cancel: CancellationToken,
    rows: usize,
    fused: Vec<String>,
}

impl NodeOutput {
    fn new(
        id: String,
        events: mpsc::UnboundedSender<NodeEvent>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            id,
            events,
            cancel,
            rows: 0,
            fused: Vec::new(),
        }
    }

    fn observe(self, input: RecordBatchStream, started: Instant) -> RecordBatchStream {
        let schema = input.schema().clone();
        let stream =
            futures::stream::try_unfold((self, input), move |(mut output, mut input)| async move {
                if output.cancel.is_cancelled() {
                    return Err(DatabaseError::new(
                        DatabaseErrorKind::Cancelled,
                        "Workflow run cancelled.",
                    ));
                }
                match input.next().await {
                    Some(batch) => {
                        let batch = batch.map_err(|e| {
                            let error = format!("Node {}: {e}", output.id);
                            let _ = output.events.send(NodeEvent::Failed {
                                id: output.id.clone(),
                                error: error.clone(),
                            });
                            DatabaseError::new(e.kind(), error)
                        })?;
                        output.rows += batch.num_rows();
                        Ok(Some((batch, (output, input))))
                    }
                    None => {
                        for id in output.fused {
                            let _ = output.events.send(NodeEvent::FusedComplete(id));
                        }
                        let _ = output.events.send(NodeEvent::Complete {
                            id: output.id,
                            rows: output.rows,
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                        Ok(None)
                    }
                }
            })
            .boxed();
        RecordBatchStream::new(schema, stream)
    }
}

#[cfg(test)]
mod tests;
