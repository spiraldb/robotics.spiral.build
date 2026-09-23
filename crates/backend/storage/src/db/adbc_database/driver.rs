use std::path::PathBuf;
use std::sync::{Arc, Weak};

use adbc_core::{
    Driver as _, Optionable as _,
    options::{AdbcVersion, OptionDatabase, OptionValue},
};
use adbc_driver_manager::{ManagedDatabase, ManagedDriver};

use super::{DatabaseError, DatabaseResult};
use crate::db::DatabaseErrorKind;
use crate::{SpiralAccess, SpiralConnection};

/// The driver option carrying the signed-in user's token; the driver never refreshes it.
const ACCESS_TOKEN_OPTION: &str = "spiral.access_token";

#[derive(Clone)]
pub(super) struct DriverConfiguration {
    options: Vec<(OptionDatabase, OptionValue)>,
    /// Hosted databases authenticate as the signed-in user.
    access: Option<SpiralAccess>,
}

impl DriverConfiguration {
    pub(super) fn hosted(connection: SpiralConnection) -> Self {
        let settings = connection.settings();
        let mut uri = url::Url::parse("spiraldb://").expect("constant driver URI");
        if let Some(project) = settings.project_id() {
            uri.query_pairs_mut().append_pair("project", project);
        }
        let mut options = vec![
            (OptionDatabase::Uri, uri.to_string().into()),
            (
                OptionDatabase::Other("spiral.local_cache".into()),
                "true".into(),
            ),
            (
                OptionDatabase::Other("spiral.api_url".into()),
                settings.endpoint().into(),
            ),
            (
                OptionDatabase::Other("spiral.spfs_url".into()),
                settings.spfs_endpoint().into(),
            ),
        ];
        if let Some(token) = settings.huggingface_token() {
            options.push((
                OptionDatabase::Other("spiral.huggingface_token".into()),
                token.into(),
            ));
        }
        Self {
            options,
            access: Some(connection.access().clone()),
        }
    }

    /// Open the database as the user signed in now, and give it every later
    /// token so long-running work outlives each token's short lifetime.
    pub(super) fn load(self) -> DatabaseResult<Arc<LoadedDatabase>> {
        let mut options = self.options;
        // Follow before reading, so a token published in between still arrives.
        let follower = self.access.clone();
        if let Some(access) = &self.access {
            let token = access
                .token()
                .map_err(|message| DatabaseError::new(DatabaseErrorKind::InvalidState, message))?;
            options.push((
                OptionDatabase::Other(ACCESS_TOKEN_OPTION.into()),
                token.into(),
            ));
        }
        let path = Self::library_path()?;
        let mut driver = ManagedDriver::load_dynamic_from_filename(
            &path, Some(b"AdbcDriverSpiraldbInit"), AdbcVersion::V110,
        ).map_err(|error| DatabaseError::new(DatabaseErrorKind::Unavailable,
            format!("loading Spiral ADBC driver at {}: {error}; install the pinned driver and set SPIRALDB_ADBC_DRIVER", path.display())))?;
        let database = Arc::new(LoadedDatabase::new(driver.new_database_with_opts(options)?));
        if let Some(access) = follower {
            LoadedDatabase::follow(Arc::downgrade(&database), access);
        }
        Ok(database)
    }

    fn library_path() -> DatabaseResult<PathBuf> {
        if let Some(path) = std::env::var_os("SPIRALDB_ADBC_DRIVER") {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(DatabaseError::invalid_input(
                    "SPIRALDB_ADBC_DRIVER must be an absolute library path",
                ));
            }
            return Ok(path);
        }
        let executable = std::env::current_exe().map_err(DatabaseError::from_io)?;
        let filename = format!(
            "{}adbc_driver_spiraldb{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );
        Ok(executable.with_file_name(filename))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn memory() -> Self {
        Self {
            options: vec![(OptionDatabase::Uri, "spiral:memory:".into())],
            access: None,
        }
    }
}

/// All database releases, including initialization cancelled by its caller,
/// run off the async executor. Result buffers also retain this owner.
pub(super) struct LoadedDatabase {
    database: Option<ManagedDatabase>,
}

impl LoadedDatabase {
    fn new(database: ManagedDatabase) -> Self {
        Self {
            database: Some(database),
        }
    }

    pub(super) fn database(&self) -> &ManagedDatabase {
        self.database
            .as_ref()
            .expect("database is only taken during drop")
    }

