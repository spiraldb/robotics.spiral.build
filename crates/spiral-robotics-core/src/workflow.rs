//! Delivery-neutral Workflow identifier and SQL-placeholder policy.
//!
//! Workflow execution substitutes only server-owned qualified relation names.
//! This module deliberately does not parse SQL or know about HTTP, edge
//! serialization, or a
//! database driver; it validates the small identifier language that is safe to
//! pass to the backend adapter after the existing read-only query gate admits
//! the surrounding statement.

use std::collections::BTreeMap;
use std::fmt;

use crate::query::UnquotedSqlIdentifier;

const PLACEHOLDER_PREFIX: &str = "{{node:";
pub const SIGLIP_SCORE_COLUMN_PREFIX: &str = "score_";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowSyntaxError(String);

impl WorkflowSyntaxError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WorkflowSyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WorkflowSyntaxError {}

/// A workflow node identity admitted by the URL/JSON naming policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkflowNodeId<'a>(&'a str);

/// Workflow node IDs are URL/JSON identities, not SQL identifiers. Hyphens are
/// allowed because IDs are never interpolated; they only index a server-owned
/// relation map.
impl<'a> TryFrom<&'a str> for WorkflowNodeId<'a> {
    type Error = WorkflowSyntaxError;

    fn try_from(node_id: &'a str) -> Result<Self, Self::Error> {
        let mut bytes = node_id.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        valid.then_some(Self(node_id)).ok_or_else(|| {
            WorkflowSyntaxError::new(format!("{node_id:?} is not a valid Workflow node ID"))
        })
    }
}

impl WorkflowNodeId<'_> {
    pub const fn as_str(&self) -> &str {
        self.0
    }
}

/// One user-facing concept used to name a SigLIP score column.
#[derive(Clone, Copy, Debug)]
pub struct SiglipConcept<'a>(&'a str);

impl<'a> From<&'a str> for SiglipConcept<'a> {
    fn from(concept: &'a str) -> Self {
        Self(concept)
    }
}

/// Turn a user-facing SigLIP concept into the safe, deterministic stem used by
/// the server-owned output schema. Collisions are resolved by the contract
/// layer, which has visibility into the full ordered concept list.
impl SiglipConcept<'_> {
    pub fn score_column_stem(self) -> String {
        let mut stem = String::new();
        let mut separator_pending = false;
        for character in self.0.trim().chars() {
            if character.is_ascii_alphanumeric() {
                if separator_pending && !stem.is_empty() {
                    stem.push('_');
                }
                stem.push(character.to_ascii_lowercase());
                separator_pending = false;
            } else if !stem.is_empty() {
                separator_pending = true;
            }
        }
        if stem.is_empty() {
            "concept".to_owned()
        } else {
            stem
        }
    }
}

/// A validated unquoted, dotted Spiral relation name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QualifiedRelation<'a>(&'a str);

/// Admit an unquoted dotted Spiral relation such as `dataset.camera_frames`.
/// Requiring at least two segments makes the catalog boundary visible and
/// prevents later code from silently inheriting a connection-local default.
impl<'a> TryFrom<&'a str> for QualifiedRelation<'a> {
    type Error = WorkflowSyntaxError;

    fn try_from(relation: &'a str) -> Result<Self, Self::Error> {
        if relation.trim() != relation || relation.contains('\0') {
            return Err(WorkflowSyntaxError::new(
                "Spiral relation names cannot contain surrounding whitespace or NUL bytes",
            ));
        }
        let segments = relation.split('.').collect::<Vec<_>>();
        if segments.len() < 2
            || segments
                .iter()
                .any(|segment| UnquotedSqlIdentifier::try_from(*segment).is_err())
        {
            return Err(WorkflowSyntaxError::new(
                "Spiral relations must be qualified dotted identifiers such as dataset.table",
            ));
        }
        Ok(Self(relation))
    }
}

impl QualifiedRelation<'_> {
    pub const fn as_str(&self) -> &str {
        self.0
    }
}

/// Workflow SQL whose node references can be inspected or substituted.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowSql<'a>(&'a str);

impl<'a> From<&'a str> for WorkflowSql<'a> {
    fn from(sql: &'a str) -> Self {
        Self(sql)
    }
}

