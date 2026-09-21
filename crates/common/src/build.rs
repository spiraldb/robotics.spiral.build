//! Build documents: one opaque, versioned JSON payload per Build graph.
//!
//! The frontend owns the payload schema and its encoding version; the backend
//! stores, lists, and loads documents by identity without reading the payload.
//! Saves follow the workflow rule: a save carries the revision it was loaded
//! at (0 for a new document) and the stored document comes back one higher.
//! A mismatch is a `conflict` failure.
use serde::{Deserialize, Serialize};

/// Largest payload the backend accepts, in bytes of JSON text.
pub const MAX_BUILD_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
const MAX_ID_LEN: usize = 128;
const MAX_NAME_LEN: usize = 200;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildDocumentSummary {
    id: String,
    name: String,
    revision: u64,
}
impl BuildDocumentSummary {
    pub fn new(id: impl Into<String>, name: impl Into<String>, revision: u64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision,
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
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildDocument {
    id: String,
    name: String,
    /// Revision this document was loaded at; 0 for a document never saved.
    revision: u64,
    /// Frontend-owned encoding version of `payload`.
    encoding: u32,
    /// JSON text the frontend encodes and decodes. Opaque to the backend.
    payload: String,
}
impl BuildDocument {
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
    pub fn summary(&self) -> BuildDocumentSummary {
        BuildDocumentSummary::new(&self.id, &self.name, self.revision)
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
            return Err("Build document id must be 1–128 non-blank characters.".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_LEN {
            return Err("Build document name must be 1–200 characters.".into());
        }
        if self.encoding == 0 {
            return Err("Build document encoding version must be positive.".into());
        }
        if self.payload.is_empty() {
            return Err("Build document payload is empty.".into());
        }
        if self.payload.len() > MAX_BUILD_PAYLOAD_BYTES {
            return Err("Build document payload exceeds 32 MiB.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rules_reject_blank_and_oversized_values() {
        let document = BuildDocument::new("build-1", "Cloth folding", 1, "{}");
        assert_eq!(document.validate(), Ok(()));
        assert_eq!(document.revision(), 0);
        assert_eq!(
            document.summary(),
            BuildDocumentSummary::new("build-1", "Cloth folding", 0)
        );
        assert!(BuildDocument::new("", "n", 1, "{}").validate().is_err());
        assert!(
            BuildDocument::new("has space", "n", 1, "{}")
                .validate()
                .is_err()
        );
        assert!(BuildDocument::new("a", "   ", 1, "{}").validate().is_err());
        assert!(BuildDocument::new("a", "n", 0, "{}").validate().is_err());
        assert!(BuildDocument::new("a", "n", 1, "").validate().is_err());
        assert!(
            BuildDocument::new("a", "n", 1, "x".repeat(MAX_BUILD_PAYLOAD_BYTES + 1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn json_shape_is_flat_and_strict() {
        let document =
            BuildDocument::new("build-1", "Cloth folding", 1, "{\"nodes\":[]}").with_revision(3);
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["revision"], 3);
        assert_eq!(json["encoding"], 1);
        assert_eq!(json["payload"], "{\"nodes\":[]}");
        let restored: BuildDocument = serde_json::from_value(json).unwrap();
        assert_eq!(restored, document);
        assert!(
            serde_json::from_str::<BuildDocument>(
                r#"{"id":"a","name":"n","revision":0,"encoding":1,"payload":"{}","extra":1}"#
            )
            .is_err()
        );
    }
}
