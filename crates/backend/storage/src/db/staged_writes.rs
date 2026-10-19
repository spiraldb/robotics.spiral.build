//! What one transaction writes, held until it commits: each table's
//! ingestion and drops folded into the one write the table gets, since
//! Spiral publishes one change per table in a transaction.

use std::collections::BTreeMap;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use futures::TryStreamExt as _;

use super::database::{BatchIngestion, DatabaseResult, IngestMode, RelationName};

/// Rows one write carries, of one schema.
pub(super) struct StagedRows {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
}

impl StagedRows {
    pub(super) fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Self {
        Self { schema, batches }
    }

    pub(super) fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    pub(super) fn is_empty(&self) -> bool {
        self.batches.iter().all(|batch| batch.num_rows() == 0)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn batches(&self) -> &[RecordBatch] {
        &self.batches
    }

    pub(super) fn into_parts(self) -> (SchemaRef, Vec<RecordBatch>) {
        (self.schema, self.batches)
    }

    /// These rows, then `rows`.
    pub(super) fn extended(mut self, rows: Self) -> Self {
        self.batches.extend(rows.batches);
        self
    }
}

/// The one write a table gets when its transaction commits.
pub(super) enum StagedWrite {
    /// Rows added to the table, which is made when missing.
    Append(StagedRows),
    /// The table's rows replaced, or the table made with them.
    Replace(StagedRows),
    /// The table removed, if it exists.
    Drop,
}

/// Every table's write in one transaction.
#[derive(Default)]
pub(super) struct StagedWrites {
    writes: BTreeMap<RelationName, StagedWrite>,
}

impl StagedWrites {
    /// Hold every batch `ingestion` streams.
    pub(super) async fn stage(&mut self, ingestion: BatchIngestion) -> DatabaseResult<()> {
        let (relation, mode, records) = ingestion.into_parts();
        let schema = records.schema().clone();
        let batches = records.try_collect::<Vec<_>>().await?;
        self.ingest(relation, mode, StagedRows::new(schema, batches));
        Ok(())
    }

    /// Hold `rows` for `relation`, after whatever the transaction holds for
    /// it already.
    pub(super) fn ingest(&mut self, relation: RelationName, mode: IngestMode, rows: StagedRows) {
        let write = match (self.writes.remove(&relation), mode) {
            (Some(StagedWrite::Append(held)), IngestMode::CreateOrAppend) => {
                StagedWrite::Append(held.extended(rows))
            }
            (Some(StagedWrite::Replace(held)), IngestMode::CreateOrAppend) => {
                StagedWrite::Replace(held.extended(rows))
            }
            // Rows written after a drop make the table again.
            (Some(StagedWrite::Drop), _) | (_, IngestMode::ReplaceOrCreate) => {
                StagedWrite::Replace(rows)
            }
            (None, IngestMode::CreateOrAppend) => StagedWrite::Append(rows),
        };
        self.writes.insert(relation, write);
    }

    /// Remove `relation`, dropping whatever the transaction held for it.
    pub(super) fn drop_table(&mut self, relation: RelationName) {
        self.writes.insert(relation, StagedWrite::Drop);
    }

    /// The schema the transaction holds for `relation`: `Some(None)` once it
    /// drops it, `None` when it holds nothing for it.
    pub(super) fn schema(&self, relation: &RelationName) -> Option<Option<SchemaRef>> {
        self.writes.get(relation).map(|write| match write {
            StagedWrite::Append(rows) | StagedWrite::Replace(rows) => Some(rows.schema().clone()),
            StagedWrite::Drop => None,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    pub(super) fn into_writes(self) -> impl Iterator<Item = (RelationName, StagedWrite)> {
        self.writes.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Int64Array;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn rows(values: &[i64]) -> StagedRows {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(values.to_vec()))],
        )
        .unwrap();
        StagedRows::new(schema, vec![batch])
    }

    fn held(writes: StagedWrites) -> Vec<(String, &'static str, usize)> {
        writes
            .into_writes()
            .map(|(relation, write)| {
                let (kind, rows) = match write {
                    StagedWrite::Append(rows) => ("append", rows),
                    StagedWrite::Replace(rows) => ("replace", rows),
                    StagedWrite::Drop => return (relation.as_str().to_owned(), "drop", 0),
                };
                let count = rows.into_parts().1.iter().map(RecordBatch::num_rows).sum();
                (relation.as_str().to_owned(), kind, count)
            })
            .collect()
    }

    #[test]
    fn each_table_gets_one_write_folded_from_everything_written_to_it() {
        let name = |name: &str| RelationName::parse(name).unwrap();
        let mut writes = StagedWrites::default();
        writes.ingest(name("a"), IngestMode::CreateOrAppend, rows(&[1, 2]));
        writes.ingest(name("a"), IngestMode::CreateOrAppend, rows(&[3]));
        writes.ingest(name("b"), IngestMode::ReplaceOrCreate, rows(&[1]));
        writes.ingest(name("b"), IngestMode::CreateOrAppend, rows(&[2]));
        writes.drop_table(name("c"));
        writes.ingest(name("c"), IngestMode::CreateOrAppend, rows(&[1]));
        writes.ingest(name("d"), IngestMode::CreateOrAppend, rows(&[1]));
        writes.drop_table(name("d"));
        assert_eq!(writes.schema(&name("d")), Some(None));
        assert!(writes.schema(&name("a")).unwrap().is_some());
        assert_eq!(writes.schema(&name("e")), None);
        assert_eq!(
            held(writes),
            [
                ("a".to_owned(), "append", 3),
                ("b".to_owned(), "replace", 2),
                ("c".to_owned(), "replace", 1),
                ("d".to_owned(), "drop", 0),
            ]
        );
    }
}
