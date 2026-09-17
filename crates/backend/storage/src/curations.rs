//! One row per saved curation recipe. The list reads metadata; the recipe
//! itself is loaded by identity and saved only on explicit Save.

use spiral_robotics_common::curation::{CurationSpec, CurationSummary};

use crate::db::StorageClock;

/// The stored shape of one recipe, versioned independently of the wire type.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredCurationDocument {
    version: u32,
    curation: CurationSpec,
}

impl StoredCurationDocument {
    pub const CURRENT_VERSION: u32 = 1;

    pub fn new(curation: CurationSpec) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            curation,
        }
    }
    pub const fn version(&self) -> u32 {
        self.version
    }
    pub const fn curation(&self) -> &CurationSpec {
        &self.curation
    }
    pub fn into_curation(self) -> CurationSpec {
        self.curation
    }
    pub(crate) fn validate(&self, row: &crate::studio::curations::Model) -> Result<(), String> {
        if self.version != Self::CURRENT_VERSION {
            return Err(format!(
                "Curation document version {} is not supported",
                self.version
            ));
        }
        if self.curation.id() != row.id
            || self.curation.name() != row.name
            || self.curation.revision() != row.revision as u64
            || row.revision < 1
            || row.id.is_empty()
            || row.id.len() > 128
            || row.name.trim().is_empty()
        {
            return Err(
                "Curation document identity, name, or revision disagrees with its row".into(),
            );
        }
        Ok(())
    }
}

pub(crate) struct CurationPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> CurationPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    pub(crate) async fn list(&self) -> Result<Vec<CurationSummary>, String> {
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

    pub(crate) async fn load(
        &self,
        curation_id: &str,
    ) -> Result<Option<StoredCurationDocument>, String> {
        use crate::studio::curations as table;
        use sea_orm::EntityTrait;
        let row = table::Entity::find_by_id((
            self.project_store.key().to_owned(),
            curation_id.to_owned(),
        ))
        .one(self.project_store.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        row.map(|row| serde_json::from_value(row.document).map_err(|error| error.to_string()))
            .transpose()
    }

    /// Revision 1 inserts; revision n replaces revision n-1 and nothing else.
    pub(crate) async fn save(&self, document: &StoredCurationDocument) -> Result<bool, String> {
        use crate::studio::curations as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
        let curation = document.curation();
        let revision = i64::try_from(curation.revision())
            .map_err(|_| "Curation revision exceeds SQLite integer range")?;
        if revision < 1 {
            return Ok(false);
        }
        let model = table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(curation.id().to_owned()),
            name: Set(curation.name().to_owned()),
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
                .filter(table::Column::Id.eq(curation.id()))
                .filter(table::Column::Revision.eq(revision - 1))
                .exec(database)
                .await
                .map_err(|error| error.to_string())?;
            Ok(result.rows_affected == 1)
        }
    }
}
