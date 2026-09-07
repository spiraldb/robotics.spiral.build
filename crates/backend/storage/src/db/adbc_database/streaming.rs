use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::Arc;

use arrow_array::{RecordBatch, RecordBatchOptions, RecordBatchReader, make_array};
use arrow_buffer::{BooleanBuffer, Buffer, NullBuffer};
use arrow_data::ArrayData;
use arrow_schema::{ArrowError, SchemaRef};
use futures::StreamExt as _;

use super::RecordBatchStream;
use super::driver::LoadedDatabase;

/// ADBC pulls input synchronously, on the blocking worker or a driver-owned
/// thread. Polling through the captured Tokio handle keeps async producers live
/// without an eager collection, an extra pump task, or an unbounded queue.
pub(super) struct IngestionReader {
    batches: RecordBatchStream,
    runtime: tokio::runtime::Handle,
}

impl IngestionReader {
    pub(super) fn new(batches: RecordBatchStream) -> Self {
        Self {
            batches,
            runtime: tokio::runtime::Handle::current(),
        }
    }
}

impl Iterator for IngestionReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.runtime
            .block_on(self.batches.next())
            .map(|batch| batch.map_err(|error| ArrowError::ExternalError(Box::new(error))))
    }
}

impl RecordBatchReader for IngestionReader {
    fn schema(&self) -> SchemaRef {
        self.batches.schema().clone()
    }
}

/// Arrow's C release callbacks live in the driver library. Retain the library
/// on each exported buffer, so sliced/cloned columns can outlive the database.
/// The original allocation must be released before its library owner.
struct DriverBuffer {
    _buffer: Buffer,
    _database: AssertUnwindSafe<Arc<LoadedDatabase>>,
}

pub(super) struct DriverBuffers;

impl DriverBuffers {
    pub(super) fn retain(batch: RecordBatch, database: Arc<LoadedDatabase>) -> RecordBatch {
        let (schema, columns, row_count) = batch.into_parts();
        let columns = columns
            .into_iter()
            .map(|column| make_array(Self::array(column.to_data(), &database)))
            .collect();
        RecordBatch::try_new_with_options(
            schema,
            columns,
            &RecordBatchOptions::new().with_row_count(Some(row_count)),
        )
        .expect("retaining allocation owners preserves batch layout")
    }

    fn array(data: ArrayData, database: &Arc<LoadedDatabase>) -> ArrayData {
        let buffers = data
            .buffers()
            .iter()
            .map(|buffer| Self::buffer(buffer, database))
            .collect();
        let children = data
            .child_data()
            .iter()
            .cloned()
            .map(|child| Self::array(child, database))
            .collect();
        let nulls = data.nulls().map(|nulls| {
            // SAFETY: the bitmap and its offset/length are unchanged, so its
            // existing null count remains valid without rescanning the bits.
            unsafe {
                NullBuffer::new_unchecked(
                    BooleanBuffer::new(
                        Self::buffer(nulls.buffer(), database),
                        nulls.offset(),
                        nulls.len(),
                    ),
                    nulls.null_count(),
                )
            }
        });
        // SAFETY: types, lengths, offsets, contents and child layouts are
        // unchanged; only allocation ownership is extended. Avoid rescanning
        // already validated buffers in the streaming hot path.
        unsafe {
            data.into_builder()
                .buffers(buffers)
                .child_data(children)
                .nulls(nulls)
                .build_unchecked()
        }
    }

    fn buffer(buffer: &Buffer, database: &Arc<LoadedDatabase>) -> Buffer {
        let address = NonNull::new(buffer.as_ptr().cast_mut()).expect("Arrow buffers are non-null");
        let owner = Arc::new(DriverBuffer {
            _buffer: buffer.clone(),
            _database: AssertUnwindSafe(database.clone()),
        });
        // SAFETY: owner retains this exact allocation and its release callback's
        // library; length and pointer describe the existing immutable slice.
        unsafe { Buffer::from_custom_allocation(address, buffer.len(), owner) }
    }
}
