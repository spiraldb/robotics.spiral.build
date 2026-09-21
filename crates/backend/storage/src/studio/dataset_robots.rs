//! Dataset visualization results, including a remembered absence of a model.
use sea_orm::entity::prelude::*;
use sea_orm::{Set, sea_query::OnConflict};
use spiral_robotics_common::datasets::DatasetRobot;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "dataset_robots")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) dataset: String,
    pub(super) version: String,
    pub(super) document: Json,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}

impl super::StudioProject {
    pub(crate) async fn dataset_robot(
        &self,
        identity: (&str, &str),
    ) -> Result<Option<DatasetRobot>, String> {
        Entity::find_by_id((self.key().to_owned(), identity.0.to_owned()))
            .filter(Column::Version.eq(identity.1))
            .one(self.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .map(|row| serde_json::from_value(row.document).map_err(|error| error.to_string()))
            .unwrap_or(Ok(None))
    }
    pub(crate) async fn save_dataset_robot(
        &self,
        identity: (&str, &str),
        robot: Option<&DatasetRobot>,
    ) -> Result<(), String> {
        if let Some(robot) = robot {
            robot.validate()?;
        }
        Entity::insert(ActiveModel {
            project: Set(self.key().to_owned()),
            dataset: Set(identity.0.to_owned()),
            version: Set(identity.1.to_owned()),
            document: Set(serde_json::to_value(robot).map_err(|error| error.to_string())?),
        })
        .on_conflict(
            OnConflict::columns([Column::Project, Column::Dataset])
                .update_columns([Column::Version, Column::Document])
                .to_owned(),
        )
        .exec(self.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
