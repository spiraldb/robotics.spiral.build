//! Which recording a dataset member is.
//!
//! Spiral gives each member a UUIDv7 when its file joins the dataset, and the
//! id never changes afterwards. It is the only thing that identifies an
//! episode: reads bind it as `$1`, documents store it, and every name a
//! surface shows is derived from it.
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

/// One dataset member's id, as Spiral returns it: 32 lowercase hex digits.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EpisodeId(String);

impl EpisodeId {
    /// How many trailing digits an operator reads the id by. A UUIDv7 leads
    /// with the moment its file was imported, so every file of one import
    /// shares the head; the tail is random.
    const SHORT_DIGITS: usize = 8;

    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The random tail an operator tells episodes apart by.
    pub fn short(&self) -> &str {
        &self.0[self.0.len() - Self::SHORT_DIGITS..]
    }

    /// The id with `ordinal` as its value, for tests that need a few distinct
    /// episodes rather than real recordings.
    #[cfg(feature = "test-support")]
    pub fn fixture(ordinal: u32) -> Self {
        Self(format!("{ordinal:032x}"))
    }
}

impl TryFrom<String> for EpisodeId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 32 && value.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(format!("{value:?} is not a dataset member id"))
        }
    }
}

impl TryFrom<&str> for EpisodeId {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl From<EpisodeId> for String {
    fn from(id: EpisodeId) -> Self {
        id.0
    }
}

impl Display for EpisodeId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_the_member_uuid_spiral_returns_and_reads_by_its_tail() {
        let id = EpisodeId::try_from("01A0989FB6557788A2DD36342635E3D1").unwrap();
        assert_eq!(id.as_str(), "01a0989fb6557788a2dd36342635e3d1");
        assert_eq!(id.short(), "2635e3d1");
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            serde_json::json!("01a0989fb6557788a2dd36342635e3d1")
        );
        assert_eq!(
            serde_json::from_value::<EpisodeId>(serde_json::json!(id.as_str())).unwrap(),
            id
        );
    }

    #[test]
    fn a_name_or_a_hyphenated_uuid_is_not_a_member_id() {
        for text in [
            "Episode 7",
            "ep-1",
            "01a0989f-b655-7788-a2dd-36342635e3d1",
            "",
        ] {
            assert!(EpisodeId::try_from(text).is_err(), "{text}");
        }
        assert!(serde_json::from_value::<EpisodeId>(serde_json::json!("ep-1")).is_err());
    }
}
