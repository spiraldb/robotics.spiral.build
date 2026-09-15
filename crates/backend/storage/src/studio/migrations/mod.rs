use sea_orm_migration::prelude::*;

pub(super) struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(InitialStudioSchema),
            Box::new(RemoveLegacyRegistry),
        ]
    }
}

struct InitialStudioSchema;

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