    /// Replace the database's token with each one the account publishes, until
    /// the database is released.
    fn follow(database: Weak<Self>, mut access: SpiralAccess) {
        tokio::spawn(async move {
            while let Some(token) = access.replaced().await {
                let Some(loaded) = database.upgrade() else {
                    return;
                };
                // Signing out retires every project; its last token lapses.
                let Some(token) = token else { continue };
                let mut managed = loaded.database().clone();
                drop(loaded);
                let replaced = tokio::task::spawn_blocking(move || {
                    managed.set_option(
                        OptionDatabase::Other(ACCESS_TOKEN_OPTION.into()),
                        token.into(),
                    )
                })
                .await;
                if let Ok(Err(error)) = replaced {
                    tracing::warn!(detail = %error.message, "could not replace the Spiral access token");
                }
            }
        });
    }
}

impl Drop for LoadedDatabase {
    fn drop(&mut self) {
        let database = self.database.take();
        // The last Arrow buffer may be released outside a Tokio runtime.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(move || drop(database));
        } else {
            drop(database);
        }
    }
}

impl From<adbc_core::error::Error> for DatabaseError {
    fn from(error: adbc_core::error::Error) -> Self {
        use adbc_core::error::Status;
        let kind = match error.status {
            Status::NotImplemented => DatabaseErrorKind::Unsupported,
            Status::NotFound => DatabaseErrorKind::NotFound,
            Status::AlreadyExists | Status::Integrity => DatabaseErrorKind::Conflict,
            Status::InvalidArguments | Status::InvalidData => DatabaseErrorKind::InvalidInput,
            Status::InvalidState => DatabaseErrorKind::InvalidState,
            Status::IO | Status::Timeout | Status::Unauthenticated | Status::Unauthorized => {
                DatabaseErrorKind::Unavailable
            }
            Status::Cancelled => DatabaseErrorKind::Cancelled,
            _ => DatabaseErrorKind::Internal,
        };
        Self::new(kind, error.message)
    }
}

impl DatabaseError {
    pub(super) fn from_worker(error: tokio::task::JoinError) -> Self {
        Self::new(
            DatabaseErrorKind::Internal,
            format!("database worker failed: {error}"),
        )
    }

    fn from_io(error: std::io::Error) -> Self {
        Self::new(DatabaseErrorKind::Unavailable, error.to_string())
    }

    pub(super) fn from_arrow(error: arrow_schema::ArrowError) -> Self {
        if let arrow_schema::ArrowError::ExternalError(source) = &error {
            if let Some(error) = source.downcast_ref::<Self>() {
                return error.clone();
            }
            if let Some(error) = source.downcast_ref::<adbc_core::error::Error>() {
                return error.clone().into();
            }
        }
        Self::new(DatabaseErrorKind::Internal, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> crate::SpiralConnectionSettings {
        crate::SpiralConnectionSettings::new(
            "http://127.0.0.1:1".into(),
            "http://127.0.0.1:1".into(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn huggingface_token_reaches_the_native_driver_as_a_write_only_option() {
        let settings = settings().with_huggingface_token(Some("hf_test_only".into()));
        let connection = SpiralConnection::new(settings, SpiralAccess::fixed("token".into()));
        let database = DriverConfiguration::hosted(connection).load().unwrap();
        let error = database
            .database()
            .get_option_string(OptionDatabase::Other("spiral.huggingface_token".into()))
            .unwrap_err();
        assert_eq!(error.status, adbc_core::error::Status::InvalidState);
        assert!(error.message.contains("write-only"));
        assert!(!error.message.contains("hf_test_only"));
    }

    /// Spiral's API as the driver reaches it, recording each request's bearer token.
    async fn recording_api() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let recorded = seen.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = recorded.clone();
            async move {
                if let Some(value) = request.headers().get("authorization") {
                    recorded
                        .lock()
                        .unwrap()
                        .push(value.to_str().unwrap().to_owned());
                }
                axum::Json(serde_json::json!({"items": [], "next_page_token": null}))
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (endpoint, seen)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_databases_authenticate_with_each_published_access_token() {
        use crate::Database as _;
        use futures::StreamExt as _;

        let (endpoint, seen) = recording_api().await;
        let settings = crate::SpiralConnectionSettings::new(endpoint.clone(), endpoint).unwrap();
        let (publisher, access) = crate::SpiralAccessPublisher::new();
        let database = crate::AdbcDatabase::new(SpiralConnection::new(settings, access));
        let signed_out = database
            .connect()
            .await
            .err()
            .expect("hosted databases need a user");
        assert!(signed_out.to_string().contains("Sign in to Spiral"));

        let read_projects = || async {
            let mut connection = database.connect().await.unwrap();
            if let Ok(mut rows) = connection
                .query("SELECT * FROM spql.meta.projects()".into())
                .await
            {
                while rows.next().await.is_some() {}
            }
        };
        publisher.publish(Some("first".into()));
        read_projects().await;
        assert_eq!(
            seen.lock().unwrap().last().map(String::as_str),
            Some("Bearer first")
        );

        publisher.publish(Some("second".into()));
        for _ in 0..50 {
            read_projects().await;
            if seen.lock().unwrap().last().map(String::as_str) == Some("Bearer second") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!(
            "the open database kept its first token: {:?}",
            seen.lock().unwrap()
        );
    }
}