/// Scan exact `{{node:<id>}}` placeholders in source order.
///
/// Repeated placeholders are preserved intentionally: a read-only query may
/// reference the same upstream relation more than once with different aliases.
impl WorkflowSql<'_> {
    pub fn node_placeholders(self) -> Result<Vec<String>, WorkflowSyntaxError> {
        let sql = self.0;
        let mut placeholders = Vec::new();
        let mut cursor = 0_usize;
        while let Some(relative_start) = sql[cursor..].find("{{") {
            let start = cursor + relative_start;
            if !sql[start..].starts_with(PLACEHOLDER_PREFIX) {
                return Err(WorkflowSyntaxError::new(format!(
                    "Malformed Workflow placeholder at byte {start}; expected {{{{node:<id>}}}}",
                )));
            }
            let id_start = start + PLACEHOLDER_PREFIX.len();
            let Some(relative_end) = sql[id_start..].find("}}") else {
                return Err(WorkflowSyntaxError::new(format!(
                    "Unterminated Workflow placeholder at byte {start}",
                )));
            };
            let end = id_start + relative_end;
            let id = &sql[id_start..end];
            WorkflowNodeId::try_from(id).map_err(|_| {
                WorkflowSyntaxError::new(format!(
                    "Workflow placeholder {id:?} is not a valid node ID",
                ))
            })?;
            placeholders.push(id.to_owned());
            cursor = end + 2;
        }
        if let Some(relative_end) = sql[cursor..].find("}}") {
            return Err(WorkflowSyntaxError::new(format!(
                "Unexpected Workflow placeholder terminator at byte {}",
                cursor + relative_end,
            )));
        }
        Ok(placeholders)
    }

    /// Replace placeholders with prevalidated, server-owned qualified relations.
    pub fn substitute(
        self,
        relations: &BTreeMap<String, String>,
    ) -> Result<String, WorkflowSyntaxError> {
        let placeholders = self.node_placeholders()?;
        for relation in relations.values() {
            QualifiedRelation::try_from(relation.as_str())?;
        }
        let mut substituted = self.0.to_owned();
        for id in placeholders {
            let relation = relations.get(&id).ok_or_else(|| {
                WorkflowSyntaxError::new(format!(
                    "Workflow query references unknown node placeholder {id:?}",
                ))
            })?;
            substituted = substituted.replace(&format!("{{{{node:{id}}}}}"), relation);
        }
        Ok(substituted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_relations_reject_identifier_injection() {
        for valid in [
            "spiral_robotics.episodes",
            "pick_place.camera_frames",
            "project.dataset.table_2",
        ] {
            assert!(QualifiedRelation::try_from(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "episodes",
            "dataset.",
            ".table",
            "dataset.table;DROP_TABLE_x",
            "dataset.\"table\"",
            "dataset.table --",
            " dataset.table",
            "dataset.table\0suffix",
        ] {
            assert!(QualifiedRelation::try_from(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn placeholder_scanning_is_exact_and_preserves_repeats() {
        assert_eq!(
            WorkflowSql::from(
                "SELECT * FROM {{node:frames}} f JOIN {{node:frames}} g ON f.id = g.id"
            )
            .node_placeholders()
            .unwrap(),
            vec!["frames", "frames"]
        );
        for malformed in [
            "SELECT * FROM {{frames}}",
            "SELECT * FROM {{node:}}",
            "SELECT * FROM {{node:bad.id}}",
            "SELECT * FROM {{node:frames}",
            "SELECT * FROM frames}}",
        ] {
            assert!(
                WorkflowSql::from(malformed).node_placeholders().is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn substitution_uses_only_validated_server_relations() {
        let relations =
            BTreeMap::from([("frames".to_owned(), "pick_place.camera_frames".to_owned())]);
        assert_eq!(
            WorkflowSql::from(
                "SELECT * FROM {{node:frames}} a JOIN {{node:frames}} b ON a.id = b.id",
            )
            .substitute(&relations)
            .unwrap(),
            "SELECT * FROM pick_place.camera_frames a JOIN pick_place.camera_frames b ON a.id = b.id"
        );
        assert!(
            WorkflowSql::from("SELECT * FROM {{node:missing}}")
                .substitute(&relations)
                .is_err()
        );
        let injected = BTreeMap::from([(
            "frames".to_owned(),
            "pick_place.frames; DROP TABLE x".to_owned(),
        )]);
        assert!(
            WorkflowSql::from("SELECT * FROM {{node:frames}}")
                .substitute(&injected)
                .is_err()
        );
    }

    #[test]
    fn siglip_score_stems_are_identifier_safe_and_stable() {
        assert_eq!(
            SiglipConcept::from(" Red cube ").score_column_stem(),
            "red_cube"
        );
        assert_eq!(
            SiglipConcept::from("red-cube!").score_column_stem(),
            "red_cube"
        );
        assert_eq!(SiglipConcept::from("  🤖  ").score_column_stem(), "concept");
        assert_eq!(
            SiglipConcept::from("Café / CUP").score_column_stem(),
            "caf_cup"
        );
    }
}
