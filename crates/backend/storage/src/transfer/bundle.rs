//! Format 4 project ZIP: a manifest, a project-scoped SQLite snapshot, and
//! one Arrow IPC stream per Spiral data relation.
use crate::datasets::{McapDatasetDefinition, McapSources};
use crate::db::{self as spiral, ProjectConnection, Table};
use crate::studio::StudioSnapshot;
use arrow_array::RecordBatch;
use arrow_ipc::{reader::StreamReader, writer::StreamWriter};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::ProjectImportSummary;
use std::collections::BTreeSet;
use std::io::{self, Cursor, Read, Write};

const PROJECT_FORMAT: &str = "spiral-robotics-project";
/// Format 5 dropped the training checkpoints, which now live on Modal.
const FORMAT_VERSION: u32 = 5;
const MANIFEST_ENTRY: &str = "manifest.json";
const STUDIO_ENTRY: &str = "studio.sqlite3";
const MAX_ARROW_MESSAGE_METADATA_BYTES: usize = 1024 * 1024;
const MAX_ARROW_DECODED_BYTES: usize = ProjectBundle::MAX_BYTES;
const MAX_ARROW_RECORD_BATCH_ROWS: usize = 1_000_000;
const MAX_ARROW_DECODED_ROWS: usize = 1_000_000;
const MAX_ARROW_SCHEMA_FIELDS: usize = 256;

pub struct ProjectBundle;
impl ProjectBundle {
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;
}

pub(super) struct ProjectBundleExport<'a> {
    studio: &'a crate::StudioProject,
    datasets: Vec<String>,
}
impl<'a> ProjectBundleExport<'a> {
    pub(super) fn new(studio: &'a crate::StudioProject, datasets: Vec<String>) -> Self {
        Self { studio, datasets }
    }
}

pub(super) struct ProjectBundleImport<'a> {
    studio: &'a crate::StudioProject,
    archive: DecodedBundle,
    current_datasets: Vec<String>,
}
impl<'a> ProjectBundleImport<'a> {
    pub(super) fn new(studio: &'a crate::StudioProject, bytes: &[u8]) -> Result<Self, String> {
        Ok(Self {
            studio,
            archive: ProjectBundle::decode(bytes)?,
            current_datasets: Vec::new(),
        })
    }
    pub(super) fn dataset_names(&self) -> &[String] {
        &self.archive.datasets
    }
    pub(super) fn with_current_datasets(mut self, names: Vec<String>) -> Self {
        self.current_datasets = names;
        self
    }
}

struct DecodedBundle {
    studio: Vec<u8>,
    tables: Vec<(Table, Vec<RecordBatch>)>,
    datasets: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    format_version: u32,
    schema_version: u32,
    mcap_datasets: Vec<String>,
}

impl Manifest {
    fn current(mcap_datasets: Vec<String>) -> Self {
        Self {
            format: PROJECT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            mcap_datasets,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.format != PROJECT_FORMAT || self.format_version != FORMAT_VERSION {
            return Err(format!(
                "Project bundle format {} is not supported; this Studio imports format {FORMAT_VERSION} only",
                self.format_version
            ));
        }
        if self.schema_version != spiral::SCHEMA_VERSION {
            return Err(format!(
                "project schema {} cannot be imported by schema {}",
                self.schema_version,
                spiral::SCHEMA_VERSION
            ));
        }
        let datasets = self.mcap_datasets.iter().collect::<BTreeSet<_>>();
        if datasets.len() != self.mcap_datasets.len()
            || datasets
                .iter()
                .any(|name| !spiral::StorageSchema::is_dataset_id(name) || name.contains('.'))
        {
            return Err("Project manifest contains invalid or duplicate MCAP datasets".into());
        }
        Ok(())
    }
}

impl ProjectBundle {
    pub(super) async fn export(
        &self,
        connection: &mut ProjectConnection,
        source: ProjectBundleExport<'_>,
    ) -> Result<Vec<u8>, String> {
        let snapshot = StudioSnapshot::capture(source.studio).await?;
        let mut bundle = BundleWriter::new(
            &Manifest::current(source.datasets.clone()),
            &snapshot.bytes()?,
        )?;
        for table in Table::ALL {
            bundle.start(&Self::relation_entry(table))?;
            match table {
                Table::DatasetSources => {
                    Self::export_sources(connection, &source.datasets, &mut bundle).await?;
                }
            }
        }
        bundle.finish()
    }

