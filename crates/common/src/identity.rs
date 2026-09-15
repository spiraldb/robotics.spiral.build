//! Stable identities shared by browser and native application boundaries.

/// A validated logical dataset identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DatasetId(String);

impl DatasetId {
    pub const MAX_BYTES: usize = 96;

    /// Whether a string is already in the canonical persisted form.
    pub fn is_canonical(value: &str) -> bool {
        Self::try_from(value).is_ok_and(|identity| identity.as_str() == value)
    }
}

impl TryFrom<&str> for DatasetId {
    type Error = &'static str;

    fn try_from(dataset_id: &str) -> Result<Self, Self::Error> {
        let dataset_id = dataset_id.trim();
        if dataset_id.is_empty()
            || dataset_id.len() > Self::MAX_BYTES
            || !dataset_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err("dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters");
        }
        Ok(Self(dataset_id.to_owned()))
    }
}

impl DatasetId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<DatasetId> for String {
    fn from(dataset_id: DatasetId) -> Self {
        dataset_id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_identity_trims_and_guards_the_persisted_key() {
        assert_eq!(
            DatasetId::try_from(" 2026.pick-place_v1 ")
                .unwrap()
                .as_str(),
            "2026.pick-place_v1"
        );
        for invalid in ["", "pick place", "../pick", "x/y"] {
            assert!(DatasetId::try_from(invalid).is_err(), "{invalid:?}");
        }
    }
}
