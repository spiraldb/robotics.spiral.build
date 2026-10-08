use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "published_models")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) camera: String,
    pub(crate) run_id: String,
    pub(crate) step: i64,
    pub(crate) created_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
