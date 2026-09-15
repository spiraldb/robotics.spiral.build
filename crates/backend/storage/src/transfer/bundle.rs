//! Version 3 ZIP: a project-scoped SQLite snapshot and a Spiral data archive.
use crate::datasets::McapSources;
use crate::db::{self as spiral, ProjectConnection, StorageClock, Table};
use crate::studio::StudioSnapshot;
use arrow_array::RecordBatch;
use arrow_ipc::{reader::StreamReader, writer::StreamWriter};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::ProjectImportSummary;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read, Write};

const PROJECT_MAGIC: &[u8; 8] = b"SPRDATA3";
const PROJECT_FORMAT: &str = "spiral-robotics-project";
const FORMAT_VERSION: u32 = 3;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 256;
const MAX_ARROW_MESSAGE_METADATA_BYTES: usize = MAX_MANIFEST_BYTES;
const MAX_ARROW_DECODED_BYTES: usize = ProjectBundle::MAX_BYTES;
const MAX_ARROW_RECORD_BATCH_ROWS: usize = 1_000_000;
const MAX_ARROW_DECODED_ROWS: usize = 1_000_000;
const MAX_ARROW_SCHEMA_FIELDS: usize = 256;

pub struct ProjectBundle;
impl ProjectBundle {
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;
}

pub(super) struct ProjectBundleExport<'a> {
    table_prefix: &'a str,
    studio: &'a crate::StudioProject,
    datasets: Vec<String>,
}
impl<'a> ProjectBundleExport<'a> {
    pub(super) fn new(
        table_prefix: &'a str,
        studio: &'a crate::StudioProject,
        datasets: Vec<String>,
    ) -> Self {
        Self {
            table_prefix,
            studio,
            datasets,
        }
    }
}

