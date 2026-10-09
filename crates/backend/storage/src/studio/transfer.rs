//! Native SQLite snapshots contain one project's records and no backend secrets.
use super::{
    StudioProject, curations, dataset_check_reports, dataset_joint_roles, dataset_robots,
    published_models, studio_settings, training_runs,
};
use sea_orm::{
    ActiveModelTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection,
    DatabaseTransaction, DbBackend, EntityTrait, IntoActiveModel, Statement, TransactionTrait,
};
use tempfile::NamedTempFile;

pub(crate) struct StudioSnapshot {
    file: NamedTempFile,
    database: DatabaseConnection,
}

impl StudioSnapshot {
    pub(crate) async fn capture(project: &StudioProject) -> Result<Self, String> {
        let file = NamedTempFile::new().map_err(|error| error.to_string())?;
        let destination =
            url::Url::from_file_path(file.path()).map_err(|_| "invalid snapshot path")?;
        // VACUUM INTO takes a consistent native snapshot, including committed WAL data.
        project
            .connection()
            .await?
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "VACUUM INTO ?",
                [format!("{destination}?mode=rwc").into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let database = Self::connect(&file, false).await?;
        let snapshot = Self { file, database };
        snapshot
            .database
            .execute_unprepared("PRAGMA journal_mode=DELETE")
            .await
            .map_err(|error| error.to_string())?;
        let transaction = snapshot
            .database
            .begin()
            .await
            .map_err(|error| error.to_string())?;
        // No browser's Spiral sign-in leaves this backend, and no robot
        // connector's token, even hashed.
        for credentials in ["sessions", "robot_connector_tokens"] {
            transaction
                .execute_unprepared(&format!("DELETE FROM {credentials}"))
                .await
                .map_err(|error| error.to_string())?;
        }
        // A bundle carries no model answers, so no view that reads them.
        transaction
            .execute_unprepared("DELETE FROM curated_views")
            .await
            .map_err(|error| error.to_string())?;
        // Workspaces are this backend's names for projects; an import lands
        // in whichever workspace receives it.
        transaction
            .execute_unprepared("DELETE FROM workspaces")
            .await
            .map_err(|error| error.to_string())?;
        for table in StudioProject::RECORD_TABLES {
            transaction
                .execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    format!("DELETE FROM {table} WHERE project <> ?"),
                    [project.key().into()],
                ))
                .await
                .map_err(|error| error.to_string())?;
            transaction
                .execute_unprepared(&format!("UPDATE {table} SET project = ''"))
                .await
                .map_err(|error| error.to_string())?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        // Rebuild pages so deleted credentials and other projects cannot remain in free space.
        snapshot
            .database
            .execute_unprepared("VACUUM")
            .await
            .map_err(|error| error.to_string())?;
        Ok(snapshot)
    }

    pub(crate) async fn open(bytes: &[u8], project: &StudioProject) -> Result<Self, String> {
        use std::io::Write;
        let mut file = NamedTempFile::new().map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        let database = Self::connect(&file, true).await?;
        let snapshot = Self { file, database };
        // Never execute schema or migration scripts from an uploaded database.
        if Self::schema(&snapshot.database).await?
            != Self::schema(project.connection().await?).await?
        {
            return Err("Studio snapshot uses an unsupported SQLite schema".into());
        }
        let check = snapshot
            .database
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA integrity_check",
            ))
            .await
            .map_err(|error| error.to_string())?
            .ok_or("SQLite integrity check returned no result")?;
        if check
            .try_get::<String>("", "integrity_check")
            .map_err(|error| error.to_string())?
            != "ok"
        {
            return Err("Studio snapshot failed SQLite integrity checking".into());
        }
        let sessions = snapshot
            .database
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT count(*) AS count FROM sessions",
            ))
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Missing Spiral sign-ins count")?;
        if sessions
            .try_get::<i64>("", "count")
            .map_err(|error| error.to_string())?
            != 0
        {
            return Err("Studio snapshot must not contain Spiral sign-ins".into());
        }
        for table in StudioProject::RECORD_TABLES {
            let row = snapshot
                .database
                .query_one(Statement::from_string(
                    DbBackend::Sqlite,
                    format!("SELECT project FROM {table} WHERE project <> '' LIMIT 1"),
                ))
                .await
                .map_err(|error| error.to_string())?;
            if row.is_some() {
                return Err(
                    "Studio snapshot contains records outside its portable project scope".into(),
                );
            }
        }
        snapshot.validate_documents().await?;
        Ok(snapshot)
    }

    async fn connect(file: &NamedTempFile, read_only: bool) -> Result<DatabaseConnection, String> {
        let url = url::Url::from_file_path(file.path()).map_err(|_| "invalid snapshot path")?;
        let mut options = ConnectOptions::new(format!(
            "sqlite://{}?mode={}",
            &url.as_str()[7..],
            if read_only { "ro" } else { "rw" }
        ));
        options
            .sqlx_logging(false)
            .map_sqlx_sqlite_opts(|options| options.pragma("trusted_schema", "OFF"));
        Database::connect(options)
            .await
            .map_err(|error| error.to_string())
    }

    async fn schema(database: &DatabaseConnection) -> Result<Vec<(String, String)>, String> {
        database
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name",
            ))
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "name").map_err(|error| error.to_string())?,
                    row.try_get("", "sql").map_err(|error| error.to_string())?,
                ))
            })
            .collect()
    }

    pub(crate) fn bytes(&self) -> Result<Vec<u8>, String> {
        let size = self
            .file
            .as_file()
            .metadata()
            .map_err(|error| error.to_string())?
            .len();
        if size > crate::bundle::ProjectBundle::MAX_BYTES as u64 {
            return Err("Studio snapshot exceeds the project bundle size limit".into());
        }
        std::fs::read(self.file.path()).map_err(|error| error.to_string())
    }

    async fn validate_documents(&self) -> Result<(), String> {
        for row in dataset_robots::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            let robot: Option<spiral_robotics_common::datasets::DatasetRobot> =
                serde_json::from_value(row.document).map_err(|error| error.to_string())?;
            if let Some(robot) = robot {
                robot.validate()?;
            }
        }
        for row in dataset_joint_roles::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.roles()?;
        }
        for row in curations::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            crate::curations::CurationPersistence::document(row)?;
        }
        for row in training_runs::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            crate::training::TrainingRunPersistence::validate_record(&row.id, row.document)?;
        }
        for row in studio_settings::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            serde_json::from_value::<crate::StoredStudioPreferences>(row.document)
                .map_err(|error| error.to_string())?
                .validated()?;
        }
        Ok(())
    }

    /// Stage replacements before Spiral writes; the caller commits only after Spiral succeeds.
    pub(crate) async fn stage(
        &self,
        project: &StudioProject,
    ) -> Result<DatabaseTransaction, String> {
        let transaction = project
            .connection()
            .await?
            .begin()
            .await
            .map_err(|error| error.to_string())?;
        // The restore replaces every dataset, and with it the answers the
        // project's published views read.
        project.delete_records(&transaction).await?;
        for mut row in dataset_robots::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in dataset_joint_roles::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in dataset_check_reports::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in curations::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in training_runs::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in published_models::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for mut row in studio_settings::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())?
        {
            row.project = project.key().to_owned();
            row.into_active_model()
                .insert(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(transaction)
    }
}

#[cfg(test)]
mod tests;
