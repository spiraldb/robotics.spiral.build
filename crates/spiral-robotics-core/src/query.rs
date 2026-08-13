//! Authorization policy for user-supplied read-only queries.
//!
//! This module deliberately knows nothing about a database driver or SQL
//! execution. Adapters remain responsible for constructing and executing SQL;
//! policy code only decides whether an input is eligible for a read-only path.

use std::fmt;

const MAX_QUERY_BYTES: usize = 64 * 1024;

/// Product context in which a read-only query will run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadOnlyMode {
    /// Interactive data preview, where explaining a query is useful and safe.
    Preview,
    /// Training input selection, which must directly produce rows.
    Training,
}

impl ReadOnlyMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Preview => "Spiral preview",
            Self::Training => "Training SQL",
        }
    }
}

/// A stable, delivery-neutral rejection from the query authorization policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryPolicyError(String);

impl QueryPolicyError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for QueryPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for QueryPolicyError {}

/// Apply the conservative authorization gate for a user-supplied query.
///
/// This is intentionally not a SQL parser. The database driver's parser is
/// authoritative after this policy admits the input. This gate ignores words
/// inside quoted values, quoted identifiers, and comments; rejects additional
/// statements; and rejects mutating keywords in any unquoted position so that
/// writable CTEs cannot bypass a first-keyword check.
pub fn ensure_read_only(sql: &str, mode: ReadOnlyMode) -> Result<(), QueryPolicyError> {
    let sql = sql.trim();
    let label = mode.label();
    if sql.is_empty() {
        return Err(QueryPolicyError::new(format!("{label} cannot be empty")));
    }
    if sql.len() > MAX_QUERY_BYTES {
        return Err(QueryPolicyError::new(format!(
            "{label} must be under 64 KiB"
        )));
    }

    let (tokens, semicolons) = sql_tokens(sql)?;
    if semicolons > 1 {
        return Err(QueryPolicyError::new(format!(
            "{label} must contain exactly one read-only query"
        )));
    }

    let first = tokens.first().map(String::as_str).unwrap_or_default();
    let allowed = match mode {
        ReadOnlyMode::Preview => matches!(first, "SELECT" | "WITH" | "EXPLAIN"),
        ReadOnlyMode::Training => matches!(first, "SELECT" | "WITH"),
    };
    if !allowed {
        return Err(QueryPolicyError::new(match mode {
            ReadOnlyMode::Preview => "Spiral preview accepts SELECT, WITH, or EXPLAIN queries only",
            ReadOnlyMode::Training => "Training SQL must be a read-only SELECT or WITH query",
        }));
    }

    const MUTATING_KEYWORDS: &[&str] = &[
        "ALTER", "ATTACH", "CALL", "COPY", "CREATE", "DELETE", "DETACH", "DROP", "EXECUTE",
        "GRANT", "INSERT", "MERGE", "REFRESH", "REPLACE", "REVOKE", "TRUNCATE", "UPDATE",
    ];
    if let Some(keyword) = tokens
        .iter()
        .find(|token| MUTATING_KEYWORDS.contains(&token.as_str()))
    {
        return Err(QueryPolicyError::new(format!(
            "{label} is read-only; {:?} is not allowed",
            keyword.to_ascii_lowercase()
        )));
    }
    Ok(())
}

