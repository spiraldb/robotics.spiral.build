//! Lifetime guard for a read transaction on an existing connection.

use spiral_engine::sql::{ConnectionCtx, TransactionCtx};

use super::{DatabaseError, DatabaseResult};

/// Reuses the caller's transaction, or begins one on the same connection.
/// Only a transaction begun by this guard is rolled back when it finishes
/// or the read is cancelled.
pub(super) struct ReadTransactionGuard<'a> {
    transaction: TransactionCtx,
    cleanup_connection: Option<&'a ConnectionCtx>,
}

impl<'a> ReadTransactionGuard<'a> {
    pub(super) async fn open(connection: &'a ConnectionCtx) -> DatabaseResult<Self> {
        let (transaction, cleanup_connection) = match connection.active_transaction() {
            Some(transaction) => (transaction, None),
            None => (
                connection
                    .begin_connection_transaction()
                    .await
                    .map_err(DatabaseError::from_external)?,
                Some(connection),
            ),
        };
        Ok(Self {
            transaction,
            cleanup_connection,
        })
    }

    pub(super) fn transaction(&self) -> &TransactionCtx {
        &self.transaction
    }

    pub(super) fn finish<T>(mut self, result: DatabaseResult<T>) -> DatabaseResult<T> {
        match (result, self.close()) {
            (result, Ok(())) => result,
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(operation), Err(cleanup)) => Err(DatabaseError::invalid_state(format!(
                "{operation}; closing the read transaction also failed: {cleanup}"
            ))),
        }
    }

    fn close(&mut self) -> DatabaseResult<()> {
        let Some(connection) = self.cleanup_connection.take() else {
            return Ok(());
        };
        connection
            .rollback_transaction()
            .map_err(DatabaseError::from_external)
    }
}

impl Drop for ReadTransactionGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::warn!(%error, "could not close cancelled read transaction");
        }
    }
}
