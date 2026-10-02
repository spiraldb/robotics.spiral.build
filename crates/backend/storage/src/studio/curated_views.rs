use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "curated_views")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) name: String,
    pub(crate) statement: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