pub(super) struct ProjectBundleImport<'a> {
    table_prefix: &'a str,
    studio: &'a crate::StudioProject,
    archive: DecodedBundle,
    current_datasets: Vec<String>,
}
impl<'a> ProjectBundleImport<'a> {
    pub(super) fn new(
        table_prefix: &'a str,
        studio: &'a crate::StudioProject,
        bytes: &[u8],
    ) -> Result<Self, String> {
        Ok(Self {
            table_prefix,
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

struct ParsedBundle {
    entries: Vec<(String, Vec<u8>)>,
    datasets: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    format_version: u32,
    schema_version: u32,
    exported_ms: i64,
    relations: Vec<String>,
    mcap_datasets: Vec<String>,
}

impl ProjectBundle {
    pub(super) async fn export(
        &self,
        connection: &mut ProjectConnection,
        source: ProjectBundleExport<'_>,
    ) -> Result<Vec<u8>, String> {
        let snapshot = StudioSnapshot::capture(source.studio).await?;
        let studio = snapshot.bytes()?;
        let mut entries = Vec::new();
        for table in Table::ALL {
            let relation = table.qualified_name(source.table_prefix);
            let mut payload = LimitedPayload::new(
                Self::MAX_BYTES
                    .saturating_sub(studio.len())
                    .saturating_sub(Self::encoded_entries_size(&entries) + MAX_MANIFEST_BYTES),
            );
            if connection.table_exists(&relation).await? {
                let query = match table {
                    Table::DatasetSources => format!(
                        "SELECT dataset_name, recording, spql.io.read_bytes(source) AS payload FROM {relation}"
                    ),
                    Table::TrainingCheckpoints => format!("SELECT * FROM {relation}"),
                };
                let mut records = connection.execute_query(query).await?;
                table.validate(records.schema().as_ref(), "project export")?;
                let mut writer = StreamWriter::try_new(&mut payload, records.schema().as_ref())
                    .map_err(|error| error.to_string())?;
                while let Some(batch) = records.next().await {
                    writer
                        .write(&batch.map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                }
                writer.finish().map_err(|error| error.to_string())?;
            } else {
                let mut writer = StreamWriter::try_new(&mut payload, &table.schema())
                    .map_err(|error| error.to_string())?;
                writer.finish().map_err(|error| error.to_string())?;
            }
            entries.push((table.name().to_owned(), payload.into_inner()));
        }
        let data = self.build(entries, source.datasets)?;
        Self::zip(&studio, &data)
    }

    fn zip(studio: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
        if studio.len().saturating_add(data.len()) > Self::MAX_BYTES {
            return Err("Project exceeds the uncompressed bundle size limit".into());
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        for (name, bytes) in [("studio.sqlite3", studio), ("spiral.data", data)] {
            zip.start_file(name, options)
                .map_err(|error| error.to_string())?;
            zip.write_all(bytes).map_err(|error| error.to_string())?;
        }
        let bytes = zip
            .finish()
            .map_err(|error| error.to_string())?
            .into_inner();
        if bytes.len() > Self::MAX_BYTES {
            return Err("Project exceeds the bundle size limit".into());
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<DecodedBundle, String> {
        if bytes.len() > Self::MAX_BYTES {
            return Err("Project exceeds the bundle size limit".into());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| "Not a version 3 project ZIP; older bundles are unsupported")?;
        if zip.len() != 2 {
            return Err("Project ZIP must contain exactly studio.sqlite3 and spiral.data".into());
        }
        let mut files = BTreeMap::new();
        let mut remaining = Self::MAX_BYTES;
        for index in 0..zip.len() {
            let file = zip.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().to_owned();
            if !matches!(name.as_str(), "studio.sqlite3" | "spiral.data")
                || files.contains_key(&name)
            {
                return Err("Project ZIP contains an unexpected or duplicate file".into());
            }
            if file.size() > remaining as u64 {
                return Err("Project exceeds the uncompressed bundle size limit".into());
            }
            let mut payload = Vec::new();
            file.take(remaining as u64 + 1)
                .read_to_end(&mut payload)
                .map_err(|error| error.to_string())?;
            remaining = remaining
                .checked_sub(payload.len())
                .ok_or("Project exceeds the uncompressed bundle size limit")?;
            files.insert(name, payload);
        }
        let data = files
            .remove("spiral.data")
            .ok_or("Project ZIP is missing spiral.data")?;
        let parsed = Self.parse(&data)?;
        let mut budget = ArrowDecodeBudget::default();
        let mut tables = Vec::new();
        for (name, payload) in parsed.entries {
            let table =
                Table::named(&name).ok_or_else(|| format!("Unknown data relation {name}"))?;
            let batches = budget.decode(ArrowPayload::new(&name, payload))?;
            table.validate(batches[0].schema().as_ref(), "project import")?;
            tables.push((table, batches));
        }
        Ok(DecodedBundle {
            studio: files
                .remove("studio.sqlite3")
                .ok_or("Project ZIP is missing studio.sqlite3")?,
            tables,
            datasets: parsed.datasets,
        })
    }

    pub(super) async fn import(
        &self,
        connection: &mut ProjectConnection,
        source: ProjectBundleImport<'_>,
    ) -> Result<ProjectImportSummary, String> {
        let ProjectBundleImport {
            table_prefix,
            studio,
            archive,
            current_datasets,
        } = source;
        let snapshot = StudioSnapshot::open(&archive.studio, studio).await?;
        let local_transaction = snapshot.stage(studio).await?;
        connection
            .begin_bundle_transaction("project import")
            .await?;
        let outcome = async {
            McapSources::retire(connection, &current_datasets).await?;
            let mut summary = ProjectImportSummary::default();
            for (table, batches) in archive.tables {
                summary.record_relation(
                    batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64
                );
                let batches = if table == Table::DatasetSources {
                    batches
                        .into_iter()
                        .map(McapSources::restore)
                        .collect::<Result<Vec<_>, _>>()?
                        .into_iter()
                        .flatten()
                        .collect()
                } else {
                    batches
                };
                connection
                    .replace_or_create_batches(&table.qualified_name(table_prefix), batches)
                    .await?;
            }
            McapSources::refresh(connection, &archive.datasets).await?;
            Ok(summary)
        }
        .await;
        let (mut summary, reconnect_warning) = connection
            .finish_bundle_transaction(outcome, "project import")
            .await?;
        local_transaction.commit().await.map_err(|error| format!("Spiral data was restored, but committing Studio records failed: {error}. Retry the same project import before continuing."))?;
        if let Some(warning) = reconnect_warning {
            summary.push_warning(warning);
        }
        Ok(summary)
    }
}

impl ProjectBundle {
    fn encoded_entries_size(entries: &[(String, Vec<u8>)]) -> usize {
        entries
            .iter()
            .map(|(name, payload)| 2 + name.len() + 8 + payload.len())
            .sum()
    }
}

impl ProjectBundle {
    fn build(
        &self,
        entries: Vec<(String, Vec<u8>)>,
        datasets: Vec<String>,
    ) -> Result<Vec<u8>, String> {
        if let Some((name, _)) = entries
            .iter()
            .find(|(name, _)| name.len() > MAX_ENTRY_NAME_BYTES)
        {
            return Err(format!("project relation name {name:?} is too long"));
        }
        let manifest = Manifest {
            format: PROJECT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            exported_ms: StorageClock::unix_millis(),
            relations: entries.iter().map(|(name, _)| name.clone()).collect(),
            mcap_datasets: datasets,
        };
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|error| format!("encoding project manifest: {error}"))?;
        if manifest.len() > MAX_MANIFEST_BYTES {
            return Err("Project manifest exceeds its size limit".into());
        }
        let mut bundle = Vec::new();
        bundle.extend_from_slice(PROJECT_MAGIC);
        BundleCodec::write_u32(&mut bundle, manifest.len())?;
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            BundleCodec::write_u16(&mut bundle, name.len())?;
            bundle.extend_from_slice(name.as_bytes());
            BundleCodec::write_u64(&mut bundle, payload.len())?;
            bundle.extend_from_slice(&payload);
        }
        bundle.extend_from_slice(&0_u16.to_le_bytes());
        if bundle.len() > Self::MAX_BYTES {
            return Err(format!(
                "project export exceeds the {} MiB portable-file limit",
                Self::MAX_BYTES / (1024 * 1024),
            ));
        }
        Ok(bundle)
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
            let first = BundleCodec::read_arrow_u32(payload, &mut position, relation)?;
            let metadata_len = if first == u32::MAX {
                BundleCodec::read_arrow_u32(payload, &mut position, relation)?
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
}

struct BundleCodec;

impl BundleCodec {
    fn read_arrow_u32(payload: &[u8], position: &mut usize, relation: &str) -> Result<u32, String> {
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

impl ProjectConnection {
    async fn begin_bundle_transaction(&mut self, operation: &str) -> Result<(), String> {
        if let Err(error) = self.begin_transaction().await {
            self.discard();
            return Err(format!(
                "starting {operation} transaction failed and its connection was discarded: {error}"
            ));
        }
        Ok(())
    }

    async fn finish_bundle_transaction<T>(
        &mut self,
        outcome: Result<T, String>,
        operation: &str,
    ) -> Result<(T, Option<String>), String> {
        let value = match outcome {
            Ok(value) => value,
            Err(primary) => match self.rollback().await {
                Ok(()) => return Err(primary),
                Err(rollback) => {
                    self.discard();
                    return Err(format!(
                        "{primary}; rolling back {operation} also failed: {rollback}; connection discarded"
                    ));
                }
            },
        };

        if let Err(commit) = self.commit().await {
            self.discard();
            return Err(format!(
                "committing {operation} failed with an uncertain outcome; connection discarded: {commit}"
            ));
        }
        match self.reconnect().await {
            Ok(()) => Ok((value, None)),
            Err(error) => {
                self.discard();
                Ok((
                    value,
                    Some(format!(
                        "{operation} committed, but its post-commit Spiral connection could not be opened and was discarded: {error}"
                    )),
                ))
            }
        }
    }
}

impl ProjectBundle {
    fn parse(&self, bundle: &[u8]) -> Result<ParsedBundle, String> {
        if bundle.len() > Self::MAX_BYTES {
            return Err(format!(
                "project file exceeds the {} MiB import limit",
                Self::MAX_BYTES / (1024 * 1024),
            ));
        }
        let mut source = Cursor::new(bundle);
        let mut magic = [0_u8; 8];
        source
            .read_exact(&mut magic)
            .map_err(|_| "project file is truncated".to_owned())?;
        if &magic != PROJECT_MAGIC {
            return Err(
                "Unsupported Spiral data format; only version 3 bundles are accepted".to_owned(),
            );
        }
        let manifest_len = BundleCodec::read_u32(&mut source)? as usize;
        if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
            return Err("project manifest has an invalid size".to_owned());
        }
        let mut manifest = vec![0_u8; manifest_len];
        source
            .read_exact(&mut manifest)
            .map_err(|_| "project manifest is truncated".to_owned())?;
        let manifest: Manifest = serde_json::from_slice(&manifest)
            .map_err(|error| format!("project manifest is invalid: {error}"))?;
        if manifest.format != PROJECT_FORMAT || manifest.format_version != FORMAT_VERSION {
            return Err("project file uses an unsupported format version".to_owned());
        }
        if manifest.schema_version != spiral::SCHEMA_VERSION {
            return Err(format!(
                "project schema {} cannot be imported by schema {}",
                manifest.schema_version,
                spiral::SCHEMA_VERSION
            ));
        }

        let dataset_names = manifest.mcap_datasets.iter().collect::<BTreeSet<_>>();
        if dataset_names.len() != manifest.mcap_datasets.len()
            || dataset_names
                .iter()
                .any(|name| !spiral::StorageSchema::is_dataset_id(name) || name.contains('.'))
        {
            return Err("Project manifest contains invalid or duplicate MCAP datasets".into());
        }
        let allowed = Table::ALL
            .map(|table| table.name().to_owned())
            .into_iter()
            .collect::<BTreeSet<_>>();
        if manifest.relations.iter().cloned().collect::<BTreeSet<_>>() != allowed {
            return Err("Project must contain exactly the current data-plane relations".into());
        }
        let relation_count = manifest.relations.len();
        let mut expected = manifest.relations.into_iter().collect::<BTreeSet<_>>();
        if expected.len() != relation_count {
            return Err("project manifest contains duplicate relations".to_owned());
        }
        let mut seen = BTreeSet::new();
        let mut entries = BTreeMap::new();
        loop {
            let name_len = BundleCodec::read_u16(&mut source)? as usize;
            if name_len == 0 {
                break;
            }
            if name_len > MAX_ENTRY_NAME_BYTES {
                return Err("project relation name is too long".to_owned());
            }
            let mut name = vec![0_u8; name_len];
            source
                .read_exact(&mut name)
                .map_err(|_| "project relation name is truncated".to_owned())?;
            let name = String::from_utf8(name)
                .map_err(|_| "project relation name is not UTF-8".to_owned())?;
            if !expected.remove(&name) || !seen.insert(name.clone()) {
                return Err(format!(
                    "project contains unexpected or duplicate relation {name:?}"
                ));
            }
            let payload_len = BundleCodec::read_u64(&mut source)?;
            let payload_len = usize::try_from(payload_len)
                .map_err(|_| format!("project relation {name:?} is too large for this machine"))?;
            let remaining = bundle.len().saturating_sub(source.position() as usize);
            if payload_len > remaining {
                return Err(format!("project relation {name:?} is truncated"));
            }
            let mut payload = vec![0_u8; payload_len];
            source
                .read_exact(&mut payload)
                .map_err(|_| format!("project relation {name:?} is truncated"))?;
            entries.insert(name, payload);
        }
        if !expected.is_empty() {
            return Err(format!(
                "project is missing relations: {}",
                expected.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if source.position() != bundle.len() as u64 {
            return Err("project file has trailing bytes".to_owned());
        }
        Ok(ParsedBundle {
            entries: entries.into_iter().collect(),
            datasets: manifest.mcap_datasets,
        })
    }
}
struct LimitedPayload {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedPayload {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl io::Write for LimitedPayload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(format!(
                "project export exceeds the {} MiB portable-file limit",
                self.limit / (1024 * 1024),
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl BundleCodec {
    fn write_u16(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        let value =
            u16::try_from(value).map_err(|_| "project entry name is too long".to_owned())?;
        target.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn write_u32(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        let value = u32::try_from(value).map_err(|_| "project manifest is too large".to_owned())?;
        target.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn write_u64(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        target.extend_from_slice(&(value as u64).to_le_bytes());
        Ok(())
    }
    fn read_u16(source: &mut Cursor<&[u8]>) -> Result<u16, String> {
        let mut bytes = [0; 2];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u16::from_le_bytes(bytes))
    }
    fn read_u32(source: &mut Cursor<&[u8]>) -> Result<u32, String> {
        let mut bytes = [0; 4];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn read_u64(source: &mut Cursor<&[u8]>) -> Result<u64, String> {
        let mut bytes = [0; 8];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests;
