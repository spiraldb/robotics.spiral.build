use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "staged_episode_parts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) endpoint: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) project_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) dataset: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) episode: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) start: i64,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
