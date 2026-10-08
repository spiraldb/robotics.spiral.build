//! The progress models a project published, each by its name: the selector
//! the scorer reads its weights by, the camera it scores, and the checkpoint
//! it came from. The weights themselves stay on the training service.

use spiral_robotics_common::training::{ModelPublication, PublishedModel};

use crate::db::StorageClock;

/// Published progress models of one project.
pub struct PublishedModels<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> PublishedModels<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    /// Every published model, by name.
    pub async fn list(&self) -> Result<Vec<PublishedModel>, String> {
        use crate::studio::published_models as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
        table::Entity::find()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_asc(table::Column::Name)
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| {
                let step = u64::try_from(row.step).map_err(|_| "A published step is negative")?;
                Ok(PublishedModel::new(
                    ModelPublication::new(row.run_id, step, row.name),
                    row.model,
                    row.camera,
                ))
            })
            .collect()
    }

    /// Record `model` under its name. `false` means the project already
    /// publishes a model by that name, in any case, and nothing was written.
    pub async fn insert(&self, model: &PublishedModel) -> Result<bool, String> {
        use crate::studio::published_models as table;
        use sea_orm::{EntityTrait, Set, sea_query::OnConflict};
        let step = i64::try_from(model.step()).map_err(|_| "The step exceeds SQLite's range")?;
        let row = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            name: Set(model.name().to_owned()),
            model: Set(model.model().to_owned()),
            camera: Set(model.camera().to_owned()),
            run_id: Set(model.run_id().to_owned()),
            step: Set(step),
            created_ms: Set(StorageClock::unix_millis()),
        };
        let inserted = table::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([table::Column::Project, table::Column::Name])
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(inserted == 1)
    }
}
