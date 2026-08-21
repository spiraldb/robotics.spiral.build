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

use crate::query::is_identifier;

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

/// Workflow node IDs are URL/JSON identities, not SQL identifiers. Hyphens are
/// allowed because IDs are never interpolated; they only index a server-owned
/// relation map.
pub fn is_workflow_node_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Turn a user-facing SigLIP concept into the safe, deterministic stem used by
/// the server-owned output schema. Collisions are resolved by the contract
/// layer, which has visibility into the full ordered concept list.
pub fn siglip_score_column_stem(concept: &str) -> String {
    let mut stem = String::new();
    let mut separator_pending = false;
    for character in concept.trim().chars() {
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

/// Admit an unquoted dotted Spiral relation such as `dataset.camera_frames`.
/// Requiring at least two segments makes the catalog boundary visible and
/// prevents later code from silently inheriting a connection-local default.
pub fn validate_qualified_relation(value: &str) -> Result<(), WorkflowSyntaxError> {
    if value.trim() != value || value.contains('\0') {
        return Err(WorkflowSyntaxError::new(
            "Spiral relation names cannot contain surrounding whitespace or NUL bytes",
        ));
    }
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() < 2 || segments.iter().any(|segment| !is_identifier(segment)) {
        return Err(WorkflowSyntaxError::new(
            "Spiral relations must be qualified dotted identifiers such as dataset.table",
        ));
    }
    Ok(())
}

/// Scan exact `{{node:<id>}}` placeholders in source order.
///
/// Repeated placeholders are preserved intentionally: a read-only query may
/// reference the same upstream relation more than once with different aliases.
pub fn scan_node_placeholders(sql: &str) -> Result<Vec<String>, WorkflowSyntaxError> {
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
        if !is_workflow_node_id(id) {
            return Err(WorkflowSyntaxError::new(format!(
                "Workflow placeholder {id:?} is not a valid node ID",
            )));
        }
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
pub fn substitute_node_placeholders(
    sql: &str,
    relations: &BTreeMap<String, String>,
) -> Result<String, WorkflowSyntaxError> {
    let placeholders = scan_node_placeholders(sql)?;
    for relation in relations.values() {
        validate_qualified_relation(relation)?;
    }
    let mut substituted = sql.to_owned();
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
            assert!(validate_qualified_relation(valid).is_ok(), "{valid}");
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
            assert!(validate_qualified_relation(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn placeholder_scanning_is_exact_and_preserves_repeats() {
        assert_eq!(
            scan_node_placeholders(
                "SELECT * FROM {{node:frames}} f JOIN {{node:frames}} g ON f.id = g.id"
            )
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
            assert!(scan_node_placeholders(malformed).is_err(), "{malformed}");
        }
    }

    #[test]
    fn substitution_uses_only_validated_server_relations() {
        let relations =
            BTreeMap::from([("frames".to_owned(), "pick_place.camera_frames".to_owned())]);
        assert_eq!(
            substitute_node_placeholders(
                "SELECT * FROM {{node:frames}} a JOIN {{node:frames}} b ON a.id = b.id",
                &relations,
            )
            .unwrap(),
            "SELECT * FROM pick_place.camera_frames a JOIN pick_place.camera_frames b ON a.id = b.id"
        );
        assert!(
            substitute_node_placeholders("SELECT * FROM {{node:missing}}", &relations).is_err()
        );
        let injected = BTreeMap::from([(
            "frames".to_owned(),
            "pick_place.frames; DROP TABLE x".to_owned(),
        )]);
        assert!(substitute_node_placeholders("SELECT * FROM {{node:frames}}", &injected).is_err());
    }

    #[test]
    fn siglip_score_stems_are_identifier_safe_and_stable() {
        assert_eq!(siglip_score_column_stem(" Red cube "), "red_cube");
        assert_eq!(siglip_score_column_stem("red-cube!"), "red_cube");
        assert_eq!(siglip_score_column_stem("  🤖  "), "concept");
        assert_eq!(siglip_score_column_stem("Café / CUP"), "caf_cup");
    }
}
