//! Studio's Spiral sign-in. The backend holds the user's session, keeps its
//! short-lived access token fresh, and hands every token to each project's
//! Spiral clients. The browser only ever learns who is signed in.

mod control_plane;
pub(crate) mod http;
mod login_file;
#[cfg(test)]
mod tests;
mod workos;

use std::sync::Arc;
use std::time::Duration;

use spiral_robotics_common::account::{
    SpiralAccountStatus, SpiralIdentity, SpiralOrganization, SpiralProject, SpiralSignIn,
};
use spiral_robotics_storage::{SpiralAccess, SpiralAccessPublisher};
use tokio::sync::{Mutex, Notify};

use crate::OperationError;
use control_plane::SpiralControlPlane;
use login_file::{LoginFile, LoginTokens};
use workos::{DeviceAuthorization, DevicePoll, RefreshFailure, WorkOs};

/// Replace an access token this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// Wait this long before retrying a refresh WorkOS could not answer.
const REFRESH_RETRY: Duration = Duration::from_secs(15);

pub(crate) struct SpiralAccount {
    services: AccountServices,
    session: Mutex<AccountSession>,
    access: SpiralAccessPublisher,
    /// Wakes the refresher when the session changes.
    session_changed: Notify,
}

struct AccountServices {
    login: LoginFile,
    workos: WorkOs,
    control_plane: SpiralControlPlane,
}

enum AccountSession {
    SignedOut {
        problem: Option<String>,
    },
    SigningIn(SpiralSignIn),
    SignedIn {
        tokens: LoginTokens,
        identity: SpiralIdentity,
    },
}

impl From<&AccountSession> for SpiralAccountStatus {
    fn from(session: &AccountSession) -> Self {
        match session {
            AccountSession::SignedOut { problem } => Self::SignedOut {
                problem: problem.clone(),
            },
            AccountSession::SigningIn(sign_in) => Self::SigningIn(sign_in.clone()),
            AccountSession::SignedIn { identity, .. } => Self::SignedIn(identity.clone()),
        }
    }
}

impl AccountSession {
    fn problem(problem: impl Into<String>) -> Self {
        Self::SignedOut {
            problem: Some(problem.into()),
        }
    }

    fn awaits(&self, device: &DeviceAuthorization) -> bool {
        matches!(self, Self::SigningIn(pending) if pending.user_code() == device.user_code())
    }
}

impl SpiralAccount {
    /// Studio's account, signed in with this machine's `spiraldb login`
    /// session while that session is good. `endpoint` is Spiral's API.
    pub(crate) async fn restore(endpoint: &str) -> Result<(Arc<Self>, SpiralAccess), String> {
        let services = AccountServices {
            login: LoginFile::shared()?,
            workos: WorkOs::production()?,
            control_plane: SpiralControlPlane::new()?,
        };
        Ok(Self::restore_with(services, endpoint).await)
    }

    async fn restore_with(services: AccountServices, endpoint: &str) -> (Arc<Self>, SpiralAccess) {
        let (access, reader) = SpiralAccessPublisher::new();
        let account = Arc::new(Self {
            services,
            session: Mutex::new(AccountSession::SignedOut { problem: None }),
            access,
            session_changed: Notify::new(),
        });
        let saved = account.saved_session(endpoint).await;
        account.enter(&mut *account.session.lock().await, saved);
        tokio::spawn(account.clone().keep_fresh());
        (account, reader)
    }

    /// A signed-out account that never reaches WorkOS or Spiral, for tests of
    /// other features.
    #[cfg(test)]
    pub(crate) fn offline() -> Arc<Self> {
        let unreachable = "http://127.0.0.1:9".to_owned();
        let (access, _reader) = SpiralAccessPublisher::new();
        Arc::new(Self {
            services: AccountServices {
                login: LoginFile::at(
                    std::env::temp_dir().join("spiral-robotics-offline-login.json"),
                ),
                workos: WorkOs::at(unreachable).unwrap(),
                control_plane: SpiralControlPlane::new().unwrap(),
            },
            session: Mutex::new(AccountSession::SignedOut { problem: None }),
            access,
            session_changed: Notify::new(),
        })
    }

    pub(crate) async fn status(&self) -> SpiralAccountStatus {
        SpiralAccountStatus::from(&*self.session.lock().await)
    }