    /// Every dataset's files in one stream, each row naming its dataset.
    async fn export_sources(
        connection: &mut ProjectConnection,
        datasets: &[String],
        bundle: &mut BundleWriter,
    ) -> Result<(), String> {
        let mut writer = StreamWriter::try_new(bundle, &Table::DatasetSources.schema())
            .map_err(|error| error.to_string())?;
        for name in datasets {
            let mut records = connection
                .execute_query(McapDatasetDefinition::new(name).bundle_sql())
                .await?;
            while let Some(batch) = records.next().await {
                let batch = batch.map_err(|error| error.to_string())?;
                writer
                    .write(&McapSources::bundled(name, &batch)?)
                    .map_err(|error| error.to_string())?;
            }
        }
        writer.finish().map_err(|error| error.to_string())
    }

    fn relation_entry(table: Table) -> String {
        format!("{}.arrows", table.name())
    }

    fn decode(bytes: &[u8]) -> Result<DecodedBundle, String> {
        let mut bundle = BundleReader::new(bytes)?;
        let manifest: Manifest = serde_json::from_slice(&bundle.entry(MANIFEST_ENTRY)?)
            .map_err(|error| format!("project manifest is invalid: {error}"))?;
        manifest.validate()?;
        if bundle.zip.len() != 2 + Table::ALL.len() {
            return Err(format!(
                "Project ZIP must contain exactly {MANIFEST_ENTRY}, {STUDIO_ENTRY}, and one Arrow stream per relation"
            ));
        }
        let studio = bundle.entry(STUDIO_ENTRY)?;
        let mut budget = ArrowDecodeBudget::default();
        let mut tables = Vec::new();
        for table in Table::ALL {
            let payload = bundle.entry(&Self::relation_entry(table))?;
            let batches = budget.decode(ArrowPayload::new(table.name(), payload))?;
            table.validate(batches[0].schema().as_ref(), "project import")?;
            tables.push((table, batches));
        }
        Ok(DecodedBundle {
            studio,
            tables,
            datasets: manifest.mcap_datasets,
        })
    }

    pub(super) async fn import(
        &self,
        connection: &mut ProjectConnection,
        source: ProjectBundleImport<'_>,
    ) -> Result<ProjectImportSummary, String> {
        let ProjectBundleImport {
            studio,
            archive,
            current_datasets,
        } = source;
        let snapshot = StudioSnapshot::open(&archive.studio, studio).await?;
        let local_transaction = snapshot.stage(studio).await?;
        let summary = connection
            .transaction("project import", async move |connection| {
                McapSources::retire(connection, &current_datasets).await?;
                let mut summary = ProjectImportSummary::default();
                for (table, batches) in archive.tables {
                    summary.record_relation(
                        batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64,
                    );
                    match table {
                        Table::DatasetSources => {
                            McapSources::restore(connection, &batches, &archive.datasets).await?;
                        }
                    }
                }
                Ok(summary)
            })
            .await?;
        local_transaction.commit().await.map_err(|error| format!("Spiral data was restored, but committing Studio records failed: {error}. Retry the same project import before continuing."))?;
        Ok(summary)
    }
}

/// Writes project ZIP entries, refusing contents past the bundle size limit.
struct BundleWriter {
    zip: zip::ZipWriter<Cursor<Vec<u8>>>,
    remaining: usize,
}

impl BundleWriter {
    fn new(manifest: &Manifest, studio: &[u8]) -> Result<Self, String> {
        let mut bundle = Self {
            zip: zip::ZipWriter::new(Cursor::new(Vec::new())),
            remaining: ProjectBundle::MAX_BYTES,
        };
        let manifest = serde_json::to_vec(manifest)
            .map_err(|error| format!("encoding project manifest: {error}"))?;
        for (name, bytes) in [
            (MANIFEST_ENTRY, manifest.as_slice()),
            (STUDIO_ENTRY, studio),
        ] {
            bundle.start(name)?;
            bundle.write_all(bytes).map_err(|error| error.to_string())?;
        }
        Ok(bundle)
    }

