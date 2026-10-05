//! One session's WorkOS tokens, as WorkOS issues them and Studio saves them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use spiral_robotics_storage::StoredSession;

/// The access token authenticates Spiral requests for a few minutes; the
/// refresh token replaces both, once.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SessionTokens {
    access_token: String,
    refresh_token: String,
}

impl SessionTokens {
    /// The tokens a saved session holds.
    pub(super) fn saved(stored: &StoredSession) -> Self {
        Self {
            access_token: stored.access_token().to_owned(),
            refresh_token: stored.refresh_token().to_owned(),
        }
    }

    /// These tokens saved beside the project their browser works on.
    pub(super) fn stored(&self, project_id: &str) -> StoredSession {
        StoredSession::new(
            self.access_token.clone(),
            self.refresh_token.clone(),
            project_id.to_owned(),
        )
    }

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

    fn claims(&self) -> Option<AccessClaims> {
        let payload = self.access_token.split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

impl std::fmt::Debug for SessionTokens {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionTokens")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct AccessClaims {
    exp: Option<u64>,
}

#[cfg(test)]
impl SessionTokens {
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
    fn saved_tokens_round_trip_beside_their_project_and_never_print() {
        let tokens = SessionTokens::for_test("org_1", Duration::from_secs(300), "refresh");
        let stored = tokens.stored("arms");
        assert_eq!(stored.project_id(), "arms");
        assert_eq!(SessionTokens::saved(&stored), tokens);
        assert!(tokens.remaining() > Duration::from_secs(290));
        assert!(!format!("{tokens:?}").contains("refresh"));
    }

    #[test]
    fn an_opaque_access_token_is_due_for_replacement() {
        let tokens = SessionTokens {
            access_token: "opaque".into(),
            refresh_token: "refresh".into(),
        };
        assert_eq!(tokens.remaining(), Duration::ZERO);
    }
}
