use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "robots")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub document: Json,
    pub updated_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