    fn start(&mut self, name: &str) -> Result<(), String> {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        self.zip
            .start_file(name, options)
            .map_err(|error| error.to_string())
    }

    fn finish(self) -> Result<Vec<u8>, String> {
        let bytes = self
            .zip
            .finish()
            .map_err(|error| error.to_string())?
            .into_inner();
        if bytes.len() > ProjectBundle::MAX_BYTES {
            return Err("Project exceeds the bundle size limit".into());
        }
        Ok(bytes)
    }
}

impl io::Write for BundleWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self.remaining.checked_sub(bytes.len()).ok_or_else(|| {
            io::Error::other(format!(
                "project export exceeds the {} MiB portable-file limit",
                ProjectBundle::MAX_BYTES / (1024 * 1024),
            ))
        })?;
        self.zip.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.zip.flush()
    }
}

/// Reads project ZIP entries by name within the uncompressed size limit.
struct BundleReader<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    remaining: usize,
}

impl<'a> BundleReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() > ProjectBundle::MAX_BYTES {
            return Err("Project exceeds the bundle size limit".into());
        }
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Not a project ZIP")?;
        if zip.index_for_name(MANIFEST_ENTRY).is_none() {
            return Err(format!(
                "Not a format {FORMAT_VERSION} project; project files from older Studio versions cannot be imported"
            ));
        }
        Ok(Self {
            zip,
            remaining: ProjectBundle::MAX_BYTES,
        })
    }

    fn entry(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let file = self
            .zip
            .by_name(name)
            .map_err(|_| format!("Project ZIP is missing {name}"))?;
        if file.size() > self.remaining as u64 {
            return Err("Project exceeds the uncompressed bundle size limit".into());
        }
        let mut payload = Vec::new();
        file.take(self.remaining as u64 + 1)
            .read_to_end(&mut payload)
            .map_err(|error| error.to_string())?;
        self.remaining = self
            .remaining
            .checked_sub(payload.len())
            .ok_or("Project exceeds the uncompressed bundle size limit")?;
        Ok(payload)
    }
}

#[derive(Clone, Copy)]
struct ArrowDecodeLimits {
    decoded_bytes: usize,
    decoded_rows: usize,
    relation_rows: usize,
    record_batch_rows: usize,
    schema_fields: usize,
    message_metadata_bytes: usize,
}

const ARROW_DECODE_LIMITS: ArrowDecodeLimits = ArrowDecodeLimits {
    decoded_bytes: MAX_ARROW_DECODED_BYTES,
    decoded_rows: MAX_ARROW_DECODED_ROWS,
    relation_rows: MAX_ARROW_DECODED_ROWS,
    record_batch_rows: MAX_ARROW_RECORD_BATCH_ROWS,
    schema_fields: MAX_ARROW_SCHEMA_FIELDS,
    message_metadata_bytes: MAX_ARROW_MESSAGE_METADATA_BYTES,
};

struct ArrowPayload<'a> {
    relation: &'a str,
    bytes: Vec<u8>,
}

struct ArrowPreflight<'a> {
    relation: &'a str,
    body: &'a [u8],
    limits: ArrowDecodeLimits,
}

impl<'a> ArrowPreflight<'a> {
    const fn new(relation: &'a str, limits: ArrowDecodeLimits) -> Self {
        Self {
            relation,
            body: &[],
            limits,
        }
    }

    const fn with_body(mut self, body: &'a [u8]) -> Self {
        self.body = body;
        self
    }
}

#[derive(Default)]
struct ArrowPreflightCounters {
    claimed_decoded_bytes: usize,
    rows: usize,
}

impl<'a> ArrowPayload<'a> {
    fn new(relation: &'a str, bytes: Vec<u8>) -> Self {
        Self { relation, bytes }
    }
}

#[derive(Default)]
struct ArrowDecodeBudget {
    claimed_decoded_bytes: usize,
    rows: usize,
}

impl ArrowDecodeBudget {
    fn decode(&mut self, payload: ArrowPayload<'_>) -> Result<Vec<RecordBatch>, String> {
        self.decode_with_limits(payload, ARROW_DECODE_LIMITS)
    }

