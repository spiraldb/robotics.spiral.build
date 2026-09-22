use super::{StudioProject, robots};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use spiral_robotics_common::robots::config::RobotConfig;

impl StudioProject {
    /// Every saved robot with the revision it was read at.
    pub async fn robot_configs(&self) -> Result<Vec<(RobotConfig, u64)>, String> {
        robots::Entity::find()
            .filter(robots::Column::Project.eq(self.key()))
            .order_by_asc(robots::Column::Id)
            .all(self.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(robots::Model::into_config)
            .collect()
    }

    pub async fn robot_config(&self, id: &str) -> Result<Option<(RobotConfig, u64)>, String> {
        robots::Entity::find_by_id((self.key().to_owned(), id.to_owned()))
            .one(self.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .map(robots::Model::into_config)
            .transpose()
    }

    /// Write `config` over `expected_revision`, where `None` creates the robot.
    ///
    /// Returns the new revision, or `None` when another write landed first.
    pub async fn save_robot_config(
        &self,
        config: &RobotConfig,
        expected_revision: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let database = self.connection().await?;
        let expected = expected_revision
            .map(i64::try_from)
            .transpose()
            .map_err(|_| "Robot revision is out of range")?;
        let revision = expected.map_or(1, |expected| expected + 1);
        let model = robots::ActiveModel {
            project: Set(self.key().to_owned()),
            id: Set(config.id().to_owned()),
            document: Set(serde_json::to_value(config).map_err(|error| error.to_string())?),
            revision: Set(revision),
            updated_ms: Set(crate::db::StorageClock::unix_millis()),
        };
        let written = match expected {
            None => {
                let result = robots::Entity::insert(model)
                    .on_conflict(
                        sea_orm::sea_query::OnConflict::columns([
                            robots::Column::Project,
                            robots::Column::Id,
                        ])
                        .do_nothing()
                        .to_owned(),
                    )
                    .do_nothing()
                    .exec(database)
                    .await
                    .map_err(|error| error.to_string())?;
                matches!(result, sea_orm::TryInsertResult::Inserted(_))
            }
            Some(expected) => {
                let result = robots::Entity::update_many()
                    .set(model)
                    .filter(robots::Column::Project.eq(self.key()))
                    .filter(robots::Column::Id.eq(config.id()))
                    .filter(robots::Column::Revision.eq(expected))
                    .exec(database)
                    .await
                    .map_err(|error| error.to_string())?;
                result.rows_affected == 1
            }
        };
        Ok(written.then_some(revision.unsigned_abs()))
    }

    pub async fn delete_robot_config(&self, id: &str) -> Result<bool, String> {
        robots::Entity::delete_by_id((self.key().to_owned(), id.to_owned()))
            .exec(self.connection().await?)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| error.to_string())
    }
}
