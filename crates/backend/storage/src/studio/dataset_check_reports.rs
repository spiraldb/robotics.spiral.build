//! Each dataset version's check report, as the browser's checks at one
//! definition computed it. A newer definition reads as no report, so changed
//! checks run again.
use sea_orm::entity::prelude::*;
use sea_orm::{Set, sea_query::OnConflict};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "dataset_check_reports")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) dataset: String,
    pub(super) version: String,
    pub(super) definition: i64,
    pub(super) document: Json,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}

impl super::StudioProject {
    /// The report checks at `definition` computed for dataset version
    /// `identity`, if they have.
    pub(crate) async fn dataset_check_report(
        &self,
        identity: (&str, &str),
        definition: u32,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(
            Entity::find_by_id((self.key().to_owned(), identity.0.to_owned()))
                .filter(Column::Version.eq(identity.1))
                .filter(Column::Definition.eq(i64::from(definition)))
                .one(self.connection().await?)
                .await
                .map_err(|error| error.to_string())?
                .map(|row| row.document),
        )
    }
    pub(crate) async fn save_dataset_check_report(
        &self,
        identity: (&str, &str),
        definition: u32,
        report: serde_json::Value,
    ) -> Result<(), String> {
        Entity::insert(ActiveModel {
            project: Set(self.key().to_owned()),
            dataset: Set(identity.0.to_owned()),
            version: Set(identity.1.to_owned()),
            definition: Set(i64::from(definition)),
            document: Set(report),
        })
        .on_conflict(
            OnConflict::columns([Column::Project, Column::Dataset])
                .update_columns([Column::Version, Column::Definition, Column::Document])
                .to_owned(),
        )
        .exec(self.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
