//! The signed-in Spiral user's access token, as Studio's Spiral clients read it.
//!
//! The backend's account signs the user in and keeps the token fresh; every
//! database and catalog client of every project reads the same current value,
//! and open databases receive each replacement.

use tokio::sync::watch;

use crate::settings::SpiralConnectionSettings;

/// A Spiral destination and the signed-in user's access to it: everything a
/// project's database and catalog clients need to reach Spiral.
#[derive(Clone)]
pub struct SpiralConnection {
    settings: SpiralConnectionSettings,
    access: SpiralAccess,
}

impl SpiralConnection {
    pub fn new(settings: SpiralConnectionSettings, access: SpiralAccess) -> Self {
        Self { settings, access }
    }

    pub(crate) fn settings(&self) -> &SpiralConnectionSettings {
        &self.settings
    }

    pub(crate) fn access(&self) -> &SpiralAccess {
        &self.access
    }
}

/// Replaces the access token every [`SpiralAccess`] reads. Only the account owns one.
pub struct SpiralAccessPublisher(watch::Sender<Option<String>>);

impl SpiralAccessPublisher {
    /// A publisher with no signed-in user yet, and the access it feeds.
    pub fn new() -> (Self, SpiralAccess) {
        let (sender, receiver) = watch::channel(None);
        (Self(sender), SpiralAccess(receiver))
    }

    /// Publish the current token, or `None` once the user signs out.
    pub fn publish(&self, token: Option<String>) {
        self.0.send_replace(token);
    }
}

/// Read access to the signed-in user's current Spiral access token.
#[derive(Clone)]
pub struct SpiralAccess(watch::Receiver<Option<String>>);

impl SpiralAccess {
    /// Access that always presents `token`, for hosted tests.
    #[cfg(test)]
    pub(crate) fn fixed(token: String) -> Self {
        let (sender, receiver) = watch::channel(Some(token));
        // The receiver keeps the last value after the sender is gone.
        drop(sender);
        Self(receiver)
    }

    /// The current token, or why Spiral cannot be reached as the user.
    pub(crate) fn token(&self) -> Result<String, String> {
        self.0
            .borrow()
            .clone()
            .ok_or_else(|| "Sign in to Spiral in Settings → SpiralDB.".to_owned())
    }

    /// Wait for the next replacement token. `None` once the publisher is gone.
    pub(crate) async fn replaced(&mut self) -> Option<Option<String>> {
        self.0.changed().await.ok()?;
        Some(self.0.borrow_and_update().clone())
    }
}

impl std::fmt::Debug for SpiralAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpiralAccess")
            .field("signed_in", &self.0.borrow().is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readers_see_replacements_and_sign_out() {
        let (publisher, access) = SpiralAccessPublisher::new();
        assert!(access.token().is_err());
        let mut follower = access.clone();
        publisher.publish(Some("first".to_owned()));
        assert_eq!(follower.replaced().await, Some(Some("first".to_owned())));
        assert_eq!(access.token().unwrap(), "first");
        publisher.publish(None);
        assert_eq!(follower.replaced().await, Some(None));
        assert!(access.token().is_err());
        drop(publisher);
        assert_eq!(follower.replaced().await, None);
        assert!(!format!("{access:?}").contains("first"));
    }
}
