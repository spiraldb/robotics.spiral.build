//! The session `spiraldb login` saves, which the Spiral CLI and desktop app
//! share: signing in to any of them signs in to all.

use std::io::{ErrorKind, Write as _};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// The file holding the signed-in user's WorkOS tokens.
pub(super) struct LoginFile {
    path: PathBuf,
}

impl LoginFile {
    /// Where `spiraldb login` keeps its session on this machine. The same
    /// directory lookup as Spiral's keeps every app on one file.
    pub(super) fn shared() -> Result<Self, String> {
        let directories = directories::ProjectDirs::from("", "pyspiral", "")
            .ok_or("Could not find the Spiral login directory. Is HOME set?")?;
        Ok(Self::at(directories.config_dir().join("auth.json")))
    }

    pub(super) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn read(&self) -> Result<Option<LoginTokens>, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Reading {}: {error}", self.path.display())),
        };
        serde_json::from_slice::<LoginDocument>(&bytes)
            .map(|document| document.tokens)
            .map_err(|error| {
                format!(
                    "Reading the Spiral login at {}: {error}",
                    self.path.display()
                )
            })
    }

    /// Replace the saved session, readable only by this user.
    pub(super) fn write(&self, tokens: &LoginTokens) -> Result<(), String> {
        let failed = |error: std::io::Error| format!("Saving {}: {error}", self.path.display());
        if let Some(directory) = self.path.parent() {
            std::fs::create_dir_all(directory).map_err(failed)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&self.path).map_err(failed)?;
        let document = LoginDocument {
            tokens: Some(tokens.clone()),
        };
        let json = serde_json::to_string(&document).map_err(|error| error.to_string())?;
        writeln!(file, "{json}").map_err(failed)
    }

    pub(super) fn remove(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                Err(format!("Removing {}: {error}", self.path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// The file's format, as `spiraldb login` writes it.
#[derive(Deserialize, Serialize)]
struct LoginDocument {
    tokens: Option<LoginTokens>,
}

/// One session's WorkOS tokens. The access token authenticates Spiral requests
/// for a few minutes; the refresh token replaces both, once.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct LoginTokens {
    access_token: String,
    refresh_token: String,
}

impl LoginTokens {
    pub(super) fn access_token(&self) -> &str {
        &self.access_token
    }

    pub(super) fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    /// How long the access token stays valid. Spiral verifies the token; the
    /// claim only schedules its replacement.
    pub(super) fn remaining(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.claims()
            .and_then(|claims| claims.exp)
            .map_or(Duration::ZERO, |expires| {
                Duration::from_secs(expires.saturating_sub(now))
            })
    }

    /// The organization the session acts in.
    pub(super) fn organization_id(&self) -> Option<String> {
        self.claims().and_then(|claims| claims.org_id)
    }

    fn claims(&self) -> Option<AccessClaims> {
        let payload = self.access_token.split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

impl std::fmt::Debug for LoginTokens {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginTokens")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct AccessClaims {
    exp: Option<u64>,
    org_id: Option<String>,
}

#[cfg(test)]
impl LoginTokens {
    /// Tokens whose access token expires `lifetime` from now in `organization`.
    pub(super) fn for_test(organization: &str, lifetime: Duration, refresh: &str) -> Self {
        let expires = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + lifetime;
        let claims = serde_json::json!({"org_id": organization, "exp": expires.as_secs()});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        Self {
            access_token: format!("header.{payload}.signature"),
            refresh_token: refresh.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_session_round_trips_privately_in_spirals_format() {
        let directory = tempfile::tempdir().unwrap();
        let file = LoginFile::at(directory.path().join("pyspiral").join("auth.json"));
        assert!(file.read().unwrap().is_none());
        let tokens = LoginTokens::for_test("org_1", Duration::from_secs(300), "refresh");
        file.write(&tokens).unwrap();
        assert_eq!(file.read().unwrap(), Some(tokens.clone()));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file.path).unwrap()).unwrap();
        assert_eq!(saved["tokens"]["refresh_token"], "refresh");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(tokens.organization_id().as_deref(), Some("org_1"));
        assert!(tokens.remaining() > Duration::from_secs(290));
        assert!(!format!("{tokens:?}").contains("refresh"));
        file.remove().unwrap();
        file.remove().unwrap();
        assert!(file.read().unwrap().is_none());
    }

    #[test]
    fn an_opaque_access_token_is_due_for_replacement() {
        let tokens = LoginTokens {
            access_token: "opaque".into(),
            refresh_token: "refresh".into(),
        };
        assert_eq!(tokens.remaining(), Duration::ZERO);
        assert_eq!(tokens.organization_id(), None);
    }
}
