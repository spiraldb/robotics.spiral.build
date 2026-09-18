//! Validated relation names for SQL entry points.
use super::query::UnquotedSqlIdentifier;
use std::fmt;

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

/// A validated unquoted Spiral relation path within the selected project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QualifiedRelation<'a>(&'a str);

/// Admit root resources and dotted resource paths within the selected project.
impl<'a> TryFrom<&'a str> for QualifiedRelation<'a> {
    type Error = WorkflowSyntaxError;

    fn try_from(relation: &'a str) -> Result<Self, Self::Error> {
        if relation.trim() != relation || relation.contains('\0') {
            return Err(WorkflowSyntaxError::new(
                "Spiral relation names cannot contain surrounding whitespace or NUL bytes",
            ));
        }
        if relation
            .split('.')
            .any(|segment| UnquotedSqlIdentifier::try_from(segment).is_err())
        {
            return Err(WorkflowSyntaxError::new(
                "Spiral relations must contain safe identifiers separated by dots",
            ));
        }
        Ok(Self(relation))
    }
}
