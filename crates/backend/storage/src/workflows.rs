//! One row per workflow. The list reads metadata; graph and conversation JSON
//! are loaded by identity and saved together only on explicit Save.

mod cameras;

use spiral_robotics_common::workflow::{WorkflowSpec, WorkflowSummary};

use crate::db::StorageClock;

pub struct WorkflowPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> WorkflowPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    pub async fn list(&self) -> Result<Vec<WorkflowSummary>, String> {
        use crate::studio::workflows as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
        let rows: Vec<(String, String)> = table::Entity::find()
            .select_only()
            .columns([table::Column::Id, table::Column::Name])
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_desc(table::Column::UpdatedMs)
            .order_by_asc(table::Column::Id)
            .into_tuple()
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|(id, name)| WorkflowSummary::new(id, name))
            .collect())
    }

    pub async fn load(&self, workflow_id: &str) -> Result<Option<StoredWorkflowDocument>, String> {
        use crate::studio::workflows as table;
        use sea_orm::EntityTrait;
        let row = table::Entity::find_by_id((
            self.project_store.key().to_owned(),
            workflow_id.to_owned(),
        ))
        .one(self.project_store.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        row.map(|row| serde_json::from_value(row.document).map_err(|error| error.to_string()))
            .transpose()
    }

    pub async fn save(
        &self,
        workflow: &WorkflowSpec,
        contents: (&WorkflowJson, &WorkflowJson),
    ) -> Result<bool, String> {
        use crate::studio::workflows as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
        let revision = i64::try_from(workflow.revision())
            .map_err(|_| "Workflow revision exceeds SQLite integer range")?;
        if revision < 1 {
            return Ok(false);
        }
        let document = StoredWorkflowDocument {
            graph: contents.0.clone(),
            conversation: Some(contents.1.clone()),
        };
        let model = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(workflow.id().to_owned()),
            name: Set(workflow.name().to_owned()),
            revision: Set(revision),
            document: Set(serde_json::to_value(document).map_err(|error| error.to_string())?),
            updated_ms: Set(StorageClock::unix_millis()),
        };
        let database = self.project_store.connection().await?;
        if revision == 1 {
            let result = table::Entity::insert(model)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        table::Column::Project,
                        table::Column::Id,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .do_nothing()
                .exec(database)
                .await
                .map_err(|error| error.to_string())?;
            Ok(matches!(result, sea_orm::TryInsertResult::Inserted(_)))
        } else {
            let result = table::Entity::update_many()
                .set(model)
                .filter(table::Column::Project.eq(self.project_store.key()))
                .filter(table::Column::Id.eq(workflow.id()))
                .filter(table::Column::Revision.eq(revision - 1))
                .exec(database)
                .await
                .map_err(|error| error.to_string())?;
            Ok(result.rows_affected == 1)
        }
    }
}

pub use spiral_robotics_common::workflow::document::WorkflowJson;

/// Encoded graph and conversation read together from one saved workflow row.
#[derive(Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWorkflowDocument {
    graph: WorkflowJson,
    conversation: Option<WorkflowJson>,
}

impl StoredWorkflowDocument {
    pub fn graph(&self) -> &WorkflowJson {
        &self.graph
    }

    pub fn conversation(&self) -> Option<&WorkflowJson> {
        self.conversation.as_ref()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl crate::ProjectStore {
    pub fn workflow_test() -> Self {
        let settings = crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("contract")
        .unwrap();
        Self::new(
            crate::SpiralConnection::new(settings, crate::SpiralAccessPublisher::new().1),
            "contract".into(),
            std::sync::Arc::new(crate::AdbcDatabase::memory()),
        )
        .unwrap()
    }
}

impl StoredWorkflowDocument {
    pub(crate) fn validate(
        &self,
        workflow: &crate::studio::workflows::Model,
    ) -> Result<(), String> {
        let graph: serde_json::Value =
            serde_json::from_str(self.graph.payload()).map_err(|error| error.to_string())?;
        let saved: WorkflowSpec = serde_json::from_value(
            graph
                .get("workflow")
                .cloned()
                .ok_or("Workflow graph is missing")?,
        )
        .map_err(|error| error.to_string())?;
        if saved.id() != workflow.id
            || saved.name() != workflow.name
            || saved.revision() != workflow.revision as u64
            || workflow.revision < 1
            || workflow.id.is_empty()
            || workflow.id.len() > 128
            || workflow.name.trim().is_empty()
        {
            return Err(
                "Workflow document identity, name, or revision disagrees with its row".into(),
            );
        }
        if let Some(conversation) = &self.conversation {
            crate::workflow_assistant::WorkflowConversation::load(conversation, &workflow.id)?;
        }
        Ok(())
    }
}
