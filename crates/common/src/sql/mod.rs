//! Shared SQL authorization, identifier validation, literal escaping, and
//! the nested values statements build and read, in DataFusion's dialect.

mod query;

pub use query::{
    InvalidSqlIdentifier, QueryPolicyError, ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier,
};

mod literal;
pub use literal::SqlLiteral;

mod nested;
pub use nested::{ListItems, SqlList, SqlStruct};
