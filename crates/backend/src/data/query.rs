//! Read-only SQL admission and preview limits.

use spiral_robotics_common::relations::{EpisodeQueryRequest, QueryRequest};
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};

use crate::OperationError;

const MAX_QUERY_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryCommand {
    statement: String,
    max_rows: usize,
}

impl QueryCommand {
    pub(super) fn new(statement: impl Into<String>, max_rows: usize) -> Self {
        Self {
            statement: statement.into(),
            max_rows,
        }
    }

    pub(super) fn statement(&self) -> &str {
        &self.statement
    }
    pub(super) fn max_rows(&self) -> usize {
        self.max_rows
    }
}

impl QueryCommand {
    /// A preview request, admitted: read-only, with a bounded row count.
    pub(super) fn admitted(request: QueryRequest) -> Result<Self, OperationError> {
        let (statement, max_rows) = request.into_parts();
        Self::new(statement, max_rows).validated()
    }

    /// One exact dataset member's query, with its statement admitted the same way.
    pub(super) fn admitted_for_episode(
        request: EpisodeQueryRequest,
    ) -> Result<EpisodeQueryRequest, OperationError> {
        let command = Self::new(request.statement(), request.max_rows()).validated()?;
        Ok(EpisodeQueryRequest::new(
            request.episode().clone(),
            command.statement,
            command.max_rows,
        ))
    }

    pub(super) fn validated(mut self) -> Result<Self, OperationError> {
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
