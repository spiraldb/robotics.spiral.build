//! Backend-owned data destination settings and secrets.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rerobot_core::api::{
    DataSettings, HuggingFaceSettings, SpiralSettings, UpdateDataSettingsRequest, VideoSettings,
};
use rerobot_core::spiral;
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = ".rerobot-data-settings.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredDataSettings {
    #[serde(default)]
    pub spiral: StoredSpiralSettings,
    #[serde(default)]
    pub hugging_face: StoredHuggingFaceSettings,
    #[serde(default)]
    pub video: VideoSettings,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSpiralSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_spiral_uri")]
    pub uri: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default = "default_dataset")]
    pub dataset: String,
}

impl Default for StoredSpiralSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            uri: default_spiral_uri(),
            project_id: String::new(),
            dataset: default_dataset(),
        }
    }
}

impl StoredSpiralSettings {
    /// Spiral is active when its destination is complete. The persisted
    /// `enabled` bit remains readable for compatibility with older settings
    /// files, but configuration—not a second toggle—is the source of truth.
    pub fn configured(&self) -> bool {
        let uri = self.uri.trim();
        let dataset = self.dataset.trim();
        !dataset.is_empty()
            && (uri == "spiral:memory:"
                || (uri == "spiraldb://" && !self.project_id.trim().is_empty()))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredHuggingFaceSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub repository: String,
    #[serde(default = "default_revision")]
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Default for StoredHuggingFaceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            repository: String::new(),
            revision: default_revision(),
            token: None,
        }
    }
}

impl StoredDataSettings {
    pub fn public(&self) -> DataSettings {
        DataSettings {
            spiral: SpiralSettings {
                enabled: self.spiral.configured(),
                uri: self.spiral.uri.clone(),
                project_id: self.spiral.project_id.clone(),
                dataset: self.spiral.dataset.clone(),
            },
            hugging_face: HuggingFaceSettings {
                enabled: self.hugging_face.enabled,
                repository: self.hugging_face.repository.clone(),
                revision: self.hugging_face.revision.clone(),
                has_token: self.hugging_face.token.is_some(),
            },
            video: self.video.clone(),
        }
    }
}

/// The settings file lives beside the recovery spool and is readable only by
/// the backend account. No API returns a stored token.
pub struct DataSettingsStore {
    path: PathBuf,
    current: Mutex<StoredDataSettings>,
}

impl DataSettingsStore {
    pub fn open(episode_directory: &Path) -> Result<Self, String> {
        let path = episode_directory.join(SETTINGS_FILE);
        let current = if path.exists() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("could not parse {}: {error}", path.display()))?
        } else {
            StoredDataSettings::default()
        };
        Ok(Self {
            path,
            current: Mutex::new(current),
        })
    }

    pub fn get(&self) -> StoredDataSettings {
        lock(&self.current).clone()
    }

    pub fn public(&self) -> DataSettings {
        lock(&self.current).public()
    }

    pub fn update(&self, update: UpdateDataSettingsRequest) -> Result<DataSettings, String> {
        validate_update(&update)?;
        let mut current = lock(&self.current);
        let mut staged = current.clone();
        staged.spiral.uri = update.spiral.uri.trim().to_owned();
        staged.spiral.project_id = update.spiral.project_id.trim().to_owned();
        staged.spiral.dataset = update.spiral.dataset.trim().to_owned();
        staged.spiral.enabled = staged.spiral.configured();
        staged.hugging_face.enabled = update.hugging_face.enabled;
        staged.hugging_face.repository = update.hugging_face.repository.trim().to_owned();
        staged.hugging_face.revision = update.hugging_face.revision.trim().to_owned();
        staged.video = update.video;

        if update.clear_hugging_face_token {
            staged.hugging_face.token = None;
        } else if let Some(token) = nonempty_secret(update.hugging_face_token) {
            staged.hugging_face.token = Some(token);
        }
        if staged.hugging_face.enabled && staged.hugging_face.token.is_none() {
            return Err(
                "Hugging Face publishing is enabled but no write token is stored".to_owned(),
            );
        }

        // Publish to disk before replacing the in-memory value. A failed disk
        // write must not make the running backend claim settings it cannot
        // recover after restart.
        write_private_json(&self.path, &staged)?;
        *current = staged;
        Ok(current.public())
    }
}

