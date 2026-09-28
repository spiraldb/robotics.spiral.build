//! Backend-only SQL construction and policy adaptation for SpiralDB.
//!
//! Owned relation names, table prefix qualification, and column names come from
//! this module group's `schema` layer. This module turns those names into
//! auditable statements. Raw user queries delegate to the framework-free core
//! authorization policy before a database driver sees them.

use std::io::Cursor;
use std::sync::Arc;

use arrow_array::{
    Array as _, ArrayRef, FixedSizeListArray, Int32Array, LargeListArray, ListArray, RecordBatch,
    StringArray, StructArray, UInt8Array, UInt32Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Schema};
use base64::Engine as _;
use futures::StreamExt as _;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use serde_json::{Map as JsonMap, Value as JsonValue};
use spiral_robotics_common::relations::{EpisodeQueryRequest, QueryColumn, SpiralQuerySnapshot};

use super::database::{DatabaseSession, RecordBatchStream, SqlStatement};
use super::project_store::{ProjectConnection, ProjectStore};
use super::schema::{Table, column};
use spiral_robotics_common::sql::{ReadOnlyQueryMode, SqlLiteral};

const MAX_QUERY_ROWS: usize = 1_000;
const MAX_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INTERNAL_QUERY_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const MAX_ARROW_ROWS: usize = 10_000;
const MAX_ARROW_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const QUERY_JSON_CHUNK_ROWS: usize = 64;
const QUERY_IMAGE_PREVIEW_MAX_EDGE: u32 = 640;
const QUERY_IMAGE_JPEG_QUALITY: u8 = 82;
const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const VARIABLE_TENSOR_EXTENSION_NAME: &str = "arrow.variable_shape_tensor";

#[derive(Clone, Copy)]
struct QueryLimits {
    rows: usize,
    bytes: usize,
}

impl QueryLimits {
    const fn new(rows: usize, bytes: usize) -> Self {
        Self { rows, bytes }
    }
}

struct QueryExecution {
    statement: SqlStatement,
    limits: QueryLimits,
}

impl QueryExecution {
    fn new(sql: &str, limits: QueryLimits) -> Self {
        Self {
            statement: SqlStatement::from(sql.trim()),
            limits,
        }
    }
    /// A statement that already carries its bound parameters.
    const fn bound(statement: SqlStatement, limits: QueryLimits) -> Self {
        Self { statement, limits }
    }
}

struct QueryRows {
    values: Vec<JsonMap<String, JsonValue>>,
    encoded_bytes: usize,
    max_bytes: usize,
}

impl QueryRows {
    fn with_capacity(row_capacity: usize, max_bytes: usize) -> Self {
        Self {
            values: Vec::with_capacity(row_capacity),
            encoded_bytes: 2,
            max_bytes,
        }
    }

    fn push(&mut self, row: JsonMap<String, JsonValue>) -> Result<bool, String> {
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|error| format!("measuring Spiral row: {error}"))?
            .len()
            .saturating_add(usize::from(!self.values.is_empty()));
        if self.encoded_bytes.saturating_add(row_bytes) > self.max_bytes {
            return Ok(false);
        }
        self.encoded_bytes = self.encoded_bytes.saturating_add(row_bytes);
        self.values.push(row);
        Ok(true)
    }

    fn into_values(self) -> Vec<JsonMap<String, JsonValue>> {
        self.values
    }
}

#[derive(Clone, Copy)]
struct ImageDimensions {
    width: u32,
    height: u32,
}

impl ImageDimensions {
    const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

struct QueryImage<'a> {
    pixels: &'a [u8],
    dimensions: ImageDimensions,
    pixel_format: u8,
}

impl<'a> QueryImage<'a> {
    const fn new(pixels: &'a [u8], dimensions: ImageDimensions, pixel_format: u8) -> Self {
        Self {
            pixels,
            dimensions,
            pixel_format,
        }
    }
}

/// SQL generated for Spiral Robotics-owned storage resources.
pub(crate) struct SpiralSql;

impl SpiralSql {
    pub(crate) fn delete_training_run(relation: &str, run_id: &str) -> String {
        format!(
            "DELETE FROM {relation} WHERE {} = {}",
            column::RUN_ID,
            SqlLiteral::text(run_id)
        )
    }

