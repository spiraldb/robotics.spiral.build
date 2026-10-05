use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "workspaces")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) endpoint: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) created_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
