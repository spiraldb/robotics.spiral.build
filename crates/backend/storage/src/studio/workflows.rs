use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "workflows")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) revision: i64,
    pub(crate) document: Json,
    pub updated_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
