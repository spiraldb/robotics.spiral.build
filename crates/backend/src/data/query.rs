//! Read-only SQL admission and preview limits.

use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};

use crate::OperationError;

const MAX_QUERY_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryCommand {
    statement: String,
    max_rows: usize,
}

impl QueryCommand {
    pub(crate) fn new(statement: impl Into<String>, max_rows: usize) -> Self {
        Self {
            statement: statement.into(),
            max_rows,
        }
    }

    pub(crate) fn statement(&self) -> &str {
        &self.statement
    }
    pub(crate) fn max_rows(&self) -> usize {
        self.max_rows
    }
}

impl QueryCommand {
    pub(crate) fn validated(mut self) -> Result<Self, OperationError> {
        ReadOnlyQuery::new(&self.statement, ReadOnlyQueryMode::Preview)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.max_rows = self.max_rows.clamp(1, MAX_QUERY_ROWS);
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_admission_bounds_rows_and_preserves_the_statement() {
        for (requested, admitted) in [(0, 1), (42, 42), (usize::MAX, 1_000)] {
            let command = QueryCommand::new("SELECT * FROM episodes", requested)
                .validated()
                .unwrap();
            assert_eq!(command.statement(), "SELECT * FROM episodes");
            assert_eq!(command.max_rows(), admitted);
        }
    }

    #[test]
    fn preview_admission_rejects_writes() {
        assert!(matches!(
            QueryCommand::new("DELETE FROM episodes", 100).validated(),
            Err(OperationError::InvalidInput(_))
        ));
    }
}
