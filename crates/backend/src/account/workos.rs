//! WorkOS, which signs users in to Spiral, through the same device-code flow
//! and public client as `spiraldb login`. Spiral accepts only tokens issued to
//! that client.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::login_file::LoginTokens;

const SPIRAL_CLIENT_ID: &str = "client_01J1CRS9MGF103FKTKZDCNJVM3";
const USER_MANAGEMENT: &str = "https://api.workos.com/user_management";
pub(super) const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

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

    /// Ask for a code the user confirms in their browser.
    pub(super) async fn authorize_device(&self) -> Result<DeviceAuthorization, String> {
        let response = self
            .http
            .post(format!("{}/authorize/device", self.user_management))
            .form(&DeviceRequest {
                client_id: SPIRAL_CLIENT_ID,
                scope: "email profile",
            })
            .send()
            .await
            .map_err(|error| format!("Could not reach Spiral sign-in: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Spiral sign-in could not start ({})",
                response.status()
            ));
        }
        response
            .json()
            .await
            .map_err(|error| format!("Reading Spiral sign-in: {error}"))
    }

    /// Whether the user has confirmed `device` yet.
    pub(super) async fn poll(&self, device: &DeviceAuthorization) -> Result<DevicePoll, String> {
        let request = TokenRequest {
            client_id: SPIRAL_CLIENT_ID,
            grant_type: DEVICE_CODE_GRANT,
            device_code: Some(&device.device_code),
            refresh_token: None,
            organization_id: None,
        };
        Ok(match self.authenticate(&request).await? {
            Ok(tokens) => DevicePoll::Confirmed(tokens),
            Err(error) => match error.error.as_str() {
                "authorization_pending" => DevicePoll::Pending,
                "slow_down" => DevicePoll::SlowDown,
                "access_denied" => DevicePoll::Denied,
                "expired_token" => DevicePoll::Expired,
                _ => return Err(error.describe()),
            },
        })
    }

    /// Replace the session's tokens, acting in `organization` when one is named.
    pub(super) async fn refresh(
        &self,
        tokens: &LoginTokens,
        organization: Option<&str>,
    ) -> Result<LoginTokens, RefreshFailure> {
        let request = TokenRequest {
            client_id: SPIRAL_CLIENT_ID,
            grant_type: "refresh_token",
            device_code: None,
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
    ) -> Result<Result<LoginTokens, TokenError>, String> {
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
struct DeviceRequest {
    client_id: &'static str,
    scope: &'static str,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_code: Option<&'a str>,
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

/// One started sign-in. The device code stays on the backend; the browser only
/// sees the page and the short code the user confirms.
#[derive(Deserialize)]
pub(super) struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

impl DeviceAuthorization {
    pub(super) fn user_code(&self) -> &str {
        &self.user_code
    }
    pub(super) fn verification_url(&self) -> &str {
        &self.verification_uri_complete
    }
    pub(super) fn lifetime(&self) -> Duration {
        Duration::from_secs(self.expires_in)
    }
    pub(super) fn interval(&self) -> Duration {
        Duration::from_secs(self.interval.max(1))
    }
}

pub(super) enum DevicePoll {
    Pending,
    /// WorkOS asks for a longer wait between polls.
    SlowDown,
    Confirmed(LoginTokens),
    Denied,
    Expired,
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
