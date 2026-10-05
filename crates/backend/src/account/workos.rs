//! WorkOS, which signs users in to Spiral. Studio uses the same public client
//! as Spiral's own apps, because Spiral accepts only tokens issued to it; PKCE
//! stands in for a client secret.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::tokens::SessionTokens;

const SPIRAL_CLIENT_ID: &str = "client_01J1CRS9MGF103FKTKZDCNJVM3";
const USER_MANAGEMENT: &str = "https://api.workos.com/user_management";

pub(super) struct WorkOs {
    http: reqwest::Client,
    user_management: String,
}

impl WorkOs {
    pub(super) fn production() -> Result<Self, String> {
        Self::at(USER_MANAGEMENT.to_owned())
    }

    pub(super) fn at(user_management: String) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            user_management,
        })
    }

    /// Spiral's sign-in page, which returns the browser to `redirect` with a
    /// code for `challenge` and the `state` it was given.
    pub(super) fn authorization_url(&self, redirect: &str, state: &str, challenge: &str) -> String {
        let mut url = url::Url::parse(&format!("{}/authorize", self.user_management))
            .expect("WorkOS address is a URL");
        url.query_pairs_mut()
            .append_pair("client_id", SPIRAL_CLIENT_ID)
            .append_pair("redirect_uri", redirect)
            .append_pair("response_type", "code")
            .append_pair("provider", "authkit")
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        url.into()
    }

    /// The user's tokens for the `code` the sign-in page returned, proven by
    /// the `verifier` behind its challenge.
    pub(super) async fn redeem(&self, code: &str, verifier: &str) -> Result<SessionTokens, String> {
        let request = TokenRequest {
            client_id: SPIRAL_CLIENT_ID,
            grant_type: "authorization_code",
            code: Some(code),
            code_verifier: Some(verifier),
            refresh_token: None,
            organization_id: None,
        };
        self.authenticate(&request)
            .await?
            .map_err(|error| format!("Spiral sign-in failed: {}", error.describe()))
    }

    /// Replace the session's tokens, acting in `organization` when one is named.
    pub(super) async fn refresh(
        &self,
        tokens: &SessionTokens,
        organization: Option<&str>,
    ) -> Result<SessionTokens, RefreshFailure> {
        let request = TokenRequest {
            client_id: SPIRAL_CLIENT_ID,
            grant_type: "refresh_token",
            code: None,
            code_verifier: None,
            refresh_token: Some(tokens.refresh_token()),
            organization_id: organization,
        };
        match self.authenticate(&request).await {
            Ok(Ok(tokens)) => Ok(tokens),
            Ok(Err(error)) if error.error == "invalid_grant" => {
                Err(RefreshFailure::Ended(error.describe()))
            }
            Ok(Err(error)) => Err(RefreshFailure::Rejected(error.describe())),
            Err(error) => Err(RefreshFailure::Unreachable(error)),
        }
    }

    async fn authenticate(
        &self,
        request: &TokenRequest<'_>,
    ) -> Result<Result<SessionTokens, TokenError>, String> {
        let response = self
            .http
            .post(format!("{}/authenticate", self.user_management))
            .form(request)
            .send()
            .await
            .map_err(|error| format!("Could not reach Spiral sign-in: {error}"))?;
        if response.status().is_success() {
            return response
                .json()
                .await
                .map(Ok)
                .map_err(|error| format!("Reading Spiral sign-in: {error}"));
        }
        let status = response.status();
        Ok(Err(response.json().await.unwrap_or_else(|_| TokenError {
            error: status.to_string(),
            error_description: None,
        })))
    }
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    organization_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

impl TokenError {
    fn describe(&self) -> String {
        self.error_description
            .clone()
            .unwrap_or_else(|| self.error.clone())
    }
}

pub(super) enum RefreshFailure {
    /// The session is over: revoked, expired, or its refresh token already used.
    Ended(String),
    /// The session continues, but WorkOS refused this change.
    Rejected(String),
    Unreachable(String),
}

impl RefreshFailure {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::Ended(message) | Self::Rejected(message) | Self::Unreachable(message) => message,
        }
    }
}
