//! One row per exploration. The list reads identity and revision; the
//! payload is loaded by identity and replaced only on an explicit Save.
//!
//! Storage never parses the payload. Its only rules are the ones
//! [`ExplorationDocument::validate`] states plus the revision guard below, so a
//! change to the frontend's document schema never needs a migration here.

use spiral_robotics_common::exploration::{ExplorationDocument, ExplorationSummary};

use crate::db::StorageClock;

pub(crate) struct ExplorationPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> ExplorationPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    /// Read one stored row back as the document that was saved. A row that
    /// cannot become a valid document is reported, never silently repaired.
    pub(crate) fn document(
        row: crate::studio::explorations::Model,
    ) -> Result<ExplorationDocument, String> {
        let revision =
            u64::try_from(row.revision).map_err(|_| "Exploration revision is negative")?;
        let encoding =
            u32::try_from(row.encoding).map_err(|_| "Exploration encoding is negative")?;
        let document = ExplorationDocument::new(row.id, row.name, encoding, row.payload)
            .with_revision(revision);
        document.validate()?;
        Ok(document)
    }

    pub(crate) async fn list(&self) -> Result<Vec<ExplorationSummary>, String> {
        use crate::studio::explorations as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
        let rows: Vec<(String, String, i64)> = table::Entity::find()
            .select_only()
            .columns([
                table::Column::Id,
                table::Column::Name,
                table::Column::Revision,
            ])
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_desc(table::Column::UpdatedMs)
            .order_by_asc(table::Column::Id)
            .into_tuple()
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|(id, name, revision)| {
                let revision =
                    u64::try_from(revision).map_err(|_| "Exploration revision is negative")?;
                Ok(ExplorationSummary::new(id, name, revision))
            })
            .collect()
    }

    pub(crate) async fn load(&self, id: &str) -> Result<Option<ExplorationDocument>, String> {
        use crate::studio::explorations as table;
        use sea_orm::EntityTrait;
        let row = table::Entity::find_by_id((self.project_store.key().to_owned(), id.to_owned()))
            .one(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        row.map(Self::document).transpose()
    }

    /// Conditional write against the revision the document was loaded at:
    /// revision 0 inserts a document that does not exist yet, and revision n
    /// replaces exactly revision n. The stored copy carries n + 1. `false`
    /// means another save won the race and nothing was written.
    pub(crate) async fn save(&self, document: &ExplorationDocument) -> Result<bool, String> {
        use crate::studio::explorations as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
        document.validate()?;
        let loaded_at = i64::try_from(document.revision())
            .map_err(|_| "Exploration revision exceeds SQLite integer range")?;
        let stored = loaded_at
            .checked_add(1)
            .ok_or("Exploration revision exceeds SQLite integer range")?;
        let model = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(document.id().to_owned()),
            name: Set(document.name().to_owned()),
            revision: Set(stored),
            encoding: Set(i32::try_from(document.encoding())
                .map_err(|_| "Exploration encoding exceeds SQLite integer range")?),
            payload: Set(document.payload().to_owned()),
            updated_ms: Set(StorageClock::unix_millis()),
        };
        let database = self.project_store.connection().await?;
        if loaded_at == 0 {
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
                .filter(table::Column::Id.eq(document.id()))
                .filter(table::Column::Revision.eq(loaded_at))
                .exec(database)
                .await
                .map_err(|error| error.to_string())?;
            Ok(result.rows_affected == 1)
        }
    }
}

#[cfg(test)]
mod tests;
