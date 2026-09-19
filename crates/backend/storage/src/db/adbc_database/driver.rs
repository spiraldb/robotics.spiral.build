use std::path::PathBuf;

use adbc_core::{
    Driver as _,
    options::{AdbcVersion, OptionDatabase, OptionValue},
};
use adbc_driver_manager::{ManagedDatabase, ManagedDriver};

use super::{DatabaseError, DatabaseResult};
use crate::db::DatabaseErrorKind;
use crate::settings::SpiralConnectionSettings;

#[derive(Clone)]
pub(super) struct DriverConfiguration {
    options: Vec<(OptionDatabase, OptionValue)>,
}

impl DriverConfiguration {
    pub(super) fn hosted(settings: SpiralConnectionSettings) -> Self {
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
        if let Some(credentials) = settings.credentials() {
            options.push((OptionDatabase::Username, credentials.client_id().into()));
            options.push((OptionDatabase::Password, credentials.client_secret().into()));
        }
        if let Some(token) = settings.huggingface_token() {
            options.push((
                OptionDatabase::Other("spiral.huggingface_token".into()),
                token.into(),
            ));
        }
        Self { options }
    }

    pub(super) fn load(self) -> DatabaseResult<LoadedDatabase> {
        let path = Self::library_path()?;
        let mut driver = ManagedDriver::load_dynamic_from_filename(
            &path, Some(b"AdbcDriverSpiraldbInit"), AdbcVersion::V110,
        ).map_err(|error| DatabaseError::new(DatabaseErrorKind::Unavailable,
            format!("loading Spiral ADBC driver at {}: {error}; install the pinned driver and set SPIRALDB_ADBC_DRIVER", path.display())))?;
        let database = driver.new_database_with_opts(self.options)?;
        Ok(LoadedDatabase::new(database))
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
    use adbc_core::Optionable as _;

    #[test]
    fn huggingface_token_reaches_the_native_driver_as_a_write_only_option() {
        let settings =
            SpiralConnectionSettings::new("http://127.0.0.1:1".into(), "http://127.0.0.1:1".into())
                .unwrap()
                .with_huggingface_token(Some("hf_test_only".into()));
        let database = DriverConfiguration::hosted(settings).load().unwrap();
        let error = database
            .database()
            .get_option_string(OptionDatabase::Other("spiral.huggingface_token".into()))
            .unwrap_err();
        assert_eq!(error.status, adbc_core::error::Status::InvalidState);
        assert!(error.message.contains("write-only"));
        assert!(!error.message.contains("hf_test_only"));
    }
}
