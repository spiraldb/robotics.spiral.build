//! The workspaces this backend keeps on the one Spiral endpoint it uses: each
//! is a Spiral project under the name Studio shows for it. A workspace's
//! records stay keyed by its project, so naming one moves nothing.

use sea_orm::{
    ColumnTrait as _, EntityTrait as _, QueryFilter as _, QueryOrder as _, Set,
    TransactionTrait as _,
};
use spiral_robotics_common::workspace::Workspace;

use crate::StudioStore;
use crate::db::StorageClock;
use crate::studio::workspaces as table;

#[derive(Clone)]
pub struct WorkspaceStore {
    studio: StudioStore,
    endpoint: String,
}

impl WorkspaceStore {
    pub fn new(studio: StudioStore, endpoint: impl Into<String>) -> Self {
        Self {
            studio,
            endpoint: endpoint.into(),
        }
    }

    /// Every workspace, oldest first.
    pub async fn all(&self) -> Result<Vec<Workspace>, String> {
        let rows = table::Entity::find()
            .filter(table::Column::Endpoint.eq(&self.endpoint))
            .order_by_asc(table::Column::CreatedMs)
            // Workspaces made in the same millisecond, in the order they were made.
            .order_by_asc(sea_orm::sea_query::Expr::cust("rowid"))
            .all(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows.into_iter().map(Workspace::from).collect())
    }

    /// The workspace on `project_id`, if it has one.
    pub async fn find(&self, project_id: &str) -> Result<Option<Workspace>, String> {
        let row = table::Entity::find_by_id((self.endpoint.clone(), project_id.to_owned()))
            .one(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(row.map(Workspace::from))
    }

    /// Keep `workspace`. `false` means its project already has one, which
    /// stays as it was.
    pub async fn insert(&self, workspace: &Workspace) -> Result<bool, String> {
        let row = table::ActiveModel {
            endpoint: Set(self.endpoint.clone()),
            project_id: Set(workspace.project_id().to_owned()),
            name: Set(Workspace::validated_name(workspace.name())?),
            created_ms: Set(StorageClock::unix_millis()),
        };
        let result = table::Entity::insert(row)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    table::Column::Endpoint,
                    table::Column::ProjectId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(matches!(result, sea_orm::TryInsertResult::Inserted(_)))
    }

    /// Rename the workspace on `project_id`. `false` means it has none.
    pub async fn rename(&self, project_id: &str, name: &str) -> Result<bool, String> {
        let result = table::Entity::update_many()
            .col_expr(
                table::Column::Name,
                sea_orm::sea_query::Expr::value(Workspace::validated_name(name)?),
            )
            .filter(table::Column::Endpoint.eq(&self.endpoint))
            .filter(table::Column::ProjectId.eq(project_id))
            .exec(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(result.rows_affected == 1)
    }

    /// Forget the workspace on `project_id` and every record Studio keeps for
    /// its project. The Spiral project and its data stay. `false` means it
    /// had no workspace.
    pub async fn delete(&self, project_id: &str) -> Result<bool, String> {
        let transaction = self
            .studio
            .connection()
            .await?
            .begin()
            .await
            .map_err(|error| error.to_string())?;
        self.studio
            .project_at(&self.endpoint, project_id)
            .delete_records(&transaction)
            .await?;
        let deleted = table::Entity::delete_by_id((self.endpoint.clone(), project_id.to_owned()))
            .exec(&transaction)
            .await
            .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        Ok(deleted.rows_affected == 1)
    }
}

impl From<table::Model> for Workspace {
    fn from(row: table::Model) -> Self {
        Workspace::new(row.project_id, row.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpiralConnectionSettings;
    use crate::training::TrainingRunPersistence;
    use spiral_robotics_common::training::{
        TrainingConfig, TrainingInputSummary, TrainingRunIdentity,
    };

    const ENDPOINT: &str = "https://api.spiraldb.dev";

    #[tokio::test]
    async fn a_workspace_is_named_renamed_and_deleted_with_its_records() {
        let studio = StudioStore::in_memory();
        let workspaces = WorkspaceStore::new(studio.clone(), ENDPOINT);
        assert!(workspaces.all().await.unwrap().is_empty());
        assert!(
            workspaces
                .insert(&Workspace::new("arms-000001", " Arms "))
                .await
                .unwrap()
        );
        assert!(
            workspaces
                .insert(&Workspace::new("bench-000002", "Bench"))
                .await
                .unwrap()
        );
        // A project has one workspace; the first keeps its name.
        assert!(
            !workspaces
                .insert(&Workspace::new("arms-000001", "Again"))
                .await
                .unwrap()
        );
        assert!(
            workspaces
                .insert(&Workspace::new("arms-000003", " "))
                .await
                .is_err()
        );
        assert_eq!(
            workspaces.all().await.unwrap(),
            [
                Workspace::new("arms-000001", "Arms"),
                Workspace::new("bench-000002", "Bench")
            ]
        );
        // Another endpoint's workspaces are another backend's.
        assert!(
            WorkspaceStore::new(studio.clone(), "https://other.spiraldb.dev")
                .all()
                .await
                .unwrap()
                .is_empty()
        );

        assert!(workspaces.rename("arms-000001", "Lab arms").await.unwrap());
        assert!(
            !workspaces
                .rename("missing-000004", "Lab arms")
                .await
                .unwrap()
        );
        assert_eq!(
            workspaces.find("arms-000001").await.unwrap(),
            Some(Workspace::new("arms-000001", "Lab arms"))
        );

        let connection = |project: &str| {
            SpiralConnectionSettings::new(
                ENDPOINT.to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap()
            .with_project(project)
            .unwrap()
        };
        for project in ["arms-000001", "bench-000002"] {
            TrainingRunPersistence::new(&studio.project(&connection(project)))
                .insert(&TrainingRunIdentity::new(
                    "run",
                    TrainingConfig::default(),
                    TrainingInputSummary::new(1, 1, 2.0),
                    1,
                ))
                .await
                .unwrap();
        }
        assert!(workspaces.delete("arms-000001").await.unwrap());
        assert!(!workspaces.delete("arms-000001").await.unwrap());
        assert_eq!(
            workspaces.all().await.unwrap(),
            [Workspace::new("bench-000002", "Bench")]
        );
        let kept = |project: &str| {
            let project = studio.project(&connection(project));
            async move {
                TrainingRunPersistence::new(&project)
                    .load()
                    .await
                    .unwrap()
                    .len()
            }
        };
        assert_eq!(kept("arms-000001").await, 0);
        assert_eq!(kept("bench-000002").await, 1);
    }
}
