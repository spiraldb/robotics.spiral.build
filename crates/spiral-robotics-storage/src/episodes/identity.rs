//! Persisted episode identity validation owned by episode storage.

/// A validated persisted episode identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EpisodeId(String);

impl EpisodeId {
    pub const MAX_BYTES: usize = 64;

    /// Parse the sequence number from Studio's generated `episode_NNNNNN` IDs.
    pub fn sequence_number(&self) -> Option<u64> {
        self.0.strip_prefix("episode_")?.parse().ok()
    }
}

impl TryFrom<&str> for EpisodeId {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        (!value.is_empty()
            && value.len() <= Self::MAX_BYTES
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            }))
        .then(|| Self(value.to_owned()))
        .ok_or("episode ID must use 1–64 letters, numbers, '_' or '-' characters")
    }
}

impl From<EpisodeId> for String {
    fn from(value: EpisodeId) -> Self {
        value.0
    }
}
