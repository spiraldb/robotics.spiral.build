//! Authorization policy for user-supplied read-only queries.
//!
//! This module deliberately knows nothing about a database driver or SQL
//! execution. Adapters remain responsible for constructing and executing SQL;
//! policy code only decides whether an input is eligible for a read-only path.

use std::fmt;

const MAX_QUERY_BYTES: usize = 64 * 1024;

/// Product context in which a read-only query will run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadOnlyQueryMode {
    /// Interactive data preview.
    Preview,
    /// Training input selection.
    Training,
    /// A published curation view.
    Curation,
}

impl ReadOnlyQueryMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Preview => "Spiral preview",
            Self::Training => "Training SQL",
            Self::Curation => "Curation SQL",
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

/// One user-supplied SQL statement paired with its product authorization context.
#[derive(Clone, Copy, Debug)]
pub struct ReadOnlyQuery<'a> {
    sql: &'a str,
    mode: ReadOnlyQueryMode,
}

impl<'a> ReadOnlyQuery<'a> {
    pub const fn new(sql: &'a str, mode: ReadOnlyQueryMode) -> Self {
        Self { sql, mode }
    }

    /// Apply the conservative authorization gate for a user-supplied query.
    ///
    /// This is intentionally not a SQL parser. The database driver's parser is
    /// authoritative after this policy admits the input. This gate ignores words
    /// inside quoted values, quoted identifiers, and comments; rejects additional
    /// statements; and rejects mutating keywords in any unquoted position so that
    /// writable CTEs cannot bypass a first-keyword check.
    pub fn authorize(self) -> Result<(), QueryPolicyError> {
        let sql = self.sql.trim();
        let label = self.mode.label();
        if sql.is_empty() {
            return Err(QueryPolicyError::new(format!("{label} cannot be empty")));
        }
        if sql.len() > MAX_QUERY_BYTES {
            return Err(QueryPolicyError::new(format!(
                "{label} must be under 64 KiB"
            )));
        }

        let (tokens, semicolons) = SqlTokenScanner::new(sql).scan()?;
        if semicolons > 1 {
            return Err(QueryPolicyError::new(format!(
                "{label} must contain exactly one read-only query"
            )));
        }

        let first = tokens.first().map(String::as_str).unwrap_or_default();
        if !matches!(first, "SELECT" | "WITH") {
            return Err(QueryPolicyError::new(format!(
                "{label} must be a read-only SELECT or WITH query"
            )));
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
}

/// Whether a value is safe to interpolate as one unquoted SQL identifier.
///
/// Relation construction belongs to a database adapter. This predicate is the
/// policy boundary it must apply before interpolating caller-owned names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnquotedSqlIdentifier<'a>(&'a str);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSqlIdentifier;

impl<'a> TryFrom<&'a str> for UnquotedSqlIdentifier<'a> {
    type Error = InvalidSqlIdentifier;

    fn try_from(identifier: &'a str) -> Result<Self, Self::Error> {
        let mut bytes = identifier.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        valid
            .then_some(Self(identifier))
            .ok_or(InvalidSqlIdentifier)
    }
}

/// Extract unquoted SQL words while rejecting extra statements.
struct SqlTokenScanner<'a>(&'a str);

impl<'a> SqlTokenScanner<'a> {
    const fn new(sql: &'a str) -> Self {
        Self(sql)
    }

