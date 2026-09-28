//! Studio's record of the training runs it started.

use spiral_robotics_common::training::TrainingRunIdentity;

/// What Studio asked a worker to do, and which worker holds the run. Every
/// other fact about the run belongs to that worker and to Spiral.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrainingRunRecord {
    identity: TrainingRunIdentity,
    runtime_endpoint: String,
}

impl TrainingRunRecord {
    pub fn new(identity: TrainingRunIdentity, runtime_endpoint: impl Into<String>) -> Self {
        Self {
            identity,
            runtime_endpoint: runtime_endpoint.into(),
        }
    }

    pub const fn identity(&self) -> &TrainingRunIdentity {
        &self.identity
    }

    pub fn runtime_endpoint(&self) -> &str {
        &self.runtime_endpoint
    }
}

/// Training-run records of one project.
pub struct TrainingRunPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> TrainingRunPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    /// Every recorded run, newest first.
    pub async fn load(&self) -> Result<Vec<TrainingRunRecord>, String> {
        use crate::studio::training_runs as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let mut records = table::Entity::find()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| serde_json::from_value::<TrainingRunRecord>(row.document))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        records.sort_by_key(|record| std::cmp::Reverse(record.identity.started_ms()));
        Ok(records)
    }

    pub async fn load_run(&self, id: &str) -> Result<Option<TrainingRunRecord>, String> {
        use crate::studio::training_runs as table;
        use sea_orm::EntityTrait;
        table::Entity::find_by_id((self.project_store.key().to_owned(), id.to_owned()))
            .one(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .map(|row| serde_json::from_value(row.document))
            .transpose()
            .map_err(|error| error.to_string())
    }

    pub async fn insert(&self, record: &TrainingRunRecord) -> Result<(), String> {
        use crate::studio::training_runs as table;
        use sea_orm::{EntityTrait, Set};
        table::Entity::insert(table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(record.identity.id().to_owned()),
            document: Set(serde_json::to_value(record).map_err(|error| error.to_string())?),
        })
        .exec(self.project_store.connection().await?)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    pub async fn delete(&self, id: &str) -> Result<(), String> {
        use crate::studio::training_runs as table;
        use sea_orm::EntityTrait;
        table::Entity::delete_by_id((self.project_store.key().to_owned(), id.to_owned()))
            .exec(self.project_store.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn validate_record(id: &str, document: serde_json::Value) -> Result<(), String> {
        let record: TrainingRunRecord =
            serde_json::from_value(document).map_err(|error| error.to_string())?;
        if record.identity.id() != id || id.is_empty() {
            return Err("Training document identity disagrees with its row".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use spiral_robotics_common::training::TrainingConfig;

    #[tokio::test]
    async fn records_load_newest_first_and_delete_by_id() {
        let store = crate::StudioStore::in_memory();
        let connection = crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("training")
        .unwrap();
        let project = store.project(&connection);
        let runs = TrainingRunPersistence::new(&project);
        for (id, started_ms) in [("older", 1), ("newer", 2)] {
            let identity = TrainingRunIdentity::new(id, TrainingConfig::default(), started_ms);
            runs.insert(&TrainingRunRecord::new(identity, "http://runtime.test"))
                .await
                .unwrap();
        }

        let loaded = runs.load().await.unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|record| record.identity().id())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert_eq!(loaded[0].runtime_endpoint(), "http://runtime.test");
        assert_eq!(
            runs.load_run("older").await.unwrap(),
            Some(loaded[1].clone())
        );

        runs.delete("older").await.unwrap();
        assert_eq!(runs.load().await.unwrap().len(), 1);
        assert_eq!(runs.load_run("older").await.unwrap(), None);
    }
}
