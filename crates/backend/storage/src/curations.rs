//! One row per curation. The list reads identity and revision; the payload
//! is loaded by identity, replaced only on an explicit Save, and deleted only
//! at the revision it was loaded at.
//!
//! Storage never parses the payload. Its only rules are the ones
//! [`CurationDocument::validate`] states plus the revision guard below, so a
//! change to the frontend's document schema never needs a migration here.
//!
//! A published output is a statement Studio keeps under the view's name,
//! replaced whole on every publish; [`CuratedViews`] owns those rows.

use spiral_robotics_common::curation::{
    CurationDocument, CurationSummary, PublishedView, ViewName,
};

use crate::db::StorageClock;

pub struct CurationPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> CurationPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    /// Read one stored row back as the document that was saved. A row that
    /// cannot become a valid document is reported, never silently repaired.
    pub(crate) fn document(
        row: crate::studio::curations::Model,
    ) -> Result<CurationDocument, String> {
        let revision = u64::try_from(row.revision).map_err(|_| "Curation revision is negative")?;
        let encoding = u32::try_from(row.encoding).map_err(|_| "Curation encoding is negative")?;
        let document =
            CurationDocument::new(row.id, row.name, encoding, row.payload).with_revision(revision);
        document.validate()?;
        Ok(document)
    }

    pub async fn list(&self) -> Result<Vec<CurationSummary>, String> {
        use crate::studio::curations as table;
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
            .map(|(id, name)| CurationSummary::new(id, name))
            .collect())
    }

    pub async fn load(&self, id: &str) -> Result<Option<CurationDocument>, String> {
        use crate::studio::curations as table;
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
    pub async fn save(&self, document: &CurationDocument) -> Result<bool, String> {
        use crate::studio::curations as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
        document.validate()?;
        let loaded_at = i64::try_from(document.revision())
            .map_err(|_| "Curation revision exceeds SQLite integer range")?;
        let stored = loaded_at
            .checked_add(1)
            .ok_or("Curation revision exceeds SQLite integer range")?;
        let model = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(document.id().to_owned()),
            name: Set(document.name().to_owned()),
            revision: Set(stored),
            encoding: Set(i32::try_from(document.encoding())
                .map_err(|_| "Curation encoding exceeds SQLite integer range")?),
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

    /// Delete the curation stored at exactly `revision`. `false` means it is
    /// gone, or another save moved it on, and nothing was deleted.
    pub async fn delete(&self, id: &str, revision: u64) -> Result<bool, String> {
        use crate::studio::curations as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let revision = i64::try_from(revision)
            .map_err(|_| "Curation revision exceeds SQLite integer range")?;
        let result = table::Entity::delete_many()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .filter(table::Column::Id.eq(id))
            .filter(table::Column::Revision.eq(revision))
            .exec(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(result.rows_affected == 1)
    }
}

/// The views curation outputs are published as: each one's statement, under
/// the name Data reads it by.
pub struct CuratedViews<'a> {
    project_store: &'a crate::StudioProject,
    spiral: &'a crate::ProjectStore,
}

impl<'a> CuratedViews<'a> {
    pub(crate) const fn new(
        project_store: &'a crate::StudioProject,
        spiral: &'a crate::ProjectStore,
    ) -> Self {
        Self {
            project_store,
            spiral,
        }
    }

    /// Every view the project's curations published, by name.
    pub async fn list(&self) -> Result<Vec<PublishedView>, String> {
        use crate::studio::curated_views as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
        let rows = table::Entity::find()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_asc(table::Column::Name)
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| PublishedView::new(row.name, row.statement))
            .collect())
    }

    /// Define `name` as `statement`, replacing the view it named before. A
    /// statement Spiral cannot plan is refused, so every published view reads.
    pub async fn replace(&self, name: &ViewName, statement: &str) -> Result<(), String> {
        use crate::studio::curated_views as table;
        use sea_orm::{EntityTrait, Set, sea_query::OnConflict};
        self.spiral
            .connect()
            .await?
            .query_rows(&format!("SELECT * FROM ({statement}) {name} LIMIT 0"), 1)
            .await
            .map_err(|error| format!("“{name}” cannot be read: {error}"))?;
        let view = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            name: Set(name.as_str().to_owned()),
            statement: Set(statement.to_owned()),
        };
        table::Entity::insert(view)
            .on_conflict(
                OnConflict::columns([table::Column::Project, table::Column::Name])
                    .update_columns([table::Column::Name, table::Column::Statement])
                    .to_owned(),
            )
            .exec(self.project_store.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Forget the view `name`; one already gone is forgotten too.
    pub async fn drop(&self, name: &ViewName) -> Result<(), String> {
        use crate::studio::curated_views as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        table::Entity::delete_many()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .filter(table::Column::Name.eq(name.as_str()))
            .exec(self.project_store.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests;