    fn decode_with_limits(
        &mut self,
        payload: ArrowPayload<'_>,
        limits: ArrowDecodeLimits,
    ) -> Result<Vec<RecordBatch>, String> {
        self.preflight(&payload, limits)?;
        let ArrowPayload { relation, bytes } = payload;
        // The preflight above bounds every IPC buffer by its declared decoded
        // length before Arrow can allocate or decompress it. Do not use
        // `RecordBatch::get_array_memory_size` for this bound: the stream reader
        // exposes column buffers as slices of one shared message-body allocation,
        // and that estimate counts the shared capacity once for every slice.
        let reader = StreamReader::try_new(Cursor::new(bytes), None)
            .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
        let schema = reader.schema();
        let mut batches = Vec::new();
        for batch in reader {
            let batch =
                batch.map_err(|error| format!("reading Arrow stream for {relation}: {error}"))?;
            batches.push(batch);
        }
        if batches.is_empty() {
            batches.push(RecordBatch::new_empty(schema));
        }
        if batches
            .iter()
            .any(|batch| batch.schema() != batches[0].schema())
        {
            return Err(format!(
                "project relation {relation} changes schema between Arrow batches"
            ));
        }
        Ok(batches)
    }

    fn preflight(
        &mut self,
        payload: &ArrowPayload<'_>,
        limits: ArrowDecodeLimits,
    ) -> Result<(), String> {
        let relation = payload.relation;
        let payload = payload.bytes.as_slice();
        let mut position = 0_usize;
        let mut counters = ArrowPreflightCounters::default();
        while position < payload.len() {
            let first = Self::read_u32(payload, &mut position, relation)?;
            let metadata_len = if first == u32::MAX {
                Self::read_u32(payload, &mut position, relation)?
            } else {
                first
            };
            if metadata_len == 0 {
                if position != payload.len() {
                    return Err(format!(
                        "project Arrow stream for {relation} has bytes after its end marker"
                    ));
                }
                break;
            }
            let metadata_len = usize::try_from(metadata_len)
                .expect("Arrow metadata length always fits usize on supported targets");
            if metadata_len > limits.message_metadata_bytes {
                return Err(format!(
                    "project Arrow metadata for {relation} exceeds the {} MiB message limit",
                    limits.message_metadata_bytes / (1024 * 1024),
                ));
            }
            counters.claimed_decoded_bytes = counters
            .claimed_decoded_bytes
            .checked_add(metadata_len)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
            .ok_or_else(|| {
                format!(
                    "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            let metadata_end = position
                .checked_add(metadata_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("project Arrow metadata for {relation} is truncated"))?;
            let message = arrow_ipc::root_as_message(&payload[position..metadata_end])
                .map_err(|error| format!("opening Arrow metadata for {relation}: {error}"))?;
            position = metadata_end;

            let body_len = usize::try_from(message.bodyLength()).map_err(|_| {
                format!("project Arrow message for {relation} has an invalid body length")
            })?;
            if body_len > limits.decoded_bytes {
                return Err(format!(
                    "project Arrow message body for {relation} exceeds the {} MiB decoded-memory limit",
                    limits.decoded_bytes / (1024 * 1024),
                ));
            }
            let body_end = position
                .checked_add(body_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("project Arrow message body for {relation} is truncated"))?;
            let body = &payload[position..body_end];

            match message.header_type() {
                arrow_ipc::MessageHeader::Schema => {
                    let schema = message.header_as_schema().ok_or_else(|| {
                        format!("project Arrow schema metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, limits).validate_schema(schema)?;
                }
                arrow_ipc::MessageHeader::RecordBatch => {
                    let batch = message.header_as_record_batch().ok_or_else(|| {
                        format!("project Arrow record metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                arrow_ipc::MessageHeader::DictionaryBatch => {
                    let dictionary = message.header_as_dictionary_batch().ok_or_else(|| {
                        format!("project Arrow dictionary metadata for {relation} is invalid")
                    })?;
                    let batch = dictionary.data().ok_or_else(|| {
                        format!("project Arrow dictionary for {relation} has no record metadata")
                    })?;
                    ArrowPreflight::new(relation, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                _ => {}
            }
            position = body_end;
        }

        if counters.rows > limits.relation_rows {
            return Err(format!(
                "project Arrow relation {relation} exceeds the {}-row relation limit",
                limits.relation_rows,
            ));
        }
        self.claimed_decoded_bytes = self
        .claimed_decoded_bytes
        .checked_add(counters.claimed_decoded_bytes)
        .filter(|bytes| *bytes <= limits.decoded_bytes)
        .ok_or_else(|| {
            format!(
                "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                limits.decoded_bytes / (1024 * 1024),
            )
        })?;
        self.rows = self
            .rows
            .checked_add(counters.rows)
            .filter(|rows| *rows <= limits.decoded_rows)
            .ok_or_else(|| {
                format!(
                    "project Arrow data exceeds the {}-row decoded limit at relation {relation}",
                    limits.decoded_rows,
                )
            })?;
        Ok(())
    }
    fn read_u32(payload: &[u8], position: &mut usize, relation: &str) -> Result<u32, String> {
        let end = position
            .checked_add(4)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| format!("Arrow stream for {relation} has a truncated message prefix"))?;
        let value = u32::from_le_bytes(
            payload[*position..end]
                .try_into()
                .expect("Arrow prefix has exactly four bytes"),
        );
        *position = end;
        Ok(value)
    }
}

impl ArrowPreflight<'_> {
    fn validate_schema(&self, schema: arrow_ipc::Schema<'_>) -> Result<(), String> {
        let relation = self.relation;
        let max_fields = self.limits.schema_fields;
        let Some(fields) = schema.fields() else {
            return Ok(());
        };
        if fields.len() > max_fields {
            return Err(format!(
                "project Arrow schema for {relation} exceeds the {max_fields}-column limit"
            ));
        }
        let mut pending = fields.iter().collect::<Vec<_>>();
        let mut total = 0_usize;
        while let Some(field) = pending.pop() {
            total = total
                .checked_add(1)
                .filter(|count| *count <= max_fields)
                .ok_or_else(|| {
                    format!(
                        "project Arrow schema for {relation} exceeds the {max_fields}-field limit"
                    )
                })?;
            if let Some(children) = field.children() {
                pending.extend(children.iter());
            }
        }
        Ok(())
    }

    fn validate_record_batch(
        &self,
        batch: arrow_ipc::RecordBatch<'_>,
        counters: &mut ArrowPreflightCounters,
    ) -> Result<(), String> {
        let relation = self.relation;
        let body = self.body;
        let limits = self.limits;
        let batch_rows = usize::try_from(batch.length()).map_err(|_| {
            format!("project Arrow record batch for {relation} has an invalid row count")
        })?;
        if batch_rows > limits.record_batch_rows {
            return Err(format!(
                "project Arrow record batch for {relation} exceeds the {}-row batch limit",
                limits.record_batch_rows,
            ));
        }
        counters.rows = counters.rows.checked_add(batch_rows).ok_or_else(|| {
            format!("project Arrow row count overflows while reading relation {relation}")
        })?;

        let compressed = batch.compression().is_some();
        for buffer in batch.buffers().into_iter().flatten() {
            let offset = usize::try_from(buffer.offset()).map_err(|_| {
                format!("project Arrow buffer for {relation} has an invalid offset")
            })?;
            let length = usize::try_from(buffer.length()).map_err(|_| {
                format!("project Arrow buffer for {relation} has an invalid length")
            })?;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= body.len())
                .ok_or_else(|| {
                    format!("project Arrow buffer for {relation} exceeds its message body")
                })?;
            let decoded_length = if length == 0 || !compressed {
                length
            } else {
                if length < 8 {
                    return Err(format!(
                        "project compressed Arrow buffer for {relation} has no length prefix"
                    ));
                }
                let declared = i64::from_le_bytes(
                    body[offset..offset + 8]
                        .try_into()
                        .expect("compressed Arrow prefix has exactly eight bytes"),
                );
                match declared {
                -1 => length - 8,
                0.. => usize::try_from(declared).map_err(|_| {
                    format!(
                        "project compressed Arrow buffer for {relation} is too large for this machine"
                    )
                })?,
                _ => {
                    return Err(format!(
                        "project compressed Arrow buffer for {relation} has an invalid decoded length"
                    ));
                }
            }
            };
            counters.claimed_decoded_bytes = counters
            .claimed_decoded_bytes
            .checked_add(decoded_length)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
                .ok_or_else(|| {
                    format!(
                        "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            debug_assert!(end <= body.len());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
