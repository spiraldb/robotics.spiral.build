//! Statements run on a DataFusion session, which every Studio database
//! queries through: their parameters bound as typed values, their rows
//! streamed as they are computed.

use datafusion::common::metadata::{FieldMetadata, ScalarAndMetadata};
use datafusion::common::{ParamValues, ScalarValue};
use datafusion::error::DataFusionError;
use datafusion::prelude::SessionContext;
use futures::{StreamExt as _, TryStreamExt as _};

use super::database::{
    DatabaseError, DatabaseErrorKind, DatabaseResult, RecordBatchStream, SqlStatement,
};

impl SqlStatement {
    /// Plan the statement on `context` with its parameters bound, `$1`
    /// onwards, each keeping its field's metadata, and stream its rows.
    pub(super) async fn stream_on(
        &self,
        context: &SessionContext,
    ) -> DatabaseResult<RecordBatchStream> {
        let mut frame = context
            .sql(self.as_str())
            .await
            .map_err(DatabaseError::from_datafusion)?;
        if let Some(parameters) = self.parameters() {
            let values = parameters
                .schema()
                .fields()
                .iter()
                .zip(parameters.columns())
                .map(|(field, column)| {
                    let value = ScalarValue::try_from_array(column, 0)?;
                    Ok(ScalarAndMetadata::new(
                        value,
                        Some(FieldMetadata::new_from_field(field)),
                    ))
                })
                .collect::<Result<Vec<_>, DataFusionError>>()
                .map_err(DatabaseError::from_datafusion)?;
            frame = frame
                .with_param_values(ParamValues::List(values))
                .map_err(DatabaseError::from_datafusion)?;
        }
        let stream = frame
            .execute_stream()
            .await
            .map_err(DatabaseError::from_datafusion)?;
        let schema = stream.schema();
        Ok(RecordBatchStream::new(
            schema,
            stream.map_err(DatabaseError::from_datafusion).boxed(),
        ))
    }
}

impl DatabaseError {
    /// A statement that does not plan is the caller's to fix; anything else
    /// failed while it ran.
    pub(super) fn from_datafusion(error: DataFusionError) -> Self {
        let kind = match error.find_root() {
            DataFusionError::SQL(..)
            | DataFusionError::Plan(_)
            | DataFusionError::SchemaError(..)
            | DataFusionError::NotImplemented(_) => DatabaseErrorKind::InvalidInput,
            DataFusionError::External(_) => DatabaseErrorKind::Unavailable,
            _ => DatabaseErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}
