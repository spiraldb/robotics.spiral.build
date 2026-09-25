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
            Box::new(CreateBuildDocuments),
            Box::new(RenameToExplorations),
            Box::new(KeepRolloutSessions),
            Box::new(TypedRobotConfigs),
            Box::new(WorkerOwnedTraining),
            Box::new(CurationGraphs),
            Box::new(RetireWorkflowAndOldCurations),
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

struct CreateBuildDocuments;
impl MigrationName for CreateBuildDocuments {
    fn name(&self) -> &str {
        "m20260921_000005_build_documents"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for CreateBuildDocuments {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260921_000005_build_documents.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

/// `build_documents` keeps its rows and takes the name of the view that owns
/// them; the old migration above still creates the table under its old name.
struct RenameToExplorations;
impl MigrationName for RenameToExplorations {
    fn name(&self) -> &str {
        "m20260921_000006_explorations"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for RenameToExplorations {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260921_000006_explorations.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

struct KeepRolloutSessions;
impl MigrationName for KeepRolloutSessions {
    fn name(&self) -> &str {
        "m20260922_000007_rollout_sessions"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for KeepRolloutSessions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260922_000007_rollout_sessions.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

/// Robot rows move to the typed configuration shape and gain a save revision.
struct TypedRobotConfigs;
impl MigrationName for TypedRobotConfigs {
    fn name(&self) -> &str {
        "m20260922_000008_typed_robot_configs"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for TypedRobotConfigs {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260922_000008_typed_robot_configs.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

/// Training runs shrink to Studio's record of them; rollout sessions are no
/// longer stored.
struct WorkerOwnedTraining;
impl MigrationName for WorkerOwnedTraining {
    fn name(&self) -> &str {
        "m20260922_000009_worker_owned_training"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for WorkerOwnedTraining {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260922_000009_worker_owned_training.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

/// Explore's graphs become the curations, replacing the recipes Curation kept.
struct CurationGraphs;
impl MigrationName for CurationGraphs {
    fn name(&self) -> &str {
        "m20260923_000010_curation_graphs"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for CurationGraphs {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260923_000010_curation_graphs.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}

/// Workflow is removed; Curation is the one authoring surface. Curations
/// saved in the retired encoding go with it.
struct RetireWorkflowAndOldCurations;
impl MigrationName for RetireWorkflowAndOldCurations {
    fn name(&self) -> &str {
        "m20260924_000011_drop_workflows_and_old_curations"
    }
}
#[async_trait::async_trait]
impl MigrationTrait for RetireWorkflowAndOldCurations {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(include_str!("m20260924_000011_drop_workflows_and_old_curations.sql"))
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "Studio schema downgrades require restoring a backup".into(),
        ))
    }
}
