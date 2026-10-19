//! Spiral OAuth for this registered application, with PKCE and user consent.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use spiral_robotics_common::account::{SpiralIdentity, SpiralOrganization};

use super::tokens::SessionTokens;

const ISSUER: &str = "https://auth.spiraldb.com";
const SCOPES: &str =
    "openid email projects:read catalog:read catalog:write tables:read tables:write";

pub(super) struct SpiralOAuth {
    http: reqwest::Client,
    issuer: String,
    client_id: String,
    resource: String,
    redirect_uri: String,
}

impl SpiralOAuth {
    pub(super) fn from_environment(resource: String, redirect_uri: String) -> Result<Self, String> {
        let client_id = std::env::var("SPIRAL_ROBOTICS_CLIENT_ID").map_err(|_| {
            "Set SPIRAL_ROBOTICS_CLIENT_ID to this environment's registered application ID"
                .to_owned()
        })?;
        let issuer =
            std::env::var("SPIRAL_ROBOTICS_AUTH_URL").unwrap_or_else(|_| ISSUER.to_owned());
        Self::at(issuer, client_id, resource, redirect_uri)
    }

    pub(super) fn at(
        issuer: String,
        client_id: String,
        resource: String,
        redirect_uri: String,
    ) -> Result<Self, String> {
        let url = url::Url::parse(&issuer).map_err(|error| error.to_string())?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || url.host_str().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || client_id.trim().is_empty()
        {
            return Err("OAuth requires an HTTPS issuer origin (HTTP is allowed on loopback) and a client ID".to_owned());
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id,
            resource,
            redirect_uri,
        })
    }

    /// Spiral's sign-in page, which returns the browser to the callback with a
    /// code for `challenge` and the `state` it was given.
    pub(super) fn authorization_url(&self, state: &str, challenge: &str) -> String {
        let mut url = url::Url::parse(&format!("{}/oauth/authorize", self.issuer))
            .expect("validated authorization server URL");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("resource", &self.resource)
            .append_pair("scope", SCOPES)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        url.into()
    }

    /// The user's tokens for the `code` the sign-in page returned, proven by
    /// the `verifier` behind its challenge.
    pub(super) async fn redeem(&self, code: &str, verifier: &str) -> Result<SessionTokens, String> {
        let request = TokenRequest {
            client_id: &self.client_id,
            grant_type: "authorization_code",
            code: Some(code),
            code_verifier: Some(verifier),
            refresh_token: None,
            redirect_uri: Some(&self.redirect_uri),
            resource: &self.resource,
        };
        self.authenticate(&request)
            .await?
            .map_err(|error| format!("Spiral sign-in failed: {}", error.describe()))
    }

    /// Replace the tokens within the same client, resource and consented organization.
    pub(super) async fn refresh(
        &self,
        tokens: &SessionTokens,
    ) -> Result<SessionTokens, RefreshFailure> {
        let request = TokenRequest {
            client_id: &self.client_id,
            grant_type: "refresh_token",
            code: None,
            code_verifier: None,
            refresh_token: Some(tokens.refresh_token()),
            redirect_uri: None,
            resource: &self.resource,
        };
        match self.authenticate(&request).await {
            Ok(Ok(tokens)) => Ok(tokens),
            Ok(Err(error))
                if matches!(error.error.as_str(), "invalid_grant" | "invalid_client") =>
            {
                Err(RefreshFailure::Ended(error.describe()))
            }
            Ok(Err(error)) => Err(RefreshFailure::Rejected(error.describe())),
            Err(error) => Err(RefreshFailure::Unreachable(error)),
        }
    }

    /// Read identity through UserInfo, which enforces the identity scopes.
    pub(super) async fn identity(&self, token: &str) -> Result<SpiralIdentity, String> {
        let response = self
            .http
            .get(format!("{}/userinfo", self.issuer))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Spiral refused UserInfo ({})", response.status()));
        }
        let user: UserInfo = response.json().await.map_err(|error| error.to_string())?;
        Ok(SpiralIdentity::new(
            user.email,
            SpiralOrganization::new(user.org_id, None),
        ))
    }

    /// Revoke this application's refresh token when the user signs out.
    pub(super) async fn revoke(&self, tokens: &SessionTokens) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/oauth/revoke", self.issuer))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("token", tokens.refresh_token()),
                ("token_type_hint", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Spiral could not revoke the session ({})",
                response.status()
            ));
        }
        Ok(())
    }

    async fn authenticate(
        &self,
        request: &TokenRequest<'_>,
    ) -> Result<Result<SessionTokens, TokenError>, String> {
        let response = self
            .http
            .post(format!("{}/oauth/token", self.issuer))
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
    client_id: &'a str,
    grant_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect_uri: Option<&'a str>,
    resource: &'a str,
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
    /// The session continues, but Spiral refused this change.
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

#[derive(Deserialize)]
struct UserInfo {
    email: String,
    org_id: String,
}