fn validate_update(update: &UpdateDataSettingsRequest) -> Result<(), String> {
    validate_spiral_uri(&update.spiral.uri)?;
    let spiral_configured =
        update.spiral.uri.trim() == "spiral:memory:" || !update.spiral.project_id.trim().is_empty();
    if spiral_configured {
        if update.spiral.uri.trim() != "spiral:memory:" {
            validate_identifier("Spiral project", &update.spiral.project_id)?;
        }
        validate_dataset_identifier(&update.spiral.dataset)?;
    }
    if update.hugging_face.enabled {
        let repo = update.hugging_face.repository.trim();
        let mut parts = repo.split('/');
        if parts.next().is_none_or(str::is_empty)
            || parts.next().is_none_or(str::is_empty)
            || parts.next().is_some()
        {
            return Err("Hugging Face repository must use owner/name".to_owned());
        }
        if update.hugging_face.revision.trim().is_empty() {
            return Err("Hugging Face revision cannot be empty".to_owned());
        }
    }
    Ok(())
}

fn validate_dataset_identifier(value: &str) -> Result<(), String> {
    let value = value.trim();
    if !spiral::is_identifier(value) {
        return Err(
            "Spiral dataset must start with a letter or '_' and contain only letters, numbers, or '_'"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!(
            "{label} must contain only letters, numbers, '_' or '-'"
        ));
    }
    Ok(())
}

fn validate_spiral_uri(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value == "spiral:memory:" || value == "spiraldb://" {
        return Ok(());
    }
    Err("Spiral ADBC URI must be 'spiraldb://' or 'spiral:memory:'".to_owned())
}

fn nonempty_secret(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn write_private_json(path: &Path, value: &StoredDataSettings) -> Result<(), String> {
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not encode data settings: {error}"))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|error| format!("could not open {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
    fs::rename(&temp, path)
        .map_err(|error| format!("could not publish {}: {error}", path.display()))?;
    Ok(())
}

fn default_spiral_uri() -> String {
    "spiraldb://".to_owned()
}

fn default_dataset() -> String {
    "rerobot".to_owned()
}

fn default_revision() -> String {
    "main".to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hugging_face_secrets_are_redacted_and_preserved_when_omitted() {
        let directory = tempfile::tempdir().unwrap();
        let store = DataSettingsStore::open(directory.path()).unwrap();
        let mut update = UpdateDataSettingsRequest {
            hugging_face_token: Some("secret".to_owned()),
            ..UpdateDataSettingsRequest::default()
        };
        store.update(update.clone()).unwrap();
        assert!(store.public().hugging_face.has_token);
        assert!(!fs::read_to_string(&store.path).unwrap().is_empty());

        update.hugging_face_token = None;
        store.update(update).unwrap();
        assert_eq!(store.get().hugging_face.token.as_deref(), Some("secret"));
    }

    #[test]
    fn enabled_spiral_dataset_is_safe_for_generated_sql() {
        let mut update = UpdateDataSettingsRequest::default();
        update.spiral.enabled = true;
        update.spiral.uri = "spiral:memory:".to_owned();
        update.spiral.project_id = "project-123".to_owned();
        update.spiral.dataset = "robot-data".to_owned();

        let error = validate_update(&update).unwrap_err();
        assert!(error.contains("Spiral dataset must start"));

        update.spiral.dataset = "robot_data_1".to_owned();
        validate_update(&update).unwrap();
    }

    #[test]
    fn spiral_configuration_is_implicit_from_the_destination() {
        let mut update = UpdateDataSettingsRequest::default();
        update.spiral.uri = "spiraldb://".to_owned();
        validate_update(&update).unwrap();

        update.spiral.uri = "spiral:memory:".to_owned();
        validate_update(&update).unwrap();

        let mut stored = StoredSpiralSettings::default();
        assert!(!stored.configured());
        stored.project_id = "project-123".to_owned();
        assert!(stored.configured());
        assert!(
            StoredDataSettings {
                spiral: stored,
                ..StoredDataSettings::default()
            }
            .public()
            .spiral
            .enabled
        );
    }
}
