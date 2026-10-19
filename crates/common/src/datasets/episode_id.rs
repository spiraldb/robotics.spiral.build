//! Which episode of its dataset a recording is.
//!
//! Studio numbers each dataset's episodes as it writes them, from 0, and an
//! episode keeps its number afterwards. Episodes are written in that order, so
//! every table of the dataset holds an episode's rows next to each other and a
//! read of one episode skips the rest of a table by its statistics. The number
//! is the only thing that identifies an episode within its dataset: reads bind
//! it as `$1`, documents store it, and every name a surface shows is derived
//! from it.
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

/// One episode's number within its dataset.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EpisodeId(u64);

impl EpisodeId {
    pub fn new(number: u64) -> Self {
        Self(number)
    }
    pub fn number(&self) -> u64 {
        self.0
    }
    /// The episode after this one, which the next episode written is given.
    pub fn next(&self) -> Self {
        Self(self.0 + 1)
    }

    /// The id with `ordinal` as its number, for tests that need a few
    /// distinct episodes rather than real recordings.
    #[cfg(feature = "test-support")]
    pub fn fixture(ordinal: u32) -> Self {
        Self(u64::from(ordinal))
    }
}

impl FromStr for EpisodeId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse()
            .map(Self)
            .map_err(|_| format!("{text:?} is not an episode number"))
    }
}

impl Display for EpisodeId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_its_episode_number_as_text_and_as_json() {
        let id = EpisodeId::new(42);
        assert_eq!(id.to_string(), "42");
        assert_eq!("42".parse::<EpisodeId>().unwrap(), id);
        assert_eq!(id.next(), EpisodeId::new(43));
        assert_eq!(serde_json::to_value(&id).unwrap(), serde_json::json!(42));
        assert_eq!(
            serde_json::from_value::<EpisodeId>(serde_json::json!(42)).unwrap(),
            id
        );
    }

    #[test]
    fn anything_but_a_number_is_no_episode() {
        for text in [
            "Episode 7",
            "-1",
            "01a0989fb6557788a2dd36342635e3d1",
            "",
            "' OR true --",
        ] {
            assert!(text.parse::<EpisodeId>().is_err(), "{text}");
        }
        assert!(serde_json::from_value::<EpisodeId>(serde_json::json!("7")).is_err());
    }
}