    /// Every checkpoint's identity and policy configuration, without its weights.
    pub(crate) fn training_checkpoint_catalog() -> String {
        format!(
            "SELECT {run}, {step}, {loss}, {config} FROM {checkpoints} \
         ORDER BY {run}, {step}",
            run = column::RUN_ID,
            step = column::STEP,
            loss = column::VALIDATION_LOSS,
            config = column::CONFIG_JSON,
            checkpoints = Table::TrainingCheckpoints.qualified_name(),
        )
    }

    pub(crate) fn training_checkpoint(run_id: &str, step: u64) -> String {
        let checkpoints = Table::TrainingCheckpoints.qualified_name();
        format!(
            "SELECT {config}, {checkpoint} FROM {checkpoints} \
         WHERE {run_column} = {run_id} AND {step_column} = {step} \
         ORDER BY {created} DESC LIMIT 2",
            config = column::CONFIG_JSON,
            checkpoint = column::CHECKPOINT,
            run_column = column::RUN_ID,
            run_id = SqlLiteral::text(run_id),
            step_column = column::STEP,
            created = column::CREATED_MS,
        )
    }

    fn ensure_read_only(sql: &str) -> Result<(), String> {
        spiral_robotics_common::sql::ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Preview)
            .authorize()
            .map_err(|error| error.to_string())
    }
}

/// Read-only queries over one project store.
pub struct ProjectQueries<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> ProjectQueries<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub async fn query(&self, sql: &str, max_rows: usize) -> Result<SpiralQuerySnapshot, String> {
        SpiralSql::ensure_read_only(sql)?;
        let mut connection = self.project_store.connect().await?;
        QueryExecution::new(
            sql,
            QueryLimits::new(max_rows.clamp(1, MAX_QUERY_ROWS), MAX_QUERY_RESPONSE_BYTES),
        )
        .run_async(&mut connection)
        .await
    }

    /// The same bounded read, scoped to one exact dataset member through a
    /// bound UUID parameter (`$1`).
    pub async fn query_for_episode(
        &self,
        request: &EpisodeQueryRequest,
    ) -> Result<SpiralQuerySnapshot, String> {
        SpiralSql::ensure_read_only(request.statement())?;
        let statement = crate::datasets::EpisodeTopicQuery::for_episode(
            request.episode().dataset(),
            request.episode().episode_id(),
            request.statement().trim().to_owned(),
        )?
        .into_statement();
        let mut connection = self.project_store.connect().await?;
        QueryExecution::bound(
            statement,
            QueryLimits::new(
                request.max_rows().clamp(1, MAX_QUERY_ROWS),
                MAX_QUERY_RESPONSE_BYTES,
            ),
        )
        .run_async(&mut connection)
        .await
    }

    /// The same episode read as one Arrow IPC stream, exact to the value.
    pub async fn arrow_for_episode(
        &self,
        request: &EpisodeQueryRequest,
    ) -> Result<Vec<u8>, String> {
        SpiralSql::ensure_read_only(request.statement())?;
        let statement = crate::datasets::EpisodeTopicQuery::for_episode(
            request.episode().dataset(),
            request.episode().episode_id(),
            request.statement().trim().to_owned(),
        )?
        .into_statement();
        let mut connection = self.project_store.connect().await?;
        connection
            .query(statement)
            .await
            .map_err(|error| format!("executing Spiral query: {error}"))?
            .into_arrow_ipc(request.max_rows().clamp(1, MAX_ARROW_ROWS))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_values_are_escaped_in_one_place() {
        fn assert_single_literal(sql: &str, value: &str) {
            let literal = SqlLiteral::text(value);
            assert!(
                sql.contains(&literal),
                "{sql:?} does not contain {literal:?}"
            );
            assert!(
                !sql.contains(&format!("'{literal}'")),
                "{sql:?} wraps the encoded literal {literal:?} in another pair of quotes"
            );
        }

        let cases = [
            (
                SpiralSql::delete_training_run("robot_training_checkpoints", "run-'one"),
                "run-'one",
            ),
            (SpiralSql::training_checkpoint("run-'two", 42), "run-'two"),
        ];
        for (sql, value) in cases {
            assert_single_literal(&sql, value);
        }
    }
}

