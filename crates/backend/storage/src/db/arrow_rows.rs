//! Typed column reads for storage-owned relational rows.
//!
//! Spiral may return a logically equivalent physical layout for a column: a
//! string can arrive as `Utf8`, `LargeUtf8`, or `Utf8View`, and a list as
//! `List` or `LargeList`. These helpers accept every equivalent layout and
//! turn any other mismatch into one named, actionable error instead of a
//! downcast panic.

use arrow_array::builder::{Float64Builder, ListBuilder, StringBuilder, UInt64Builder};
use arrow_array::{
    Array, LargeStringArray, RecordBatch, StringViewArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field};
use std::sync::Arc;

/// The one list layout storage writes for a bounded collection of strings.
pub(crate) struct StringListColumn;

impl StringListColumn {
    pub(crate) fn data_type() -> DataType {
        DataType::List(Arc::new(Field::new("item", DataType::Utf8, false)))
    }

    pub(crate) fn builder() -> ListBuilder<StringBuilder> {
        ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
            "item",
            DataType::Utf8,
            false,
        )))
    }
}

/// The one list layout storage writes for a per-item measurement that may be
/// missing for an individual item but present for its neighbours.
pub(crate) struct OptionalFloatListColumn;

impl OptionalFloatListColumn {
    pub(crate) fn data_type() -> DataType {
        DataType::List(Arc::new(Field::new("item", DataType::Float64, true)))
    }

    pub(crate) fn builder() -> ListBuilder<Float64Builder> {
        ListBuilder::new(Float64Builder::new())
    }
}

/// The one list layout storage writes for a bounded collection of counts.
pub(crate) struct UnsignedListColumn;

impl UnsignedListColumn {
    pub(crate) fn data_type() -> DataType {
        DataType::List(Arc::new(Field::new("item", DataType::UInt64, false)))
    }

    pub(crate) fn builder() -> ListBuilder<UInt64Builder> {
        ListBuilder::new(UInt64Builder::new()).with_field(Arc::new(Field::new(
            "item",
            DataType::UInt64,
            false,
        )))
    }
}

/// Append one whole list cell, or a null cell, in one call. `None` and
/// `Some(&[])` are deliberately different: a null cell means the collection was
/// never recorded, an empty cell means it was recorded and held nothing.
pub(crate) trait ListColumnBuilder<T> {
    fn append_optional_list(&mut self, values: Option<&[T]>);
}

impl ListColumnBuilder<Option<f64>> for ListBuilder<Float64Builder> {
    fn append_optional_list(&mut self, values: Option<&[Option<f64>]>) {
        let Some(values) = values else {
            self.append(false);
            return;
        };
        for value in values {
            self.values().append_option(*value);
        }
        self.append(true);
    }
}

impl ListColumnBuilder<u64> for ListBuilder<UInt64Builder> {
    fn append_optional_list(&mut self, values: Option<&[u64]>) {
        let Some(values) = values else {
            self.append(false);
            return;
        };
        for value in values {
            self.values().append_value(*value);
        }
        self.append(true);
    }
}

impl ListColumnBuilder<String> for ListBuilder<StringBuilder> {
    fn append_optional_list(&mut self, values: Option<&[String]>) {
        let Some(values) = values else {
            self.append(false);
            return;
        };
        for value in values {
            self.values().append_value(value);
        }
        self.append(true);
    }
}

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

    pub(crate) fn unsigned(&self, name: &str, row: usize) -> Result<u64, String> {
        self.optional_unsigned(name, row)?
            .ok_or_else(|| self.missing(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, StringArray};
    use arrow_schema::Schema;

    fn batch() -> RecordBatch {
        let mut names = StringListColumn::builder();
        names.append_optional_list(Some(&["left".to_owned(), "right".to_owned()]));
        let schema = Arc::new(Schema::new(vec![
            Field::new("label", DataType::Utf8, true),
            Field::new("count", DataType::UInt64, true),
            Field::new("names", StringListColumn::data_type(), false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![None::<&str>])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(7_u64)])) as ArrayRef,
                Arc::new(names.finish()) as ArrayRef,
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
        assert_eq!(columns.unsigned("count", 0).unwrap(), 7);
    }
}
