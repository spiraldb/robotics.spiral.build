use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "robot_connector_tokens")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) endpoint: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(crate) project_id: String,
    pub(crate) token_sha256: Vec<u8>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