impl ProjectConnection {
    pub(crate) async fn query_rows(
        &mut self,
        sql: &str,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, String> {
        QueryExecution::new(
            sql,
            QueryLimits::new(max_rows, MAX_INTERNAL_QUERY_RESPONSE_BYTES),
        )
        .run_async(self)
        .await
    }
}

impl QueryExecution {
    async fn run_async(
        self,
        connection: &mut dyn DatabaseSession,
    ) -> Result<SpiralQuerySnapshot, String> {
        let sql = self.statement.as_str().trim();
        if sql.is_empty() {
            return Err("Spiral query cannot be empty".to_owned());
        }
        if sql.len() > 64 * 1024 {
            return Err("Spiral query is too large".to_owned());
        }
        let mut records = connection
            .query(self.statement)
            .await
            .map_err(|error| format!("executing Spiral query: {error}"))?;
        self.limits.collect_preview(&mut records).await
    }
}

impl RecordBatchStream {
    /// The complete result as one Arrow IPC stream. A result past `max_rows`
    /// rows or the byte limit is refused rather than cut short, so a caller
    /// bounds its own query.
    pub(crate) async fn into_arrow_ipc(mut self, max_rows: usize) -> Result<Vec<u8>, String> {
        let schema = Arc::clone(self.schema());
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(Vec::new(), &schema)
            .map_err(|error| error.to_string())?;
        let mut rows = 0_usize;
        while let Some(batch) = self.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
            rows += batch.num_rows();
            if rows > max_rows {
                return Err(format!(
                    "The result exceeds {max_rows} rows; narrow the query"
                ));
            }
            writer.write(&batch).map_err(|error| error.to_string())?;
            if writer.get_ref().len() > MAX_ARROW_RESPONSE_BYTES {
                return Err(
                    "The result exceeds 32 MiB; select fewer fields or a smaller window".into(),
                );
            }
        }
        writer.finish().map_err(|error| error.to_string())?;
        writer.into_inner().map_err(|error| error.to_string())
    }
}

impl QueryLimits {
    async fn collect_preview(
        self,
        records: &mut RecordBatchStream,
    ) -> Result<SpiralQuerySnapshot, String> {
        let schema = Arc::clone(records.schema());
        let columns = schema
            .fields()
            .iter()
            .map(|field| {
                QueryColumn::new(field.name().clone())
                    .with_semantic_type(field.metadata().get(EXTENSION_TYPE_NAME_KEY).cloned())
            })
            .collect();
        let chunk_rows = if schema.fields().iter().any(|field| {
            matches!(
                field
                    .metadata()
                    .get(EXTENSION_TYPE_NAME_KEY)
                    .map(String::as_str),
                Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME)
            )
        }) {
            1
        } else {
            QUERY_JSON_CHUNK_ROWS
        };
        let fetch = self.rows.saturating_add(1);
        let mut rows = QueryRows::with_capacity(self.rows.min(QUERY_JSON_CHUNK_ROWS), self.bytes);
        let mut inspected = 0_usize;
        let mut truncated = false;
        'result: while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
            let mut offset = 0_usize;
            while offset < batch.num_rows() && inspected < fetch {
                let count = chunk_rows
                    .min(batch.num_rows() - offset)
                    .min(fetch - inspected);
                let chunk = batch.slice(offset, count);
                let preview = chunk.query_preview()?;
                for row in preview.to_json_rows()? {
                    inspected += 1;
                    if rows.values.len() >= self.rows {
                        truncated = true;
                        break 'result;
                    }
                    if !rows.push(row)? {
                        truncated = true;
                        break 'result;
                    }
                }
                offset += count;
            }
        }
        Ok(SpiralQuerySnapshot::new(
            columns,
            rows.into_values(),
            truncated,
        ))
    }
}

/// Replace displayable image extension columns with bounded browser-ready
/// data URLs before Arrow's JSON writer expands every pixel into a number.
/// The response schema is captured from the original batch, so callers still
/// receive the real Arrow physical and semantic types.
trait RecordBatchPresentation {
    fn query_preview(&self) -> Result<RecordBatch, String>;
    fn to_json_rows(&self) -> Result<Vec<JsonMap<String, JsonValue>>, String>;
}

