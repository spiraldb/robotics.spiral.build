//! Studio's Spiral sign-in, one session per browser. A browser signs in on
//! Spiral's sign-in page; the backend keeps that user's tokens, keeps the
//! short-lived access token fresh, and hands it to the Spiral clients of every
//! project the browser opens. The browser holds only its session's ID, in a
//! cookie, and only ever learns who is signed in.

mod control_plane;
#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod http;
pub(crate) mod sign_in;
#[cfg(test)]
mod tests;
mod tokens;
mod workos;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use base64::Engine as _;
use spiral_robotics_common::account::{SpiralIdentity, SpiralOrganization, SpiralProject};
use spiral_robotics_storage::{SessionStore, SpiralAccess, SpiralAccessPublisher};
use tokio::sync::{Mutex, MutexGuard, Notify, OnceCell};

use crate::OperationError;
use crate::sync::MutexRecovery as _;
use control_plane::SpiralControlPlane;
use sign_in::{PublicAddress, SignInAttempt};
use tokens::SessionTokens;
use workos::{RefreshFailure, WorkOs};

/// Replace an access token this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// Wait this long before retrying a refresh WorkOS could not answer.
const REFRESH_RETRY: Duration = Duration::from_secs(15);

/// How sessions reach Spiral's sign-in and API, and where they are saved.
struct SessionServices {
    workos: WorkOs,
    control_plane: SpiralControlPlane,
    store: SessionStore,
}

/// Every browser's Spiral session. A session saved before a restart comes back
/// on its browser's first request.
pub(crate) struct SpiralSessions {
    services: Arc<SessionServices>,
    address: PublicAddress,
    /// Sessions this process has restored or started, by ID. A session being
    /// restored is restored once, however many requests wait for it.
    live: StdMutex<HashMap<String, Arc<OnceCell<Arc<SpiralSession>>>>>,
}

impl SpiralSessions {
    /// Sessions that sign in to Spiral at `spiral_api` from browsers that
    /// reach Studio at `address`.
    pub(crate) fn new(
        spiral_api: String,
        store: SessionStore,
        address: PublicAddress,
    ) -> Result<Self, String> {
        let services = SessionServices {
            workos: WorkOs::production()?,
            control_plane: SpiralControlPlane::new(spiral_api)?,
            store,
        };
        Ok(Self::with_services(services, address))
    }

    fn with_services(services: SessionServices, address: PublicAddress) -> Self {
        Self {
            services: Arc::new(services),
            address,
            live: StdMutex::new(HashMap::new()),
        }
    }

    pub(crate) fn address(&self) -> &PublicAddress {
        &self.address
    }

    /// The live session `id` names, restored from SQLite if this process has
    /// not seen it yet.
    pub(crate) async fn find(&self, id: &str) -> Result<Arc<SpiralSession>, OperationError> {
        let cell = self
            .live
            .recover_lock()
            .entry(id.to_owned())
            .or_default()
            .clone();
        let found = cell.get_or_try_init(|| self.restore(id)).await.cloned();
        match found {
            Ok(session) if !session.ended() => Ok(session),
            other => {
                let mut live = self.live.recover_lock();
                if live
                    .get(id)
                    .is_some_and(|current| Arc::ptr_eq(current, &cell))
                {
                    live.remove(id);
                }
                Err(other.err().unwrap_or_else(SpiralSession::signed_out))
            }
        }
    }

    /// End `session` and forget it.
    pub(crate) async fn sign_out(&self, session: &SpiralSession) -> Result<(), OperationError> {
        session.end().await.map_err(OperationError::Internal)?;
        self.live.recover_lock().remove(session.id());
        Ok(())
    }

    /// Spiral's sign-in page for `attempt`, which returns the browser to this
    /// Studio's callback.
    fn authorization_url(&self, attempt: &SignInAttempt) -> String {
        self.services.workos.authorization_url(
            &self.address.callback(),
            attempt.state(),
            &attempt.challenge(),
        )
    }

    /// Start the session `code` signs in, proven by `attempt`. Returns the new
    /// session's ID.
    async fn complete(
        &self,
        code: &str,
        attempt: &SignInAttempt,
    ) -> Result<String, OperationError> {
        let tokens = self
            .services
            .workos
            .redeem(code, attempt.verifier())
            .await
            .map_err(OperationError::External)?;
        let identity = self
            .services
            .control_plane
            .identity(tokens.access_token())
            .await
            .map_err(OperationError::External)?;
        let id = Self::new_id()?;
        let state = SessionState {
            tokens,
            identity,
            project_id: String::new(),
        };
        self.services
            .store
            .save(&id, &state.stored())
            .await
            .map_err(OperationError::Internal)?;
        let session = SpiralSession::start(id.clone(), self.services.clone(), state);
        self.live
            .recover_lock()
            .insert(id.clone(), Arc::new(OnceCell::new_with(Some(session))));
        Ok(id)
    }