    fn scan(self) -> Result<(Vec<String>, usize), QueryPolicyError> {
        #[derive(Clone, Copy, Eq, PartialEq)]
        enum SqlScanState {
            Normal,
            SingleQuote,
            DoubleQuote,
            LineComment,
            BlockComment,
        }

        let bytes = self.0.as_bytes();
        let mut state = SqlScanState::Normal;
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
                SqlScanState::Normal => match (byte, next) {
                    (b'-', Some(b'-')) => {
                        flush(&mut token, &mut tokens);
                        state = SqlScanState::LineComment;
                        index += 1;
                    }
                    (b'/', Some(b'*')) => {
                        flush(&mut token, &mut tokens);
                        state = SqlScanState::BlockComment;
                        index += 1;
                    }
                    (b'\'', _) => {
                        flush(&mut token, &mut tokens);
                        state = SqlScanState::SingleQuote;
                    }
                    (b'"', _) => {
                        flush(&mut token, &mut tokens);
                        state = SqlScanState::DoubleQuote;
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
                SqlScanState::SingleQuote => {
                    if byte == b'\'' {
                        if next == Some(b'\'') {
                            index += 1;
                        } else {
                            state = SqlScanState::Normal;
                        }
                    }
                }
                SqlScanState::DoubleQuote => {
                    if byte == b'"' {
                        if next == Some(b'"') {
                            index += 1;
                        } else {
                            state = SqlScanState::Normal;
                        }
                    }
                }
                SqlScanState::LineComment => {
                    if matches!(byte, b'\n' | b'\r') {
                        state = SqlScanState::Normal;
                    }
                }
                SqlScanState::BlockComment => {
                    if byte == b'*' && next == Some(b'/') {
                        state = SqlScanState::Normal;
                        index += 1;
                    }
                }
            }
            index += 1;
        }

        flush(&mut token, &mut tokens);
        if !matches!(state, SqlScanState::Normal | SqlScanState::LineComment) {
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
            assert!(
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Preview)
                    .authorize()
                    .is_ok()
            );
            assert!(
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Training)
                    .authorize()
                    .is_ok()
            );
            assert!(
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Curation)
                    .authorize()
                    .is_ok()
            );
        }
        assert_eq!(
            ReadOnlyQuery::new("EXPLAIN SELECT 1", ReadOnlyQueryMode::Curation)
                .authorize()
                .unwrap_err()
                .to_string(),
            "Curation SQL must be a read-only SELECT or WITH query"
        );
        assert_eq!(
            ReadOnlyQuery::new("EXPLAIN SELECT 1", ReadOnlyQueryMode::Preview)
                .authorize()
                .unwrap_err()
                .to_string(),
            "Spiral preview must be a read-only SELECT or WITH query"
        );
        assert_eq!(
            ReadOnlyQuery::new("EXPLAIN SELECT 1", ReadOnlyQueryMode::Training)
                .authorize()
                .unwrap_err()
                .to_string(),
            "Training SQL must be a read-only SELECT or WITH query"
        );
    }

    #[test]
    fn writable_ctes_and_extra_statements_are_rejected() {
        assert_eq!(
            ReadOnlyQuery::new(
                "WITH removed AS (DELETE FROM episodes RETURNING *) SELECT * FROM removed",
                ReadOnlyQueryMode::Training,
            )
            .authorize()
            .unwrap_err()
            .to_string(),
            "Training SQL is read-only; \"delete\" is not allowed"
        );
        for sql in ["SELECT 1; DROP TABLE t", "SELECT 1; SELECT 2", "SELECT 1;;"] {
            assert!(
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Preview)
                    .authorize()
                    .is_err(),
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
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Preview)
                    .authorize()
                    .is_ok(),
                "{sql}"
            );
        }
        for sql in [
            "SELECT 'unterminated",
            "SELECT \"unterminated",
            "SELECT 1 /* open",
        ] {
            assert_eq!(
                ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Preview)
                    .authorize()
                    .unwrap_err()
                    .to_string(),
                "Spiral SQL contains an unterminated quote or comment"
            );
        }
    }

    #[test]
    fn size_and_empty_limits_keep_execution_bounded() {
        assert_eq!(
            ReadOnlyQuery::new(" \n\t", ReadOnlyQueryMode::Preview)
                .authorize()
                .unwrap_err()
                .to_string(),
            "Spiral preview cannot be empty"
        );
        let oversized = format!("SELECT '{}'", "x".repeat(MAX_QUERY_BYTES));
        assert_eq!(
            ReadOnlyQuery::new(&oversized, ReadOnlyQueryMode::Training)
                .authorize()
                .unwrap_err()
                .to_string(),
            "Training SQL must be under 64 KiB"
        );
    }

    #[test]
    fn identifiers_are_ascii_and_safe_for_unquoted_interpolation() {
        for valid in ["spiral_robotics_v3", "_internal", "Dataset42"] {
            assert!(UnquotedSqlIdentifier::try_from(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "3dataset",
            "spiral-robotics",
            "a.b",
            "robot arm",
            "café",
        ] {
            assert!(
                UnquotedSqlIdentifier::try_from(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
