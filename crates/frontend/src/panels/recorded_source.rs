//! Recorded media identity and loading state shared by visualization components.

/// Explicit recorded-source lifecycle. `previous` may preserve the last frame
/// of the same episode while the next time sample loads, but it never carries a
/// Live value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LoadState<T> {
    Loading { previous: Option<T> },
    Ready(T),
    Unavailable { reason: String },
    Failed { reason: String, previous: Option<T> },
}

impl<T> LoadState<T> {
    fn renderable(&self) -> Option<&T> {
        match self {
            Self::Loading { previous } | Self::Failed { previous, .. } => previous.as_ref(),
            Self::Ready(value) => Some(value),
            Self::Unavailable { .. } => None,
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Loading { .. } | Self::Ready(_) => None,
            Self::Unavailable { reason } | Self::Failed { reason, .. } => Some(reason),
        }
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    fn map<U>(self, mut transform: impl FnMut(T) -> U) -> LoadState<U> {
        match self {
            Self::Loading { previous } => LoadState::Loading {
                previous: previous.map(&mut transform),
            },
            Self::Ready(value) => LoadState::Ready(transform(value)),
            Self::Unavailable { reason } => LoadState::Unavailable { reason },
            Self::Failed { reason, previous } => LoadState::Failed {
                reason,
                previous: previous.map(transform),
            },
        }
    }
}

/// A recorded value tied to the identity and time it was requested for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordedSource<T> {
    episode: String,
    seconds: f64,
    state: LoadState<T>,
}

impl<T> RecordedSource<T> {
    pub(crate) fn new(episode: impl Into<String>, seconds: f64, state: LoadState<T>) -> Self {
        Self {
            episode: episode.into(),
            seconds,
            state,
        }
    }

    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }

    pub(crate) const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub(crate) fn set_seconds(&mut self, seconds: f64) {
        self.seconds = seconds;
    }

    pub(crate) const fn state(&self) -> &LoadState<T> {
        &self.state
    }

    pub(crate) fn renderable(&self) -> Option<&T> {
        self.state.renderable()
    }

    pub(super) fn map<U>(self, transform: impl FnMut(T) -> U) -> RecordedSource<U> {
        RecordedSource {
            episode: self.episode,
            seconds: self.seconds,
            state: self.state.map(transform),
        }
    }
}