    /// The saved session `id`, its tokens refreshed when they lapsed.
    async fn restore(&self, id: &str) -> Result<Arc<SpiralSession>, OperationError> {
        let stored = self
            .services
            .store
            .load(id)
            .await
            .map_err(OperationError::Internal)?
            .ok_or_else(SpiralSession::signed_out)?;
        let mut tokens = SessionTokens::saved(&stored);
        if tokens.remaining() <= REFRESH_MARGIN {
            tokens = match self.services.workos.refresh(&tokens, None).await {
                Ok(tokens) => tokens,
                Err(RefreshFailure::Ended(ended)) => {
                    tracing::info!("a saved Spiral session ended: {ended}");
                    self.services
                        .store
                        .remove(id)
                        .await
                        .map_err(OperationError::Internal)?;
                    return Err(SpiralSession::signed_out());
                }
                Err(failure) => return Err(OperationError::External(failure.message().to_owned())),
            };
            self.services
                .store
                .save(id, &tokens.stored(stored.project_id()))
                .await
                .map_err(OperationError::Internal)?;
        }
        let identity = self
            .services
            .control_plane
            .identity(tokens.access_token())
            .await
            .map_err(OperationError::External)?;
        let state = SessionState {
            tokens,
            identity,
            project_id: stored.project_id().to_owned(),
        };
        Ok(SpiralSession::start(
            id.to_owned(),
            self.services.clone(),
            state,
        ))
    }

    /// A session ID nobody can guess: 256 random bits.
    fn new_id() -> Result<String, OperationError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| OperationError::Internal(format!("no randomness: {error}")))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }
}

/// What a live session knows: its tokens, who they belong to, and the project
/// its browser works on, empty before one is chosen.
struct SessionState {
    tokens: SessionTokens,
    identity: SpiralIdentity,
    project_id: String,
}

impl SessionState {
    fn stored(&self) -> spiral_robotics_storage::StoredSession {
        self.tokens.stored(&self.project_id)
    }
}

/// One browser's Spiral sign-in.
pub(crate) struct SpiralSession {
    id: String,
    services: Arc<SessionServices>,
    /// Held across every token change, so each refresh token is used once.
    state: Mutex<SessionState>,
    access: SpiralAccessPublisher,
    reader: SpiralAccess,
    ended: AtomicBool,
    /// Wakes the refresher when the tokens change or the session ends.
    changed: Notify,
}

