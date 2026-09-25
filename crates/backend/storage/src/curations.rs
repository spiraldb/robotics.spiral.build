//! One row per curation. The list reads identity and revision; the payload
//! is loaded by identity and replaced only on an explicit Save.
//!
//! Storage never parses the payload. Its only rules are the ones
//! [`CurationDocument::validate`] states plus the revision guard below, so a
//! change to the frontend's document schema never needs a migration here.
//!
//! A published output is a view at the Spiral catalog root, replaced whole
//! on every publish; [`CuratedViews`] owns that DDL.

use spiral_robotics_common::curation::{CurationDocument, CurationSummary, ViewName};
use spiral_robotics_common::datasets::ProjectResource;

use crate::db::StorageClock;
use crate::{ProjectConnection, ProjectStore};

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
                    u64::try_from(revision).map_err(|_| "Curation revision is negative")?;
                Ok(CurationSummary::new(id, name, revision))
            })
            .collect()
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
}

/// The views curation outputs are published as, at the catalog root.
pub struct CuratedViews<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> CuratedViews<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    /// The kind of resource other than a view that already holds `name` at
    /// the catalog root: a table or a Dataset, which a publish must not
    /// replace.
    pub async fn holder(&self, name: &ViewName) -> Result<Option<String>, String> {
        let resources = self.project_store.dataset_catalog().resources().await?;
        Ok(Self::held_by(&resources, name))
    }

    fn held_by(resources: &[ProjectResource], name: &ViewName) -> Option<String> {
        resources
            .iter()
            .filter(|resource| matches!(resource.kind(), "table" | "dataset"))
            .find(|resource| {
                matches!(resource.selection().path(), [root] if root.eq_ignore_ascii_case(name.as_str()))
            })
            .map(|resource| resource.kind().to_owned())
    }

    /// Define `name` as `statement`, replacing the view it named before.
    pub async fn replace(&self, name: &ViewName, statement: &str) -> Result<(), String> {
        Self::define(
            &mut self.project_store.connect_exclusive().await?,
            name,
            statement,
        )
        .await
    }

    /// One statement, so a reader sees the old view or the new one. Spiral
    /// refuses a `DROP VIEW` and a `CREATE VIEW` of one name in one
    /// transaction as a namespace conflict once the view exists.
    async fn define(
        connection: &mut ProjectConnection,
        name: &ViewName,
        statement: &str,
    ) -> Result<(), String> {
        connection
            .execute_update_sql(&format!("CREATE OR REPLACE VIEW {name} AS {statement}"))
            .await
    }

    pub async fn drop(&self, name: &ViewName) -> Result<(), String> {
        self.project_store
            .connect_exclusive()
            .await?
            .execute_update_sql(&format!("DROP VIEW IF EXISTS {name}"))
            .await
    }
}

#[cfg(test)]
mod tests;
