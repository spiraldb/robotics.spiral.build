//! Backend bootstrap state and project-owned Studio preferences.
//!
//! The one local JSON document stores only the control-plane pointers needed to
//! find Spiral before Spiral can be read. Project data and preferences remain
//! in Spiral; this file is never a mirror, spool, or fallback data store.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use spiral_robotics_api::settings::{
    DEFAULT_SPIRAL_ENDPOINT, DEFAULT_SPIRAL_SPFS_ENDPOINT, TrainingRuntimeSettings, VideoSettings,
};
use url::Url;

const BACKEND_STATE_FILE: &str = "backend-state.json";

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
struct BackendState {
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    spiral_endpoint: String,
    #[serde(default)]
    spiral_spfs_endpoint: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct SpiralCredentials {
    client_id: String,
    client_secret: String,
}

impl SpiralCredentials {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

/// Validated bootstrap destination for the one Spiral project connection.
///
/// Construction keeps endpoints and project identity together so callers
/// cannot partially reconfigure a live store.
#[derive(Clone, Eq, PartialEq)]
pub struct SpiralConnectionSettings {
    endpoint: String,
    spfs_endpoint: String,
    project_id: Option<String>,
    credentials: Option<SpiralCredentials>,
}

impl SpiralConnectionSettings {
    pub fn new(endpoint: String, spfs_endpoint: String) -> Result<Self, String> {
        Ok(Self {
            endpoint: ServiceEndpoint::spiral(&endpoint)?.into_string(),
            spfs_endpoint: ServiceEndpoint::spiral_spfs(&spfs_endpoint)?.into_string(),
            project_id: None,
            credentials: None,
        })
    }

    pub fn with_project(mut self, project_id: &str) -> Result<Self, String> {
        self.project_id = Some(Self::validated_project_id(project_id)?);
        Ok(self)
    }

    pub fn with_credentials(mut self, credentials: Option<SpiralCredentials>) -> Self {
        self.credentials = credentials;
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn credentials(&self) -> Option<&SpiralCredentials> {
        self.credentials.as_ref()
    }

    fn validated_project_id(project_id: &str) -> Result<String, String> {
        let project_id = project_id.trim();
        if project_id.is_empty()
            || !project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("Spiral project must contain only letters, numbers, '_' or '-'".to_owned());
        }
        Ok(project_id.to_owned())
    }
}

/// A validated HTTP service destination used by Spiral Robotics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceEndpoint(String);

impl ServiceEndpoint {
    pub fn spiral(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Spiral API endpoint")
    }

    pub fn spiral_spfs(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Spiral SpFS endpoint")
    }

    pub fn training_runtime(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Training runtime endpoint")
    }

    pub fn environment_spiral() -> Self {
        Self(
            std::env::var("SPIRAL__SERVER__URL")
                .or_else(|_| std::env::var("SPIRALDB_API_URL"))
                .unwrap_or_else(|_| DEFAULT_SPIRAL_ENDPOINT.to_owned()),
        )
    }

    pub fn environment_spiral_spfs() -> Self {
        Self(
            std::env::var("SPIRAL__SPFS__URL")
                .unwrap_or_else(|_| DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()),
        )
    }

    pub fn into_string(self) -> String {
        self.0
    }