/// Whether a value is safe to interpolate as one unquoted SQL identifier.
///
/// Relation construction belongs to a database adapter. This predicate is the
/// policy boundary it must apply before interpolating caller-owned names.
pub fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Extract unquoted SQL words while rejecting extra statements.
fn sql_tokens(sql: &str) -> Result<(Vec<String>, usize), QueryPolicyError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        LineComment,
        BlockComment,
    }

    let bytes = sql.as_bytes();
    let mut state = State::Normal;
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut semicolons = 0;
    let mut statement_token_count = None;
    let mut index = 0;
    let flush = |token: &mut String, tokens: &mut Vec<String>| {
        if !token.is_empty() {
            tokens.push(std::mem::take(token).to_ascii_uppercase());
        }
    };

    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Normal => match (byte, next) {
                (b'-', Some(b'-')) => {
                    flush(&mut token, &mut tokens);
                    state = State::LineComment;
                    index += 1;
                }
                (b'/', Some(b'*')) => {
                    flush(&mut token, &mut tokens);
                    state = State::BlockComment;
                    index += 1;
                }
                (b'\'', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::SingleQuote;
                }
                (b'"', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::DoubleQuote;
                }
                (b';', _) => {
                    flush(&mut token, &mut tokens);
                    semicolons += 1;
                    statement_token_count.get_or_insert(tokens.len());
                }
                _ if byte.is_ascii_alphanumeric() || byte == b'_' => {
                    token.push(char::from(byte));
                }
                _ => flush(&mut token, &mut tokens),
            },
            State::SingleQuote => {
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::LineComment => {
                if matches!(byte, b'\n' | b'\r') {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    state = State::Normal;
                    index += 1;
                }
            }
        }
        index += 1;
    }

    flush(&mut token, &mut tokens);
    if !matches!(state, State::Normal | State::LineComment) {
        return Err(QueryPolicyError::new(
            "Spiral SQL contains an unterminated quote or comment",
        ));
    }
    if statement_token_count.is_some_and(|count| tokens.len() != count) {
        return Err(QueryPolicyError::new(
            "Spiral SQL must contain exactly one read-only query",
        ));
    }
    Ok((tokens, semicolons))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_admit_only_their_read_only_entry_points() {
        for sql in [
            "SELECT * FROM episodes",
            "WITH recent AS (SELECT 1) SELECT * FROM recent",
        ] {
            assert!(ensure_read_only(sql, ReadOnlyMode::Preview).is_ok());
            assert!(ensure_read_only(sql, ReadOnlyMode::Training).is_ok());
        }
        assert!(ensure_read_only("EXPLAIN SELECT 1", ReadOnlyMode::Preview).is_ok());
        assert_eq!(
            ensure_read_only("EXPLAIN SELECT 1", ReadOnlyMode::Training)
                .unwrap_err()
                .to_string(),
            "Training SQL must be a read-only SELECT or WITH query"
        );
    }

    #[test]
    fn writable_ctes_and_extra_statements_are_rejected() {
        assert_eq!(
            ensure_read_only(
                "WITH removed AS (DELETE FROM episodes RETURNING *) SELECT * FROM removed",
                ReadOnlyMode::Training,
            )
            .unwrap_err()
            .to_string(),
            "Training SQL is read-only; \"delete\" is not allowed"
        );
        for sql in ["SELECT 1; DROP TABLE t", "SELECT 1; SELECT 2", "SELECT 1;;"] {
            assert!(
                ensure_read_only(sql, ReadOnlyMode::Preview).is_err(),
                "{sql}"
            );
        }
    }

    #[test]
    fn comments_and_quoted_contents_cannot_inject_policy_tokens() {
        for sql in [
            "-- DELETE FROM episodes\nSELECT 1",
            "/* UPDATE episodes */ SELECT 1",
            "SELECT 'DELETE; DROP', \"UPDATE\"; -- harmless",
            "SELECT 'it''s still one value'",
            "SELECT \"a\"\"quoted\"",
        ] {
            assert!(
                ensure_read_only(sql, ReadOnlyMode::Preview).is_ok(),
                "{sql}"
            );
        }
        for sql in [
            "SELECT 'unterminated",
            "SELECT \"unterminated",
            "SELECT 1 /* open",
        ] {
            assert_eq!(
                ensure_read_only(sql, ReadOnlyMode::Preview)
                    .unwrap_err()
                    .to_string(),
                "Spiral SQL contains an unterminated quote or comment"
            );
        }
    }

    #[test]
    fn size_and_empty_limits_keep_execution_bounded() {
        assert_eq!(
            ensure_read_only(" \n\t", ReadOnlyMode::Preview)
                .unwrap_err()
                .to_string(),
            "Spiral preview cannot be empty"
        );
        let oversized = format!("SELECT '{}'", "x".repeat(MAX_QUERY_BYTES));
        assert_eq!(
            ensure_read_only(&oversized, ReadOnlyMode::Training)
                .unwrap_err()
                .to_string(),
            "Training SQL must be under 64 KiB"
        );
    }

    #[test]
    fn identifiers_are_ascii_and_safe_for_unquoted_interpolation() {
        for valid in ["spiral_robotics_v3", "_internal", "Dataset42"] {
            assert!(is_identifier(valid), "{valid}");
        }
        for invalid in [
            "",
            "3dataset",
            "spiral-robotics",
            "a.b",
            "robot arm",
            "café",
        ] {
            assert!(!is_identifier(invalid), "{invalid}");
        }
    }
}