impl RecordBatchPresentation for RecordBatch {
    fn query_preview(&self) -> Result<RecordBatch, String> {
        let batch = self;
        let mut fields = Vec::with_capacity(batch.num_columns());
        let mut columns = Vec::with_capacity(batch.num_columns());
        let mut changed = false;
        for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
            let semantic_type = field
                .metadata()
                .get(EXTENSION_TYPE_NAME_KEY)
                .map(String::as_str);
            let preview = match semantic_type {
                Some(IMAGE_EXTENSION_NAME) => Some(column.as_ref().image_preview()?),
                Some(VARIABLE_TENSOR_EXTENSION_NAME) => column.as_ref().tensor_preview()?,
                _ => None,
            };
            if let Some(preview) = preview {
                fields.push(Field::new(
                    field.name(),
                    DataType::Utf8,
                    field.is_nullable(),
                ));
                columns.push(preview);
                changed = true;
            } else {
                fields.push(field.as_ref().clone());
                columns.push(column.clone());
            }
        }
        if !changed {
            return Ok(batch.clone());
        }
        RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                batch.schema().metadata().clone(),
            )),
            columns,
        )
        .map_err(|error| format!("building Spiral image previews: {error}"))
    }

    fn to_json_rows(&self) -> Result<Vec<JsonMap<String, JsonValue>>, String> {
        let batch = self;
        let mut bytes = Vec::new();
        let mut writer = WriterBuilder::new()
            .with_explicit_nulls(true)
            .build::<_, JsonArray>(&mut bytes);
        writer
            .write_batches(&[batch])
            .map_err(|error| format!("encoding Spiral rows: {error}"))?;
        writer
            .finish()
            .map_err(|error| format!("finishing Spiral rows: {error}"))?;
        drop(writer);
        let values: Vec<JsonValue> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("decoding Spiral rows: {error}"))?;
        values
            .into_iter()
            .map(|value| match value {
                JsonValue::Object(row) => Ok(row),
                _ => Err("Spiral returned a non-object row".to_owned()),
            })
            .collect()
    }
}

trait ArrowArrayPreview {
    fn image_preview(&self) -> Result<ArrayRef, String>;
    fn tensor_preview(&self) -> Result<Option<ArrayRef>, String>;
    fn list_u8(&self, row: usize) -> Option<Vec<u8>>;
}

