use sea_orm_migration::prelude::*;

pub(super) struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(InitialStudioSchema),
            Box::new(RemoveLegacyRegistry),
            Box::new(CreateCurations),
            Box::new(CreateDatasetRobots),
        ]
    }
}

struct InitialStudioSchema;
struct CreateDatasetRobots;
impl MigrationName for CreateDatasetRobots {
    fn name(&self) -> &str {
        "m20260921_000004_dataset_robots"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for CreateDatasetRobots {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260921_000004_dataset_robots.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Restore a backup to downgrade Studio".into(),
        ))
    }
}

impl MigrationName for InitialStudioSchema {
    fn name(&self) -> &str {
        "m20260915_000001_studio"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for InitialStudioSchema {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260915_000001_studio.sql"))
            .await?;
        Ok(())
    }

    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

struct RemoveLegacyRegistry;
impl MigrationName for RemoveLegacyRegistry {
    fn name(&self) -> &str {
        "m20260915_000002_remove_legacy_registry"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for RemoveLegacyRegistry {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260915_000002_remove_legacy_registry.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

struct CreateCurations;
impl MigrationName for CreateCurations {
    fn name(&self) -> &str {
        "m20260916_000003_curations"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for CreateCurations {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260916_000003_curations.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}
