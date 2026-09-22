use sea_orm::entity::prelude::*;
use spiral_robotics_common::robots::config::RobotConfig;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "robots")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) id: String,
    pub(super) document: Json,
    /// Increments on every write, so a save can name the version it replaces.
    pub(super) revision: i64,
    pub updated_ms: i64,
}

impl Model {
    /// Decode the saved configuration, which must name the robot its row is keyed by.
    pub(super) fn into_config(self) -> Result<(RobotConfig, u64), String> {
        let config = serde_json::from_value::<RobotConfig>(self.document).map_err(|error| {
            format!("robot {:?} has an invalid configuration: {error}", self.id)
        })?;
        if config.id() != self.id {
            return Err(format!(
                "robot {:?} is stored under the ID {:?}",
                config.id(),
                self.id
            ));
        }
        Ok((config, self.revision.unsigned_abs()))
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