    /// Start a sign-in the user confirms in their browser. It completes in the
    /// background; starting again replaces an unconfirmed one.
    pub(crate) async fn sign_in(
        self: &Arc<Self>,
        endpoint: String,
    ) -> Result<SpiralAccountStatus, OperationError> {
        let mut session = self.session.lock().await;
        if matches!(*session, AccountSession::SignedIn { .. }) {
            return Err(OperationError::conflict(
                "Sign out of Spiral before signing in again.",
            ));
        }
        let device = self
            .services
            .workos
            .authorize_device()
            .await
            .map_err(OperationError::External)?;
        let sign_in = SpiralSignIn::new(
            device.verification_url().to_owned(),
            device.user_code().to_owned(),
        );
        self.enter(&mut session, AccountSession::SigningIn(sign_in));
        tokio::spawn(self.clone().await_confirmation(endpoint, device));
        Ok(SpiralAccountStatus::from(&*session))
    }

    /// Sign out of Spiral on this machine, like `spiraldb logout`.
    pub(crate) async fn sign_out(&self) -> Result<SpiralAccountStatus, OperationError> {
        let mut session = self.session.lock().await;
        self.services
            .login
            .remove()
            .map_err(OperationError::Internal)?;
        self.enter(&mut session, AccountSession::SignedOut { problem: None });
        Ok(SpiralAccountStatus::from(&*session))
    }

    /// Continue the session in another of the user's organizations.
    pub(crate) async fn switch_organization(
        &self,
        endpoint: &str,
        organization: &str,
    ) -> Result<SpiralAccountStatus, OperationError> {
        let mut session = self.session.lock().await;
        let AccountSession::SignedIn { tokens, .. } = &*session else {
            return Err(Self::sign_in_first());
        };
        let tokens = match self
            .services
            .workos
            .refresh(tokens, Some(organization))
            .await
        {
            Ok(tokens) => tokens,
            Err(RefreshFailure::Ended(ended)) => {
                self.enter(&mut session, AccountSession::problem(Self::ended(&ended)));
                return Err(OperationError::External(Self::ended(&ended)));
            }
            Err(failure) => return Err(OperationError::External(failure.message().to_owned())),
        };
        let next = self.saved(self.signed_in(endpoint, tokens).await);
        self.enter(&mut session, next);
        match &*session {
            AccountSession::SignedOut {
                problem: Some(problem),
            } => Err(OperationError::External(problem.clone())),
            session => Ok(SpiralAccountStatus::from(session)),
        }
    }

    pub(crate) async fn organizations(
        &self,
        endpoint: &str,
    ) -> Result<Vec<SpiralOrganization>, OperationError> {
        let token = self.access_token().await?;
        self.services
            .control_plane
            .organizations(endpoint, &token)
            .await
            .map_err(OperationError::External)
    }

    pub(crate) async fn projects(
        &self,
        endpoint: &str,
    ) -> Result<Vec<SpiralProject>, OperationError> {
        let token = self.access_token().await?;
        self.services
            .control_plane
            .projects(endpoint, &token)
            .await
            .map_err(OperationError::External)
    }

    async fn access_token(&self) -> Result<String, OperationError> {
        match &*self.session.lock().await {
            AccountSession::SignedIn { tokens, .. } => Ok(tokens.access_token().to_owned()),
            _ => Err(Self::sign_in_first()),
        }
    }

    fn sign_in_first() -> OperationError {
        OperationError::invalid_input("Sign in to Spiral first.")
    }

    fn ended(detail: &str) -> String {
        format!("Your Spiral session ended ({detail}). Sign in again.")
    }

    /// Change the session, and give every Spiral client its access token.
    fn enter(&self, session: &mut AccountSession, next: AccountSession) {
        self.access.publish(match &next {
            AccountSession::SignedIn { tokens, .. } => Some(tokens.access_token().to_owned()),
            _ => None,
        });
        *session = next;
        self.session_changed.notify_one();
    }

    /// The session `spiraldb login` saved, refreshed when its token lapsed.
    async fn saved_session(&self, endpoint: &str) -> AccountSession {
        let tokens = match self.services.login.read() {
            Ok(Some(tokens)) => tokens,
            Ok(None) => return AccountSession::SignedOut { problem: None },
            Err(problem) => return AccountSession::problem(problem),
        };
        if tokens.remaining() > REFRESH_MARGIN {
            return self.signed_in(endpoint, tokens).await;
        }
        match self.services.workos.refresh(&tokens, None).await {
            Ok(tokens) => self.saved(self.signed_in(endpoint, tokens).await),
            Err(failure) => AccountSession::problem(Self::ended(failure.message())),
        }
    }