    fn parse(endpoint: &str, label: &str) -> Result<Self, String> {
        let endpoint = endpoint.trim();
        let parsed = Url::parse(endpoint)
            .map_err(|error| format!("{label} must be an http:// or https:// URL: {error}"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(format!(
                "{label} must be an http:// or https:// URL with a host"
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(format!("{label} must not contain embedded credentials"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(format!(
                "{label} must not contain a query string or fragment"
            ));
        }
        Ok(Self(parsed.as_str().trim_end_matches('/').to_owned()))
    }
}

/// Private backend-local bootstrap state. Future auth/session metadata may
/// live here, but durable product records must not.
pub struct BackendStateStore {
    path: PathBuf,
    current: Mutex<BackendState>,
}

impl BackendStateStore {
    pub fn open_default() -> Result<Self, String> {
        let path = std::env::var_os("SPIRAL_ROBOTICS_BACKEND_STATE")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .map(|directory| directory.join("spiral-robotics").join(BACKEND_STATE_FILE))
                    .ok_or_else(|| {
                        "could not determine the backend configuration directory".to_owned()
                    })
            })?;
        Self::open(path)
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        let current = if path.exists() {
            let bytes =
                fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("parsing {}: {error}", path.display()))?
        } else {
            BackendState::default()
        };
        Ok(Self {
            path,
            current: Mutex::new(current),
        })
    }

    pub fn project_id(&self) -> Option<String> {
        let project_id = self.current.lock_unpoisoned().project_id.trim().to_owned();
        (!project_id.is_empty()).then_some(project_id)
    }

    pub fn spiral_endpoint(&self) -> Option<String> {
        let endpoint = self
            .current
            .lock_unpoisoned()
            .spiral_endpoint
            .trim()
            .to_owned();
        (!endpoint.is_empty()).then_some(endpoint)
    }

    pub fn spiral_spfs_endpoint(&self) -> Option<String> {
        let endpoint = self
            .current
            .lock_unpoisoned()
            .spiral_spfs_endpoint
            .trim()
            .to_owned();
        (!endpoint.is_empty()).then_some(endpoint)
    }

    pub fn credentials(&self) -> Option<SpiralCredentials> {
        let current = self.current.lock_unpoisoned();
        let client_id = current.client_id.trim().to_owned();
        let client_secret = current.client_secret.clone();
        (!client_id.is_empty() && !client_secret.is_empty())
            .then_some(SpiralCredentials::new(client_id, client_secret))
    }

    pub fn save_connection(&self, settings: &SpiralConnectionSettings) -> Result<(), String> {
        let project_id = settings.project_id().ok_or_else(|| {
            "Spiral project is not configured. Choose a project in Settings → SpiralDB.".to_owned()
        })?;
        let next = BackendState {
            project_id: project_id.to_owned(),
            spiral_endpoint: settings.endpoint().to_owned(),
            spiral_spfs_endpoint: settings.spfs_endpoint().to_owned(),
            client_id: settings
                .credentials()
                .map(|credentials| credentials.client_id().to_owned())
                .unwrap_or_default(),
            client_secret: settings
                .credentials()
                .map(|credentials| credentials.client_secret().to_owned())
                .unwrap_or_default(),
        };
        next.write_private(&self.path)?;
        *self.current.lock_unpoisoned() = next;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredStudioPreferences {
    #[serde(default)]
    training: TrainingRuntimeSettings,
    #[serde(default)]
    video: VideoSettings,
}

impl StoredStudioPreferences {
    pub(crate) const fn new(training: TrainingRuntimeSettings, video: VideoSettings) -> Self {
        Self { training, video }
    }

    pub const fn training(&self) -> &TrainingRuntimeSettings {
        &self.training
    }
    pub const fn video(&self) -> &VideoSettings {
        &self.video
    }
}

pub struct StudioPreferencesStore {
    current: Mutex<StoredStudioPreferences>,
}

impl StudioPreferencesStore {
    pub fn new(current: StoredStudioPreferences) -> Self {
        Self {
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> StoredStudioPreferences {
        self.current.lock_unpoisoned().clone()
    }

    pub fn stage_update(
        &self,
        mut training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Result<StoredStudioPreferences, String> {
        let endpoint = training.endpoint().trim().to_owned();
        training.set_endpoint(ServiceEndpoint::training_runtime(&endpoint)?.into_string());
        Ok(StoredStudioPreferences::new(training, video))
    }

    pub fn replace(&self, preferences: StoredStudioPreferences) -> StoredStudioPreferences {
        let mut current = self.current.lock_unpoisoned();
        *current = preferences;
        current.clone()
    }
}

trait UnpoisonedMutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T> UnpoisonedMutex<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl BackendState {
    fn write_private(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("backend state path {} has no parent", path.display()))?;
        let mut directory = fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            directory.mode(0o700);
        }
        directory
            .create(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;

        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("encoding backend state: {error}"))?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("opening {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("writing {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("publishing {}: {error}", path.display()))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("syncing {}: {error}", parent.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_state_round_trips_only_the_spiral_bootstrap_pointers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(BACKEND_STATE_FILE);
        let store = BackendStateStore::open(path.clone()).unwrap();
        assert_eq!(store.project_id(), None);
        assert_eq!(store.spiral_endpoint(), None);
        assert_eq!(store.spiral_spfs_endpoint(), None);
        assert!(store.credentials().is_none());

        let credentials =
            SpiralCredentials::new("client-id".to_owned(), "client-secret".to_owned());
        let settings = SpiralConnectionSettings::new(
            " http://127.0.0.1:4279/ ".to_owned(),
            " http://127.0.0.1:4295/ ".to_owned(),
        )
        .unwrap()
        .with_project(" project-123 ")
        .unwrap()
        .with_credentials(Some(credentials.clone()));
        store.save_connection(&settings).unwrap();
        assert_eq!(store.project_id().as_deref(), Some("project-123"));
        assert_eq!(
            store.spiral_endpoint().as_deref(),
            Some("http://127.0.0.1:4279")
        );
        assert_eq!(
            store.spiral_spfs_endpoint().as_deref(),
            Some("http://127.0.0.1:4295")
        );
        let saved = store.credentials().expect("credentials are saved");
        assert_eq!(saved.client_id(), credentials.client_id());
        assert_eq!(saved.client_secret(), credentials.client_secret());
        let reopened = BackendStateStore::open(path).unwrap();
        assert_eq!(reopened.project_id().as_deref(), Some("project-123"));
        assert_eq!(
            reopened.spiral_endpoint().as_deref(),
            Some("http://127.0.0.1:4279")
        );
        assert_eq!(
            reopened.spiral_spfs_endpoint().as_deref(),
            Some("http://127.0.0.1:4295")
        );
        let saved = reopened.credentials().expect("credentials are reopened");
        assert_eq!(saved.client_id(), credentials.client_id());
        assert_eq!(saved.client_secret(), credentials.client_secret());
    }

    #[test]
    fn spiral_endpoint_rejects_non_http_and_embedded_credentials() {
        assert!(ServiceEndpoint::spiral("file:///tmp/spiral").is_err());
        assert!(ServiceEndpoint::spiral("https://user:secret@example.com").is_err());
        assert!(ServiceEndpoint::spiral("http://127.0.0.1:4279?debug=1").is_err());
        assert!(ServiceEndpoint::spiral_spfs("file:///tmp/spfs").is_err());
        assert!(ServiceEndpoint::spiral_spfs("https://user:secret@example.com").is_err());
    }

    #[test]
    fn project_only_backend_state_remains_readable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(BACKEND_STATE_FILE);
        fs::write(&path, br#"{"project_id":"legacy-project"}"#).unwrap();

        let store = BackendStateStore::open(path).unwrap();
        assert_eq!(store.project_id().as_deref(), Some("legacy-project"));
        assert_eq!(store.spiral_endpoint(), None);
        assert_eq!(store.spiral_spfs_endpoint(), None);
    }

    #[test]
    fn training_runtime_requires_an_http_or_https_endpoint() {
        let store = StudioPreferencesStore::new(StoredStudioPreferences::default());
        let invalid = TrainingRuntimeSettings::new("ftp://runtime.example");
        assert!(
            store
                .stage_update(invalid, VideoSettings::default())
                .unwrap_err()
                .contains("http:// or https://")
        );

        let valid = TrainingRuntimeSettings::new(" http://runtime.example:50051 ");
        let staged = store
            .stage_update(valid, VideoSettings::default())
            .expect("HTTP runtime endpoint is accepted");
        assert_eq!(staged.training.endpoint(), "http://runtime.example:50051");
    }
}
