use sea_orm_migration::prelude::*;

pub(super) struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(SqlMigration::new(
                "m20260928_000001_studio",
                include_str!("m20260928_000001_studio.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261001_000001_curated_views",
                include_str!("m20261001_000001_curated_views.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261005_000001_sessions",
                include_str!("m20261005_000001_sessions.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261005_000002_provider_keys",
                include_str!("m20261005_000002_provider_keys.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261005_000003_robot_connector",
                include_str!("m20261005_000003_robot_connector.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261005_000004_workspaces",
                include_str!("m20261005_000004_workspaces.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261008_000001_curations_encoding_5",
                include_str!("m20261008_000001_curations_encoding_5.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261008_000002_training_models",
                include_str!("m20261008_000002_training_models.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261008_000003_published_models",
                include_str!("m20261008_000003_published_models.sql"),
            )),
            Box::new(SqlMigration::new(
                "m20261009_000001_robot_connector_tokens",
                include_str!("m20261009_000001_robot_connector_tokens.sql"),
            )),
        ]
    }
}

/// One forward-only schema step, written as SQL.
struct SqlMigration {
    name: &'static str,
    sql: &'static str,
}
impl SqlMigration {
    const fn new(name: &'static str, sql: &'static str) -> Self {
        Self { name, sql }
    }
}
impl MigrationName for SqlMigration {
    fn name(&self) -> &str {
        self.name
    }
}

#[async_trait::async_trait]
impl MigrationTrait for SqlMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(self.sql)
            .await?;
        Ok(())
    }

    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}