impl ArrowArrayPreview for dyn arrow_array::Array {
    fn image_preview(&self) -> Result<ArrayRef, String> {
        let array = self;
        let values = array
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| "Spiral Image column did not use struct storage".to_owned())?;
        let data = values
            .column_by_name("data")
            .ok_or_else(|| "Spiral Image column has no data field".to_owned())?;
        let widths = values
            .column_by_name("width")
            .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| "Spiral Image column has no UInt32 width field".to_owned())?;
        let heights = values
            .column_by_name("height")
            .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| "Spiral Image column has no UInt32 height field".to_owned())?;
        let formats = values
            .column_by_name("pixel_format")
            .and_then(|column| column.as_any().downcast_ref::<UInt8Array>())
            .ok_or_else(|| "Spiral Image column has no UInt8 pixel_format field".to_owned())?;
        let mut previews = Vec::with_capacity(values.len());
        for row in 0..values.len() {
            if values.is_null(row) {
                previews.push(None);
                continue;
            }
            if widths.is_null(row) || heights.is_null(row) || formats.is_null(row) {
                return Err("Spiral Image dimensions and pixel format cannot be null".to_owned());
            }
            let pixels = data
                .as_ref()
                .list_u8(row)
                .ok_or_else(|| "Spiral Image data must be a List<UInt8>".to_owned())?;
            previews.push(Some(
                QueryImage::new(
                    &pixels,
                    ImageDimensions::new(widths.value(row), heights.value(row)),
                    formats.value(row),
                )
                .encode()?,
            ));
        }
        Ok(Arc::new(StringArray::from(previews)))
    }

    /// Render only the canonical HWC UInt8 tensor shape used by
    /// `spql.image.to_tensor(image, 'hwc', 'none')`. Other tensors retain their
    /// ordinary JSON representation instead of being mislabeled as imagery.
    fn tensor_preview(&self) -> Result<Option<ArrayRef>, String> {
        let array = self;
        let Some(values) = array.as_any().downcast_ref::<StructArray>() else {
            return Ok(None);
        };
        let Some(data) = values.column_by_name("data") else {
            return Ok(None);
        };
        let Some(shapes) = values
            .column_by_name("shape")
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
        else {
            return Ok(None);
        };
        if shapes.value_length() != 3 {
            return Ok(None);
        }
        let mut previews = Vec::with_capacity(values.len());
        for row in 0..values.len() {
            if values.is_null(row) {
                previews.push(None);
                continue;
            }
            let shape_value = shapes.value(row);
            let Some(shape) = shape_value.as_any().downcast_ref::<Int32Array>() else {
                return Ok(None);
            };
            let [height, width, channels] = shape.values().as_ref() else {
                return Ok(None);
            };
            if *height <= 0 || *width <= 0 || !matches!(*channels, 3 | 4) {
                return Ok(None);
            }
            let Some(pixels) = data.as_ref().list_u8(row) else {
                return Ok(None);
            };
            previews.push(Some(
                QueryImage::new(
                    &pixels,
                    ImageDimensions::new(
                        u32::try_from(*width)
                            .map_err(|_| "Spiral image tensor width overflowed u32")?,
                        u32::try_from(*height)
                            .map_err(|_| "Spiral image tensor height overflowed u32")?,
                    ),
                    if *channels == 3 { 1 } else { 2 },
                )
                .encode()?,
            ));
        }
        Ok(Some(Arc::new(StringArray::from(previews))))
    }

    fn list_u8(&self, row: usize) -> Option<Vec<u8>> {
        let array = self;
        if array.is_null(row) {
            return None;
        }
        let value = if let Some(values) = array.as_any().downcast_ref::<ListArray>() {
            values.value(row)
        } else {
            let values = array.as_any().downcast_ref::<LargeListArray>()?;
            values.value(row)
        };
        let values = value.as_any().downcast_ref::<UInt8Array>()?;
        (values.null_count() == 0).then(|| values.values().to_vec())
    }
}

