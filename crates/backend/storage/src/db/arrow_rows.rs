//! Typed column reads for storage-owned relational rows.
//!
//! Spiral may return a logically equivalent physical layout for a column: a
//! string can arrive as `Utf8`, `LargeUtf8`, or `Utf8View`. These helpers accept
//! equivalent string and unsigned integer layouts, read signed integers and
//! floats as stored, and
//! turn any other mismatch into one named, actionable error instead of a
//! downcast panic.

use arrow_array::{
    Array, LargeStringArray, RecordBatch, StringViewArray, UInt32Array, UInt64Array,
};

/// Column reads scoped to one relation, so every failure names the relation.
pub(crate) struct ArrowColumns<'a> {
    batch: &'a RecordBatch,
    label: &'a str,
}

impl<'a> ArrowColumns<'a> {
    pub(crate) const fn new(batch: &'a RecordBatch, label: &'a str) -> Self {
        Self { batch, label }
    }

    fn column(&self, name: &str) -> Result<&'a dyn Array, String> {
        self.batch
            .column_by_name(name)
            .map(AsRef::as_ref)
            .ok_or_else(|| format!("stored {} row has no {name} column", self.label))
    }

    fn missing(&self, name: &str) -> String {
        format!("stored {} row has a null {name}", self.label)
    }

    fn mismatch(&self, name: &str, expected: &str) -> String {
        format!("stored {} column {name} is not {expected}", self.label)
    }

    pub(crate) fn optional_text(&self, name: &str, row: usize) -> Result<Option<&'a str>, String> {
        let column = self.column(name)?;
        if column.is_null(row) {
            return Ok(None);
        }
        if let Some(values) = column.as_any().downcast_ref::<arrow_array::StringArray>() {
            return Ok(Some(values.value(row)));
        }
        if let Some(values) = column.as_any().downcast_ref::<LargeStringArray>() {
            return Ok(Some(values.value(row)));
        }
        column
            .as_any()
            .downcast_ref::<StringViewArray>()
            .map(|values| Some(values.value(row)))
            .ok_or_else(|| self.mismatch(name, "text"))
    }

    pub(crate) fn text(&self, name: &str, row: usize) -> Result<&'a str, String> {
        self.optional_text(name, row)?
            .ok_or_else(|| self.missing(name))
    }

    pub(crate) fn optional_unsigned(&self, name: &str, row: usize) -> Result<Option<u64>, String> {
        let column = self.column(name)?;
        if column.is_null(row) {
            return Ok(None);
        }
        if let Some(values) = column.as_any().downcast_ref::<UInt64Array>() {
            return Ok(Some(values.value(row)));
        }
        column
            .as_any()
            .downcast_ref::<UInt32Array>()
            .map(|values| Some(u64::from(values.value(row))))
            .ok_or_else(|| self.mismatch(name, "an unsigned integer"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("label", DataType::Utf8, true),
            Field::new("count", DataType::UInt64, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![None::<&str>])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(7_u64)])) as ArrayRef,
            ],
        )
        .unwrap()
    }

    #[test]
    fn null_and_missing_columns_are_named_in_the_error() {
        let batch = batch();
        let columns = ArrowColumns::new(&batch, "example");
        assert_eq!(columns.optional_text("label", 0).unwrap(), None);
        assert!(columns.text("label", 0).unwrap_err().contains("null label"));
        assert!(
            columns
                .text("absent", 0)
                .unwrap_err()
                .contains("no absent column")
        );
        assert!(
            columns
                .text("count", 0)
                .unwrap_err()
                .contains("count is not text")
        );
    }

    #[test]
    fn unsigned_column_reads_the_count() {
        let batch = batch();
        let columns = ArrowColumns::new(&batch, "example");
        assert_eq!(columns.optional_unsigned("count", 0).unwrap(), Some(7));
    }
}