    /// Who `tokens` belong to, as Spiral sees them.
    async fn signed_in(&self, endpoint: &str, tokens: LoginTokens) -> AccountSession {
        match self
            .services
            .control_plane
            .identity(endpoint, tokens.access_token())
            .await
        {
            Ok(identity) => AccountSession::SignedIn { tokens, identity },
            Err(problem) => AccountSession::problem(problem),
        }
    }

    /// Share a new session with the machine's other Spiral apps.
    fn saved(&self, session: AccountSession) -> AccountSession {
        if let AccountSession::SignedIn { tokens, .. } = &session
            && let Err(problem) = self.services.login.write(tokens)
        {
            return AccountSession::problem(problem);
        }
        session
    }

    async fn await_confirmation(self: Arc<Self>, endpoint: String, device: DeviceAuthorization) {
        let expires = tokio::time::Instant::now() + device.lifetime();
        let mut interval = device.interval();
        let expired = "The sign-in code expired. Sign in again.";
        let next = loop {
            tokio::time::sleep(interval).await;
            if !self.session.lock().await.awaits(&device) {
                return;
            }
            if tokio::time::Instant::now() >= expires {
                break AccountSession::problem(expired);
            }
            match self.services.workos.poll(&device).await {
                Ok(DevicePoll::Pending) => {}
                Ok(DevicePoll::SlowDown) => interval += Duration::from_secs(5),
                Ok(DevicePoll::Confirmed(tokens)) => {
                    break self.saved(self.signed_in(&endpoint, tokens).await);
                }
                Ok(DevicePoll::Denied) => {
                    break AccountSession::problem("Sign-in was declined in the browser.");
                }
                Ok(DevicePoll::Expired) => break AccountSession::problem(expired),
                // The code stays valid through a failed poll; keep trying until it expires.
                Err(error) => tracing::warn!("polling Spiral sign-in: {error}"),
            }
        };
        let mut session = self.session.lock().await;
        if session.awaits(&device) {
            self.enter(&mut session, next);
        }
    }

    /// Replace each access token shortly before it expires, for as long as
    /// the process runs.
    async fn keep_fresh(self: Arc<Self>) {
        loop {
            let changed = self.session_changed.notified();
            let due = match &*self.session.lock().await {
                AccountSession::SignedIn { tokens, .. } => {
                    Some(tokens.remaining().saturating_sub(REFRESH_MARGIN))
                }
                _ => None,
            };
            let Some(due) = due else {
                changed.await;
                continue;
            };
            tokio::select! {
                () = changed => {}
                () = tokio::time::sleep(due) => {
                    if let Err(error) = self.replace_token().await {
                        tracing::warn!("could not refresh the Spiral session: {error}");
                        tokio::time::sleep(REFRESH_RETRY).await;
                    }
                }
            }
        }
    }

    async fn replace_token(&self) -> Result<(), String> {
        let mut session = self.session.lock().await;
        let AccountSession::SignedIn { tokens, identity } = &*session else {
            return Ok(());
        };
        let next = match self.services.workos.refresh(tokens, None).await {
            Ok(next) => next,
            Err(RefreshFailure::Ended(ended)) => {
                match self.shared_successor(tokens, identity).await {
                    Some(next) => next,
                    None => {
                        self.enter(&mut session, AccountSession::problem(Self::ended(&ended)));
                        return Ok(());
                    }
                }
            }
            Err(failure) => return Err(failure.message().to_owned()),
        };
        if let Err(problem) = self.services.login.write(&next) {
            tracing::warn!("could not share the refreshed Spiral session: {problem}");
        }
        let identity = identity.clone();
        self.enter(
            &mut session,
            AccountSession::SignedIn {
                tokens: next,
                identity,
            },
        );
        Ok(())
    }

    /// Another Spiral app on this machine may have refreshed the shared
    /// session first, retiring Studio's refresh token. Continue from the file
    /// when it still holds this organization's session.
    async fn shared_successor(
        &self,
        tokens: &LoginTokens,
        identity: &SpiralIdentity,
    ) -> Option<LoginTokens> {
        let saved = self
            .services
            .login
            .read()
            .ok()
            .flatten()
            .filter(|saved| saved.refresh_token() != tokens.refresh_token())?;
        if saved.organization_id().as_deref() != Some(identity.organization().id()) {
            return None;
        }
        if saved.remaining() > REFRESH_MARGIN {
            return Some(saved);
        }
        self.services.workos.refresh(&saved, None).await.ok()
    }
}
