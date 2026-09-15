use super::{StudioProject, robots};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

/// Validated by the robot feature before persistence; SQLite owns identity and CAS.
pub struct RobotDocumentWrite {
    id: String,
    document: serde_json::Value,
    expected: Option<serde_json::Value>,
}

impl RobotDocumentWrite {
    pub fn new(
        id: String,
        document: serde_json::Value,
        expected: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id,
            document,
            expected,
        }
    }
}

impl StudioProject {
    pub async fn robot_documents(&self) -> Result<Vec<(String, serde_json::Value)>, String> {
        robots::Entity::find()
            .filter(robots::Column::Project.eq(self.key()))
            .order_by_asc(robots::Column::Id)
            .all(self.connection().await?)
            .await
            .map(|rows| rows.into_iter().map(|row| (row.id, row.document)).collect())
            .map_err(|error| error.to_string())
    }

    pub async fn robot_document(&self, id: &str) -> Result<Option<serde_json::Value>, String> {
        robots::Entity::find_by_id((self.key().to_owned(), id.to_owned()))
            .one(self.connection().await?)
            .await
            .map(|row| row.map(|row| row.document))
            .map_err(|error| error.to_string())
    }

    pub async fn save_robot_document(&self, write: RobotDocumentWrite) -> Result<bool, String> {
        let database = self.connection().await?;
        let model = robots::ActiveModel {
            project: Set(self.key().to_owned()),
            id: Set(write.id.clone()),
            document: Set(write.document),
            updated_ms: Set(crate::db::StorageClock::unix_millis()),
        };
        match write.expected {
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
                Ok(matches!(result, sea_orm::TryInsertResult::Inserted(_)))
            }
            Some(expected) => {
                let result = robots::Entity::update_many()
                    .set(model)
                    .filter(robots::Column::Project.eq(self.key()))
                    .filter(robots::Column::Id.eq(write.id))
                    .filter(robots::Column::Document.eq(expected))
                    .exec(database)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(result.rows_affected == 1)
            }
        }
    }

    pub async fn delete_robot_document(&self, id: &str) -> Result<bool, String> {
        robots::Entity::delete_by_id((self.key().to_owned(), id.to_owned()))
            .exec(self.connection().await?)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| error.to_string())
    }
}