impl QueryImage<'_> {
    fn encode(self) -> Result<String, String> {
        let ImageDimensions { width, height } = self.dimensions;
        let pixels = self.pixels;
        let pixel_format = self.pixel_format;
        if width == 0 || height == 0 {
            return Err("Spiral Image dimensions must be positive".to_owned());
        }
        let channels = match pixel_format {
            1 => 3_usize,
            2 => 4_usize,
            value => {
                return Err(format!(
                    "Spiral Image uses unsupported pixel format {value}"
                ));
            }
        };
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(channels))
            .ok_or_else(|| "Spiral Image dimensions overflowed the host".to_owned())?;
        if pixels.len() != expected {
            return Err(format!(
                "Spiral Image declares {width}×{height}×{channels} but contains {} values",
                pixels.len()
            ));
        }
        let image = match pixel_format {
            1 => image::RgbImage::from_raw(width, height, pixels.to_vec())
                .map(DynamicImage::ImageRgb8),
            2 => image::RgbaImage::from_raw(width, height, pixels.to_vec())
                .map(DynamicImage::ImageRgba8),
            _ => unreachable!(),
        }
        .ok_or_else(|| "Spiral Image pixels did not match their dimensions".to_owned())?;
        let preview =
            if width > QUERY_IMAGE_PREVIEW_MAX_EDGE || height > QUERY_IMAGE_PREVIEW_MAX_EDGE {
                image.thumbnail(QUERY_IMAGE_PREVIEW_MAX_EDGE, QUERY_IMAGE_PREVIEW_MAX_EDGE)
            } else {
                image
            };
        let (mime_type, encoded) = if pixel_format == 1 {
            let rgb = preview.to_rgb8();
            let mut encoded = Vec::new();
            JpegEncoder::new_with_quality(&mut encoded, QUERY_IMAGE_JPEG_QUALITY)
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|error| format!("encoding Spiral Image preview as JPEG: {error}"))?;
            ("image/jpeg", encoded)
        } else {
            let mut encoded = Cursor::new(Vec::new());
            preview
                .write_to(&mut encoded, ImageFormat::Png)
                .map_err(|error| format!("encoding Spiral Image preview as PNG: {error}"))?;
            ("image/png", encoded.into_inner())
        };
        let payload = base64::engine::general_purpose::STANDARD.encode(encoded);
        Ok(format!("data:{mime_type};base64,{payload}"))
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use arrow_array::builder::{FixedSizeListBuilder, Int32Builder, ListBuilder, UInt8Builder};
    use arrow_schema::Fields;
    use std::collections::HashMap;

    #[test]
    fn decoded_spiral_images_become_bounded_browser_previews() {
        let mut data = ListBuilder::new(UInt8Builder::new());
        for value in [255, 0, 0, 0, 255, 0] {
            data.values().append_value(value);
        }
        data.append(true);
        let data = Arc::new(data.finish()) as ArrayRef;
        let widths = Arc::new(UInt32Array::from(vec![2])) as ArrayRef;
        let heights = Arc::new(UInt32Array::from(vec![1])) as ArrayRef;
        let formats = Arc::new(UInt8Array::from(vec![1])) as ArrayRef;
        let storage_fields = Fields::from(vec![
            Arc::new(Field::new("data", data.data_type().clone(), false)),
            Arc::new(Field::new("width", DataType::UInt32, false)),
            Arc::new(Field::new("height", DataType::UInt32, false)),
            Arc::new(Field::new("pixel_format", DataType::UInt8, false)),
        ]);
        let images = Arc::new(StructArray::new(
            storage_fields,
            vec![data, widths, heights, formats],
            None,
        )) as ArrayRef;
        let image_field =
            Field::new("image", images.data_type().clone(), false).with_metadata(HashMap::from([
                (
                    EXTENSION_TYPE_NAME_KEY.to_owned(),
                    IMAGE_EXTENSION_NAME.to_owned(),
                ),
            ]));
        let batch =
            RecordBatch::try_new(Arc::new(Schema::new(vec![image_field])), vec![images]).unwrap();

        let preview = batch.query_preview().unwrap();
        let rows = preview.to_json_rows().unwrap();
        let value = rows[0].get("image").and_then(JsonValue::as_str).unwrap();
        assert!(value.starts_with("data:image/jpeg;base64,"));
        let encoded = value.split_once(',').unwrap().1;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 2);
        assert!(value.len() < 2_048, "tiny preview unexpectedly expanded");
    }

    #[test]
    fn hwc_u8_arrow_tensors_use_the_same_image_preview_path() {
        let mut data = ListBuilder::new(UInt8Builder::new());
        for value in [255, 0, 0, 0, 255, 0] {
            data.values().append_value(value);
        }
        data.append(true);
        let data = Arc::new(data.finish()) as ArrayRef;
        let mut shapes = FixedSizeListBuilder::new(Int32Builder::new(), 3);
        for value in [1, 2, 3] {
            shapes.values().append_value(value);
        }
        shapes.append(true);
        let shapes = Arc::new(shapes.finish()) as ArrayRef;
        let storage_fields = Fields::from(vec![
            Arc::new(Field::new("data", data.data_type().clone(), false)),
            Arc::new(Field::new("shape", shapes.data_type().clone(), false)),
        ]);
        let tensors =
            Arc::new(StructArray::new(storage_fields, vec![data, shapes], None)) as ArrayRef;
        let tensor_field =
            Field::new("image", tensors.data_type().clone(), false).with_metadata(HashMap::from([
                (
                    EXTENSION_TYPE_NAME_KEY.to_owned(),
                    VARIABLE_TENSOR_EXTENSION_NAME.to_owned(),
                ),
            ]));
        let batch =
            RecordBatch::try_new(Arc::new(Schema::new(vec![tensor_field])), vec![tensors]).unwrap();

        let rows = batch.query_preview().unwrap().to_json_rows().unwrap();
        assert!(
            rows[0]
                .get("image")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| value.starts_with("data:image/jpeg;base64,"))
        );
    }
}
