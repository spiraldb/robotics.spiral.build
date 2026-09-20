//! Shared SQL authorization, identifier validation, and literal escaping.

mod query;

pub use query::{
    InvalidSqlIdentifier, QueryPolicyError, ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier,
};

mod literal;
pub use literal::SqlLiteral;
