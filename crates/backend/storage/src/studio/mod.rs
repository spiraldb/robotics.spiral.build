//! SQLite owns Studio state. Spiral connections never participate in local CRUD.

// SeaORM's derives require public fields/types; entities stay private to storage.
#[allow(unreachable_pub)]
mod backend_settings;
#[allow(unreachable_pub)]
pub(crate) mod curated_views;
#[allow(unreachable_pub)]
pub(crate) mod curations;
mod migrations;
#[allow(unreachable_pub)]
pub(crate) mod robots;
#[allow(unreachable_pub)]
pub(crate) mod studio_settings;
#[allow(unreachable_pub)]
pub(crate) mod training_runs;
mod transfer;
pub(crate) use transfer::StudioSnapshot;
#[allow(unreachable_pub)]
mod dataset_check_reports;
#[allow(unreachable_pub)]
mod dataset_joint_roles;
#[allow(unreachable_pub)]
mod dataset_robots;
mod robot_configs;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, EntityTrait,
    Statement, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct StudioStore {
    inner: Arc<StudioDatabase>,
}

struct StudioDatabase {
    path: Option<PathBuf>,
    connection: OnceCell<DatabaseConnection>,
}

impl StudioStore {
    pub fn at(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(StudioDatabase {
                path: Some(path),
                connection: OnceCell::new(),
            }),
        }
    }

    pub(super) fn open_default() -> Result<Self, String> {
        let path = match std::env::var_os("SPIRAL_ROBOTICS_STUDIO_DB") {
            Some(path) => PathBuf::from(path),
            // Resolve against the source repository, independent of the launch
            // directory used by Cargo, Dioxus, or the backend executable.
            None => Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(3)
                .ok_or("could not determine the backend repository directory")?
                .join(".studio")
                .join("studio.sqlite3"),
        };
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        tracing::info!(path = %path.display(), "Studio SQLite database");
        Ok(Self::at(path))
    }

    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(StudioDatabase {
                path: None,
                connection: OnceCell::new(),
            }),
        }
    }

    pub(super) async fn initialize(&self) -> Result<(), String> {
        self.connection().await.map(|_| ())
    }

    pub(crate) async fn connection(&self) -> Result<&DatabaseConnection, String> {
        self.inner
            .connection
            .get_or_try_init(|| async {
                let mut options = if let Some(path) = &self.inner.path {
                    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
                    let parent = path.parent().ok_or("Studio database path has no parent")?;
                    let mut directory = std::fs::DirBuilder::new();
                    directory.recursive(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::DirBuilderExt;
                        directory.mode(0o700);
                    }
                    directory
                        .create(parent)
                        .map_err(|error| error.to_string())?;
                    let mut file = std::fs::OpenOptions::new();
                    file.write(true).create(true).truncate(false);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        file.mode(0o600);
                    }
                    file.open(&path)
                        .map_err(|error| format!("opening {}: {error}", path.display()))?;
                    let url = url::Url::from_file_path(&path)
                        .map_err(|_| "invalid Studio database path")?;
                    ConnectOptions::new(format!("sqlite://{}?mode=rwc", &url.as_str()[7..]))
                } else {
                    let mut options = ConnectOptions::new("sqlite::memory:");
                    // An in-memory test database belongs to exactly one connection.
                    options.max_connections(1);
                    options
                };
                options.sqlx_logging(false).map_sqlx_sqlite_opts(|options| {
                    options
                        .foreign_keys(true)
                        .pragma("journal_mode", "WAL")
                        .pragma("synchronous", "FULL")
                        .busy_timeout(std::time::Duration::from_secs(5))
                });
                let database = Database::connect(options)
                    .await
                    .map_err(|error| error.to_string())?;
                // SQLite migrations and their journal are committed together. SeaORM
                // otherwise only automatically wraps PostgreSQL migrations.
                let transaction = database.begin().await.map_err(|error| error.to_string())?;
                // A database from an older schema chain is refused here, naming
                // the migrations this build no longer has.
                migrations::Migrator::up(&transaction, None)
                    .await
                    .map_err(|error| {
                        let path = self.path().unwrap_or(Path::new(":memory:"));
                        format!("Studio database {}: {error}", path.display())
                    })?;
                transaction
                    .commit()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(database)
            })
            .await
    }

    pub fn project(&self, connection: &crate::SpiralConnectionSettings) -> StudioProject {
        // Credentials and ephemeral session UUIDs are deliberately absent.
        let key = serde_json::to_string(&(
            connection.endpoint(),
            connection.project_id().unwrap_or_default(),
        ))
        .expect("strings serialize");
        StudioProject {
            store: self.clone(),
            key,
        }
    }

    pub(crate) async fn backend_settings(&self) -> Result<Option<serde_json::Value>, String> {
        backend_settings::Entity::find_by_id(1)
            .one(self.connection().await?)
            .await
            .map(|row| row.map(|row| row.document))
            .map_err(|error| error.to_string())
    }

    /// Merge `patch` into the backend settings document (RFC 7396): the
    /// fields it names are replaced, a `null` removes one, and every other
    /// field stays as saved.
    pub(crate) async fn patch_backend_settings(
        &self,
        patch: serde_json::Value,
    ) -> Result<(), String> {
        let patch = patch.to_string();
        self.connection()
            .await?
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO backend_settings (id, document) VALUES (1, json_patch('{}', ?)) \
                 ON CONFLICT (id) DO UPDATE SET document = json_patch(document, ?)",
                [patch.clone().into(), patch.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// Local records belonging to one stable Spiral destination.
#[derive(Clone)]
pub struct StudioProject {
    store: StudioStore,
    key: String,
}

impl StudioProject {
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
    pub(crate) async fn connection(&self) -> Result<&DatabaseConnection, String> {
        self.store.connection().await
    }
}

#[cfg(test)]
mod tests;
