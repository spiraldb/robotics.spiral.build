//! Stable domain identities shared by recording, training, and persistence.

use std::fmt;

/// A validated logical dataset identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DatasetId(String);

impl DatasetId {
    pub const MAX_BYTES: usize = 96;

    pub fn parse(value: &str) -> Result<Self, InvalidDatasetId> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > Self::MAX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(InvalidDatasetId);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidDatasetId;

impl fmt::Display for InvalidDatasetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters")
    }
}

impl std::error::Error for InvalidDatasetId {}

/// Convert an external immutable ID into one SQL-safe generated-name component.
pub fn identifier_component(value: &str) -> String {
    let rendered = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if rendered == value && !rendered.is_empty() {
        return rendered;
    }
    let hash = value
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{rendered}_{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_identity_trims_and_guards_the_persisted_key() {
        assert_eq!(
            DatasetId::parse(" 2026.pick-place_v1 ").unwrap().as_str(),
            "2026.pick-place_v1"
        );
        for invalid in ["", "pick place", "../pick", "x/y"] {
            assert!(DatasetId::parse(invalid).is_err(), "{invalid:?}");
        }
    }
}
