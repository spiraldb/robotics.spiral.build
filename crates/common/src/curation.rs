//! Curations: one opaque, versioned JSON graph per document, and the catalog
//! views their outputs are published as.
//!
//! The frontend owns the payload schema and its encoding version; the backend
//! stores, lists, and loads documents by identity without reading the payload.
//! A save carries the revision it was loaded at (0 for a new document) and the
//! stored document comes back one higher; a delete carries it too. A mismatch
//! is a `conflict` failure.
//!
//! The frontend also compiles each output to one read-only statement; the
//! backend publishes it under a plain name at the catalog root. A node's
//! preview queries run on the backend as one polled run (`preview`).
mod preview;

pub use preview::{PreviewQuery, PreviewQueryState, PreviewRunStatus};

use crate::sql::UnquotedSqlIdentifier;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

/// Largest payload the backend accepts, in bytes of JSON text.
pub const MAX_CURATION_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
const MAX_ID_LEN: usize = 128;
const MAX_NAME_LEN: usize = 200;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationSummary {
    id: String,
    name: String,
}
impl CurationSummary {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationDocument {
    id: String,
    name: String,
    /// Revision this document was loaded at; 0 for a document never saved.
    revision: u64,
    /// Frontend-owned encoding version of `payload`.
    encoding: u32,
    /// JSON text the frontend encodes and decodes. Opaque to the backend.
    payload: String,
}
impl CurationDocument {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        encoding: u32,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision: 0,
            encoding,
            payload: payload.into(),
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn encoding(&self) -> u32 {
        self.encoding
    }
    pub fn payload(&self) -> &str {
        &self.payload
    }
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn summary(&self) -> CurationSummary {
        CurationSummary::new(&self.id, &self.name)
    }
    /// Identity and size rules both sides enforce before storing or sending.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > MAX_ID_LEN
            || self
                .id
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err("Curation id must be 1–128 non-blank characters.".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_LEN {
            return Err("Curation name must be 1–200 characters.".into());
        }
        if self.encoding == 0 {
            return Err("Curation encoding version must be positive.".into());
        }
        if self.payload.is_empty() {
            return Err("Curation payload is empty.".into());
        }
        if self.payload.len() > MAX_CURATION_PAYLOAD_BYTES {
            return Err("Curation payload exceeds 32 MiB.".into());
        }
        Ok(())
    }
}

/// The name a published curation view is read by: one plain identifier at
/// the catalog root, spelled as the reader types it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewName(String);

impl TryFrom<&str> for ViewName {
    type Error = String;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        UnquotedSqlIdentifier::try_from(name)
            .map(|_| Self(name.to_owned()))
            .map_err(|_| {
                "A view name is letters, digits, and underscores, and does not start with a digit."
                    .to_owned()
            })
    }
}

impl ViewName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ViewName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Publish one curation output: the view's name and the read-only statement
/// the view is defined as.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRequest {
    name: String,
    statement: String,
}
impl PublishRequest {
    pub fn new(name: impl Into<String>, statement: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            statement: statement.into(),
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn statement(&self) -> &str {
        &self.statement
    }
}

/// Delete one saved curation at the revision it was loaded at, so a delete
/// never removes a version its caller has not seen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationDeletion {
    id: String,
    revision: u64,
}
impl CurationDeletion {
    pub fn new(id: impl Into<String>, revision: u64) -> Self {
        Self {
            id: id.into(),
            revision,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Drop one published curation view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnpublishRequest {
    name: String,
}
impl UnpublishRequest {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_name_is_one_plain_identifier() {
        for valid in ["folds", "_draft", "Moving2"] {
            assert_eq!(ViewName::try_from(valid).unwrap().as_str(), valid);
        }
        for invalid in ["", "2folds", "my folds", "public.folds", "folds;", "fold-s"] {
            assert!(ViewName::try_from(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn identity_rules_reject_blank_and_oversized_values() {
        let document = CurationDocument::new("curation-1", "Cloth folding", 1, "{}");
        assert_eq!(document.validate(), Ok(()));
        assert_eq!(document.revision(), 0);
        assert_eq!(
            document.summary(),
            CurationSummary::new("curation-1", "Cloth folding")
        );
        assert!(CurationDocument::new("", "n", 1, "{}").validate().is_err());
        assert!(
            CurationDocument::new("has space", "n", 1, "{}")
                .validate()
                .is_err()
        );
        assert!(
            CurationDocument::new("a", "   ", 1, "{}")
                .validate()
                .is_err()
        );
        assert!(CurationDocument::new("a", "n", 0, "{}").validate().is_err());
        assert!(CurationDocument::new("a", "n", 1, "").validate().is_err());
        assert!(
            CurationDocument::new("a", "n", 1, "x".repeat(MAX_CURATION_PAYLOAD_BYTES + 1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn json_shape_is_flat_and_strict() {
        let document = CurationDocument::new("curation-1", "Cloth folding", 1, "{\"nodes\":[]}")
            .with_revision(3);
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["revision"], 3);
        assert_eq!(json["encoding"], 1);
        assert_eq!(json["payload"], "{\"nodes\":[]}");
        let restored: CurationDocument = serde_json::from_value(json).unwrap();
        assert_eq!(restored, document);
        assert!(
            serde_json::from_str::<CurationDocument>(
                r#"{"id":"a","name":"n","revision":0,"encoding":1,"payload":"{}","extra":1}"#
            )
            .is_err()
        );
    }
}