impl SpiralSession {
    /// A live session, publishing its access token and keeping it fresh.
    fn start(id: String, services: Arc<SessionServices>, state: SessionState) -> Arc<Self> {
        let (access, reader) = SpiralAccessPublisher::new();
        access.publish(Some(state.tokens.access_token().to_owned()));
        let session = Arc::new(Self {
            id,
            services,
            state: Mutex::new(state),
            access,
            reader,
            ended: AtomicBool::new(false),
            changed: Notify::new(),
        });
        tokio::spawn(session.clone().keep_fresh());
        session
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Whether the session is over: signed out, or ended by Spiral.
    pub(crate) fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// The session's current access token, as its projects' Spiral clients read it.
    pub(crate) fn access(&self) -> SpiralAccess {
        self.reader.clone()
    }

    pub(crate) async fn identity(&self) -> Result<SpiralIdentity, OperationError> {
        Ok(self.active().await?.identity.clone())
    }

    /// The project this session's browser works on; empty before one is chosen.
    pub(crate) async fn project_id(&self) -> Result<String, OperationError> {
        Ok(self.active().await?.project_id.clone())
    }

    /// Work on `project_id` from now on, across restarts.
    pub(crate) async fn choose_project(&self, project_id: &str) -> Result<(), OperationError> {
        let mut state = self.active().await?;
        if state.project_id != project_id {
            state.project_id = project_id.to_owned();
            self.save(&state).await.map_err(OperationError::Internal)?;
        }
        Ok(())
    }

    /// Whether Spiral lets this user open `project_id`. Studio keeps its own
    /// records of every project, so it asks before showing them.
    pub(crate) async fn admit(&self, project_id: &str) -> Result<(), OperationError> {
        let token = self.access_token().await?;
        match self.services.control_plane.admits(&token, project_id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(OperationError::NotFound(format!(
                "Your Spiral account cannot open the project {project_id}."
            ))),
            Err(problem) => Err(OperationError::External(problem)),
        }
    }

    pub(crate) async fn organizations(&self) -> Result<Vec<SpiralOrganization>, OperationError> {
        let token = self.access_token().await?;
        self.services
            .control_plane
            .organizations(&token)
            .await
            .map_err(OperationError::External)
    }

    pub(crate) async fn projects(&self) -> Result<Vec<SpiralProject>, OperationError> {
        let token = self.access_token().await?;
        self.services
            .control_plane
            .projects(&token)
            .await
            .map_err(OperationError::External)
    }

    /// Continue in another of the user's organizations, whose projects are
    /// others: the chosen project is forgotten.
    pub(crate) async fn switch_organization(
        &self,
        organization: &str,
    ) -> Result<SpiralIdentity, OperationError> {
        let mut state = self.active().await?;
        let tokens = match self
            .services
            .workos
            .refresh(&state.tokens, Some(organization))
            .await
        {
            Ok(tokens) => tokens,
            Err(RefreshFailure::Ended(ended)) => {
                self.mark_ended();
                self.forget().await;
                return Err(OperationError::Unauthenticated(format!(
                    "Your Spiral session ended ({ended}). Sign in again."
                )));
            }
            Err(failure) => return Err(OperationError::External(failure.message().to_owned())),
        };
        state.tokens = tokens;
        state.project_id.clear();
        self.access
            .publish(Some(state.tokens.access_token().to_owned()));
        self.changed.notify_one();
        let saved = self.save(&state).await;
        state.identity = self
            .services
            .control_plane
            .identity(state.tokens.access_token())
            .await
            .map_err(OperationError::External)?;
        saved.map_err(OperationError::Internal)?;
        Ok(state.identity.clone())
    }

    /// The failure a request without a live session gets.
    fn signed_out() -> OperationError {
        OperationError::Unauthenticated("Sign in to Spiral to continue.".to_owned())
    }

    async fn active(&self) -> Result<MutexGuard<'_, SessionState>, OperationError> {
        let state = self.state.lock().await;
        if self.ended() {
            return Err(Self::signed_out());
        }
        Ok(state)
    }

    async fn access_token(&self) -> Result<String, OperationError> {
        Ok(self.active().await?.tokens.access_token().to_owned())
    }

    async fn save(&self, state: &SessionState) -> Result<(), String> {
        self.services.store.save(&self.id, &state.stored()).await
    }

    /// Sign out: no Spiral client of this session reaches Spiral again.
    async fn end(&self) -> Result<(), String> {
        let _state = self.state.lock().await;
        self.mark_ended();
        self.services.store.remove(&self.id).await
    }

    fn mark_ended(&self) {
        self.ended.store(true, Ordering::Release);
        self.access.publish(None);
        self.changed.notify_one();
    }

    /// Forget a session Spiral ended. Its browser signs in again.
    async fn forget(&self) {
        if let Err(problem) = self.services.store.remove(&self.id).await {
            tracing::warn!("could not forget an ended Spiral session: {problem}");
        }
    }

    /// Replace each access token shortly before it expires, until the session ends.
    async fn keep_fresh(self: Arc<Self>) {
        while !self.ended() {
            let changed = self.changed.notified();
            let due = self
                .state
                .lock()
                .await
                .tokens
                .remaining()
                .saturating_sub(REFRESH_MARGIN);
            tokio::select! {
                () = changed => {}
                () = tokio::time::sleep(due) => {
                    if let Err(error) = self.replace_token().await {
                        tracing::warn!("could not refresh a Spiral session: {error}");
                        tokio::time::sleep(REFRESH_RETRY).await;
                    }
                }
            }
        }
    }

    async fn replace_token(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if self.ended() {
            return Ok(());
        }
        match self.services.workos.refresh(&state.tokens, None).await {
            Ok(tokens) => {
                state.tokens = tokens;
                self.access
                    .publish(Some(state.tokens.access_token().to_owned()));
                if let Err(problem) = self.save(&state).await {
                    tracing::warn!("could not save a refreshed Spiral session: {problem}");
                }
                Ok(())
            }
            Err(RefreshFailure::Ended(ended)) => {
                tracing::info!("a Spiral session ended: {ended}");
                self.mark_ended();
                self.forget().await;
                Ok(())
            }
            Err(failure) => Err(failure.message().to_owned()),
        }
    }
}
