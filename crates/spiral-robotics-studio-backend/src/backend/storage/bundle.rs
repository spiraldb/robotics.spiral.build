//! Portable single-file snapshots of the Studio-owned Spiral namespace.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{ArrayRef, LargeStringArray, RecordBatch, StringArray, StringViewArray};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use serde::{Deserialize, Serialize};
use spiral_robotics_contracts::api::{EpisodeImportSummary, ProjectImportSummary};
use spiral_robotics_contracts::spiral::{self, Table, column};

use crate::backend::storage::manager::{
    commit_catalog_transaction, ensure_dataset_namespace, episode_video_dataset_exists,
    execute_delete_sql, execute_update_sql, ingest_batch, rebuild_episode_video_dataset, run_query,
    table_exists,
};
use crate::backend::storage::repository::RepositoryConnection;
use crate::backend::storage::sql;

const PROJECT_MAGIC: &[u8; 8] = b"SPRPROJ1";
const EPISODES_MAGIC: &[u8; 8] = b"SPREPS01";
const PROJECT_FORMAT: &str = "spiral-robotics-project";
const EPISODES_FORMAT: &str = "spiral-robotics-episodes";
const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 256;
/// Browser transfers are memory-backed at the Axum/storage boundary. Keep the
/// cap low enough that one import and its decoded Arrow batch cannot exhaust a
/// typical Studio workstation.
pub(crate) const MAX_PROJECT_BUNDLE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_EPISODE_BUNDLE_BYTES: usize = MAX_PROJECT_BUNDLE_BYTES;

const EPISODE_TABLES: [Table; 5] = [
    Table::Datasets,
    Table::Episodes,
    Table::EpisodeStreams,
    Table::JointSamples,
    Table::VideoAssets,
];

#[derive(Clone, Copy)]
struct BundleSpec {
    magic: &'static [u8; 8],
    format: &'static str,
    label: &'static str,
    tables: &'static [Table],
    max_bytes: usize,
}

const PROJECT_SPEC: BundleSpec = BundleSpec {
    magic: PROJECT_MAGIC,
    format: PROJECT_FORMAT,
    label: "project",
    tables: &Table::ALL,
    max_bytes: MAX_PROJECT_BUNDLE_BYTES,
};

const EPISODES_SPEC: BundleSpec = BundleSpec {
    magic: EPISODES_MAGIC,
    format: EPISODES_FORMAT,
    label: "episode bundle",
    tables: &EPISODE_TABLES,
    max_bytes: MAX_EPISODE_BUNDLE_BYTES,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    format: String,
    format_version: u32,
    schema_version: u32,
    exported_ms: i64,
    relations: Vec<String>,
}

struct ExportedBundle {
    bytes: Vec<u8>,
    row_counts: Vec<(Table, u64)>,
}

pub(crate) fn export(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<Vec<u8>, String> {
    let mut queries = Vec::new();
    for table in Table::ALL {
        let relation = sql::relation(namespace, table);
        if !table_exists(connection, &relation)? {
            continue;
        }
        queries.push((table, format!("SELECT * FROM {relation}")));
    }
    export_queries(connection, queries, PROJECT_SPEC).map(|export| export.bytes)
}

pub(crate) fn export_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    dataset_id: &str,
) -> Result<Vec<u8>, String> {
    validate_dataset_id(dataset_id)?;
    let datasets = sql::relation(namespace, Table::Datasets);
    let episodes = sql::relation(namespace, Table::Episodes);
    if !table_exists(connection, &datasets)? || !table_exists(connection, &episodes)? {
        return Err(format!("dataset {dataset_id:?} has no episodes to export"));
    }
    let dataset = sql::string_literal(dataset_id);
    let episode_ids = format!(
        "SELECT {episode_id} FROM {episodes} WHERE {dataset_id_column} = {dataset}",
        episode_id = column::EPISODE_ID,
        dataset_id_column = column::DATASET_ID,
    );
    let mut queries = Vec::new();
    for table in EPISODE_TABLES {
        let relation = sql::relation(namespace, table);
        if !table_exists(connection, &relation)? {
            continue;
        }
        let query = match table {
            Table::Datasets | Table::Episodes => format!(
                "SELECT * FROM {relation} WHERE {} = {dataset}",
                column::DATASET_ID
            ),
            Table::EpisodeStreams | Table::JointSamples | Table::VideoAssets => format!(
                "SELECT * FROM {relation} WHERE {} IN ({episode_ids})",
                column::EPISODE_ID
            ),
            _ => unreachable!("episode bundle table set is closed"),
        };
        queries.push((table, query));
    }
    let export = export_queries(connection, queries, EPISODES_SPEC)?;
    if export
        .row_counts
        .iter()
        .find_map(|(table, rows)| (*table == Table::Episodes).then_some(*rows))
        .unwrap_or(0)
        == 0
    {
        return Err(format!("dataset {dataset_id:?} has no episodes to export"));
    }
    Ok(export.bytes)
}

fn export_queries(
    connection: &mut RepositoryConnection,
    queries: Vec<(Table, String)>,
    spec: BundleSpec,
) -> Result<ExportedBundle, String> {
    let mut entries = Vec::<(String, Vec<u8>)>::new();
    let mut row_counts = Vec::new();
    for (table, query) in queries {
        let relation = table.name();
        let mut statement = connection
            .new_statement()
            .map_err(|error| format!("opening {} export for {relation}: {error}", spec.label))?;
        statement
            .set_sql_query(query)
            .map_err(|error| format!("binding {} export for {relation}: {error}", spec.label))?;
        let reader = statement
            .execute()
            .map_err(|error| format!("reading {} relation {relation}: {error}", spec.label))?;
        let occupied = encoded_entries_size(&entries)
            .saturating_add(spec.magic.len() + 4 + MAX_MANIFEST_BYTES + 2);
        let payload_limit = spec
            .max_bytes
            .saturating_sub(occupied)
            .saturating_sub(2 + table.name().len() + 8);
        let mut payload = LimitedPayload::new(payload_limit, spec.label);
        let mut rows = 0_u64;
        {
            let mut writer = StreamWriter::try_new(&mut payload, reader.schema().as_ref())
                .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
            for batch in reader {
                let batch = batch.map_err(|error| {
                    format!("reading {} relation {relation}: {error}", spec.label)
                })?;
                rows = rows.saturating_add(batch.num_rows() as u64);
                writer
                    .write(&batch)
                    .map_err(|error| format!("writing Arrow stream for {relation}: {error}"))?;
            }
            writer
                .finish()
                .map_err(|error| format!("finishing Arrow stream for {relation}: {error}"))?;
        }
        let payload = payload.into_inner();
        let projected_size = encoded_entries_size(&entries)
            .saturating_add(2 + table.name().len() + 8 + payload.len())
            .saturating_add(spec.magic.len() + 4 + MAX_MANIFEST_BYTES);
        if projected_size > spec.max_bytes {
            return Err(format!(
                "{} export exceeds the {} MiB portable-file limit",
                spec.label,
                spec.max_bytes / (1024 * 1024),
            ));
        }
        row_counts.push((table, rows));
        entries.push((table.name().to_owned(), payload));
    }
    build_bundle(entries, spec).map(|bytes| ExportedBundle { bytes, row_counts })
}

fn encoded_entries_size(entries: &[(String, Vec<u8>)]) -> usize {
    entries
        .iter()
        .map(|(name, payload)| 2 + name.len() + 8 + payload.len())
        .sum()
}

fn build_bundle(entries: Vec<(String, Vec<u8>)>, spec: BundleSpec) -> Result<Vec<u8>, String> {
    let manifest = Manifest {
        format: spec.format.to_owned(),
        format_version: FORMAT_VERSION,
        schema_version: spiral::SCHEMA_VERSION,
        exported_ms: unix_millis(),
        relations: entries.iter().map(|(name, _)| name.clone()).collect(),
    };
    let manifest = serde_json::to_vec(&manifest)
        .map_err(|error| format!("encoding {} manifest: {error}", spec.label))?;
    let mut bundle = Vec::new();
    bundle.extend_from_slice(spec.magic);
    write_u32(&mut bundle, manifest.len())?;
    bundle.extend_from_slice(&manifest);
    for (name, payload) in entries {
        write_u16(&mut bundle, name.len())?;
        bundle.extend_from_slice(name.as_bytes());
        write_u64(&mut bundle, payload.len())?;
        bundle.extend_from_slice(&payload);
    }
    bundle.extend_from_slice(&0_u16.to_le_bytes());
    if bundle.len() > spec.max_bytes {
        return Err(format!(
            "{} export exceeds the {} MiB portable-file limit",
            spec.label,
            spec.max_bytes / (1024 * 1024),
        ));
    }
    Ok(bundle)
}

pub(crate) fn import(
    connection: &mut RepositoryConnection,
    namespace: &str,
    bundle: &[u8],
) -> Result<ProjectImportSummary, String> {
    let mut decoded = decode_entries(parse(bundle)?, PROJECT_SPEC)?;
    sort_relations(&mut decoded, &Table::ALL);
    let imported_video_episodes = video_episode_ids(&decoded)?;
    let existing_video_episodes = stored_video_episode_ids(connection, namespace)?;

    connection
        .set_autocommit(false)
        .map_err(|error| format!("starting project import transaction: {error}"))?;
    let outcome = (|| {
        // Retire only relations absent from the incoming project. Relations in
        // the bundle keep their catalog identity: their rows are cleared and
        // rebuilt below. Retiring and recreating one name in a single project
        // transaction is an unresolved resource conflict in SpiralDB, while
        // hosted Arbor does not support CREATE OR REPLACE TABLE.
        for table in Table::ALL.into_iter().rev() {
            if decoded.iter().any(|(incoming, _)| *incoming == table) {
                continue;
            }
            let relation = sql::relation(namespace, table);
            if table_exists(connection, &relation)? {
                execute_update_sql(connection, &format!("DROP TABLE {relation}"))
                    .map_err(|error| format!("clearing project relation {relation}: {error}"))?;
            }
        }

        let mut summary = ProjectImportSummary::default();
        for (table, batches) in decoded {
            let relation = sql::relation(namespace, table);
            summary.relations += 1;
            if table_exists(connection, &relation)? {
                execute_delete_sql(connection, &format!("DELETE FROM {relation}"))
                    .map_err(|error| format!("clearing project relation {relation}: {error}"))?;
            }
            for batch in batches {
                let batch = if table == Table::VideoAssets {
                    retarget_video_media_relations(batch, namespace)?
                } else {
                    batch
                };
                summary.rows = summary.rows.saturating_add(batch.num_rows() as u64);
                ingest_batch(connection, &relation, batch)?;
            }
        }
        Ok(summary)
    })();

    let (mut summary, transaction_warning) =
        finish_transaction(connection, outcome, "project import")?;
    let can_reconcile = transaction_warning.is_none();
    if let Some(warning) = transaction_warning {
        summary.warnings.push(warning);
    }
    // ADBC ingest can create a relation inside the import transaction, but
    // Spiral's SQL binder cannot resolve that new relation until the commit is
    // published. Keep the repository's exclusive checkout while reconciling
    // the derived MP4 resources immediately after the durable rows commit.
    if can_reconcile {
        if let Err(error) = reconcile_project_video_datasets(
            connection,
            namespace,
            &existing_video_episodes,
            &imported_video_episodes,
        ) {
            summary.warnings.push(format!(
                "project rows committed, but derived video datasets need repair: {error}"
            ));
        }
    } else {
        summary.warnings.push(
            "project rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
    }
    Ok(summary)
}

pub(crate) fn import_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    dataset_id: &str,
    bundle: &[u8],
) -> Result<EpisodeImportSummary, String> {
    validate_dataset_id(dataset_id)?;
    let mut decoded = decode_entries(parse_bundle(bundle, EPISODES_SPEC)?, EPISODES_SPEC)?;
    sort_relations(&mut decoded, &EPISODE_TABLES);
    let episode_ids = validate_episode_relations(&decoded)?;
    let video_episode_ids = video_episode_ids(&decoded)?;
    reject_existing_episodes(connection, namespace, &episode_ids)?;
    ensure_dataset_namespace(connection, namespace)?;

    connection
        .set_autocommit(false)
        .map_err(|error| format!("starting episode import transaction: {error}"))?;
    let outcome = (|| {
        let datasets = sql::relation(namespace, Table::Datasets);
        let dataset_exists = table_exists(connection, &datasets)?
            && !run_query(connection, &sql::dataset_exists(&datasets, dataset_id), 1)?
                .rows
                .is_empty();
        let mut summary = EpisodeImportSummary {
            dataset_id: dataset_id.to_owned(),
            episodes: episode_ids.len() as u64,
            rows: 0,
            first_episode: episode_ids.first().cloned(),
            warnings: Vec::new(),
        };
        for (table, batches) in decoded {
            if table == Table::Datasets && dataset_exists {
                continue;
            }
            let relation = sql::relation(namespace, table);
            for batch in batches {
                let batch = match table {
                    Table::Datasets | Table::Episodes => {
                        replace_utf8_column(batch, column::DATASET_ID, dataset_id)?
                    }
                    Table::VideoAssets => retarget_video_media_relations(batch, namespace)?,
                    _ => batch,
                };
                summary.rows = summary.rows.saturating_add(batch.num_rows() as u64);
                ingest_batch(connection, &relation, batch)?;
            }
        }
        Ok(summary)
    })();
    let (mut summary, transaction_warning) =
        finish_transaction(connection, outcome, "episode import")?;
    let can_reconcile = transaction_warning.is_none();
    if let Some(warning) = transaction_warning {
        summary.warnings.push(warning);
    }
    if can_reconcile {
        for episode_id in &video_episode_ids {
            let result = episode_video_dataset_exists(connection, namespace, episode_id).and_then(
                |dataset_exists| {
                    rebuild_episode_video_dataset(connection, namespace, episode_id, dataset_exists)
                },
            );
            if let Err(error) = result {
                summary.warnings.push(format!(
                    "episode {episode_id} rows committed, but its derived video dataset needs repair: {error}"
                ));
            }
        }
    } else if !video_episode_ids.is_empty() {
        summary.warnings.push(
            "episode rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
    }
    Ok(summary)
}

fn decode_entries(
    entries: Vec<(Table, Vec<u8>)>,
    spec: BundleSpec,
) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
    let mut decoded = Vec::new();
    for (table, payload) in entries {
        let reader = StreamReader::try_new(Cursor::new(payload), None)
            .map_err(|error| format!("opening Arrow stream for {table}: {error}"))?;
        let schema = reader.schema();
        let actual = schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect::<Vec<_>>();
        if actual != table.columns() {
            return Err(format!(
                "{} relation {table} has columns {:?}; expected {:?}",
                spec.label,
                actual,
                table.columns()
            ));
        }
        let mut batches = reader
            .map(|batch| {
                batch.map_err(|error| format!("reading Arrow stream for {table}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if batches.is_empty() {
            batches.push(RecordBatch::new_empty(schema));
        }
        decoded.push((table, batches));
    }
    Ok(decoded)
}

fn finish_transaction<T>(
    connection: &mut RepositoryConnection,
    outcome: Result<T, String>,
    operation: &str,
) -> Result<(T, Option<String>), String> {
    let final_result = match outcome {
        Ok(value) => commit_catalog_transaction(connection)
            .map(|()| value)
            .map_err(|error| format!("committing {operation}: {error}")),
        Err(error) => {
            let rollback = connection.rollback();
            Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!("{error}; rolling back also failed: {rollback}"),
            })
        }
    };
    let autocommit = connection
        .set_autocommit(true)
        .map_err(|error| format!("restoring Spiral autocommit after {operation}: {error}"));
    match (final_result, autocommit) {
        (Ok(value), Ok(())) => Ok((value, None)),
        (Err(error), _) => Err(error),
        (Ok(value), Err(error)) => {
            // The data is already durable. Report that truth to the operator,
            // but never return the connection to the pool with unknown
            // transaction state.
            connection.discard();
            Ok((
                value,
                Some(format!(
                    "{operation} committed, but its Spiral connection was discarded: {error}"
                )),
            ))
        }
    }
}

fn sort_relations(decoded: &mut [(Table, Vec<RecordBatch>)], order: &[Table]) {
    decoded.sort_by_key(|(table, _)| {
        order
            .iter()
            .position(|candidate| candidate == table)
            .unwrap_or(usize::MAX)
    });
}

fn validate_episode_relations(
    decoded: &[(Table, Vec<RecordBatch>)],
) -> Result<Vec<String>, String> {
    if !decoded.iter().any(|(table, _)| *table == Table::Datasets)
        || !decoded.iter().any(|(table, _)| *table == Table::Episodes)
    {
        return Err("episode bundle must contain datasets and episodes relations".to_owned());
    }
    let dataset_rows = decoded
        .iter()
        .filter(|(table, _)| *table == Table::Datasets)
        .flat_map(|(_, batches)| batches)
        .map(RecordBatch::num_rows)
        .sum::<usize>();
    if dataset_rows != 1 {
        return Err("episode bundle must describe exactly one source dataset".to_owned());
    }
    let mut source_datasets = Vec::new();
    for (_, batches) in decoded
        .iter()
        .filter(|(table, _)| *table == Table::Datasets)
    {
        for batch in batches {
            source_datasets.extend(utf8_values(batch, column::DATASET_ID)?);
        }
    }
    let source_dataset = source_datasets
        .pop()
        .ok_or_else(|| "episode bundle source dataset has no dataset ID".to_owned())?;
    validate_dataset_id(&source_dataset)?;
    let mut episode_ids = BTreeSet::new();
    for (table, batches) in decoded {
        if *table == Table::Episodes {
            for batch in batches {
                let ids = utf8_values(batch, column::EPISODE_ID)?;
                let datasets = utf8_values(batch, column::DATASET_ID)?;
                if datasets.iter().any(|dataset| dataset != &source_dataset) {
                    return Err(format!(
                        "episode bundle contains an episode outside source dataset {source_dataset:?}"
                    ));
                }
                for episode_id in ids {
                    validate_episode_id(&episode_id)?;
                    if !episode_ids.insert(episode_id.clone()) {
                        return Err(format!("episode bundle repeats episode ID {episode_id:?}"));
                    }
                }
            }
        }
    }
    if episode_ids.is_empty() {
        return Err("episode bundle contains no episodes".to_owned());
    }
    for (table, batches) in decoded {
        if matches!(
            table,
            Table::EpisodeStreams | Table::JointSamples | Table::VideoAssets
        ) {
            for batch in batches {
                for episode_id in utf8_values(batch, column::EPISODE_ID)? {
                    if !episode_ids.contains(&episode_id) {
                        return Err(format!(
                            "episode bundle relation {table} references unknown episode {episode_id:?}"
                        ));
                    }
                }
            }
        }
    }
    Ok(episode_ids.into_iter().collect())
}

fn video_episode_ids(decoded: &[(Table, Vec<RecordBatch>)]) -> Result<BTreeSet<String>, String> {
    let mut episode_ids = BTreeSet::new();
    for (_, batches) in decoded
        .iter()
        .filter(|(table, _)| *table == Table::VideoAssets)
    {
        for batch in batches {
            for episode_id in utf8_values(batch, column::EPISODE_ID)? {
                validate_episode_id(&episode_id)?;
                episode_ids.insert(episode_id);
            }
        }
    }
    Ok(episode_ids)
}

fn stored_video_episode_ids(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<BTreeSet<String>, String> {
    const MAX_VIDEO_EPISODES: usize = 100_000;

    let assets = sql::relation(namespace, Table::VideoAssets);
    if !table_exists(connection, &assets)? {
        return Ok(BTreeSet::new());
    }
    let result = run_query(
        connection,
        &format!(
            "SELECT DISTINCT {episode_id} FROM {assets}",
            episode_id = column::EPISODE_ID
        ),
        MAX_VIDEO_EPISODES,
    )?;
    if result.truncated {
        return Err(format!(
            "project has more than {MAX_VIDEO_EPISODES} video episodes; refusing an incomplete derived-resource replacement"
        ));
    }
    result
        .rows
        .into_iter()
        .map(|row| {
            let episode_id = row
                .get(column::EPISODE_ID)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "stored video asset has no valid episode ID".to_owned())?
                .to_owned();
            validate_episode_id(&episode_id)?;
            Ok(episode_id)
        })
        .collect()
}

fn reconcile_project_video_datasets(
    connection: &mut RepositoryConnection,
    namespace: &str,
    previous: &BTreeSet<String>,
    imported: &BTreeSet<String>,
) -> Result<(), String> {
    for episode_id in previous.difference(imported) {
        if episode_video_dataset_exists(connection, namespace, episode_id)? {
            drop_episode_video_dataset(connection, namespace, episode_id)?;
        }
    }
    for episode_id in imported {
        let dataset_exists = episode_video_dataset_exists(connection, namespace, episode_id)?;
        rebuild_episode_video_dataset(connection, namespace, episode_id, dataset_exists)?;
    }
    Ok(())
}

fn drop_episode_video_dataset(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode_id: &str,
) -> Result<(), String> {
    let dataset = sql::video_dataset_relation(namespace, episode_id);
    execute_update_sql(connection, &sql::drop_video_dataset(&dataset))
        .map_err(|error| format!("dropping stale Spiral MP4 dataset for {episode_id}: {error}"))?;
    let sources = sql::video_sources_relation(namespace, episode_id, true);
    execute_update_sql(connection, &sql::drop_video_sources(&sources))
        .map_err(|error| format!("dropping stale Spiral video source for {episode_id}: {error}"))
}

fn utf8_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|error| format!("episode bundle is missing column {name}: {error}"))?;
    let values = batch.column(index);
    if let Some(values) = values.as_any().downcast_ref::<StringArray>() {
        return collect_utf8(values.iter(), name);
    }
    if let Some(values) = values.as_any().downcast_ref::<LargeStringArray>() {
        return collect_utf8(values.iter(), name);
    }
    if let Some(values) = values.as_any().downcast_ref::<StringViewArray>() {
        return collect_utf8(values.iter(), name);
    }
    Err(format!("episode bundle column {name} is not UTF-8 text"))
}

fn collect_utf8<'a>(
    values: impl Iterator<Item = Option<&'a str>>,
    name: &str,
) -> Result<Vec<String>, String> {
    values
        .map(|value| {
            value
                .map(str::to_owned)
                .ok_or_else(|| format!("episode bundle column {name} contains a null value"))
        })
        .collect()
}

fn replace_utf8_column(batch: RecordBatch, name: &str, value: &str) -> Result<RecordBatch, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|error| format!("episode bundle is missing column {name}: {error}"))?;
    let mut columns = batch.columns().to_vec();
    let source = &columns[index];
    columns[index] = if source.as_any().is::<StringArray>() {
        Arc::new(StringArray::from(vec![value; batch.num_rows()])) as ArrayRef
    } else if source.as_any().is::<LargeStringArray>() {
        Arc::new(LargeStringArray::from(vec![value; batch.num_rows()])) as ArrayRef
    } else if source.as_any().is::<StringViewArray>() {
        Arc::new(StringViewArray::from(vec![value; batch.num_rows()])) as ArrayRef
    } else {
        return Err(format!("episode bundle column {name} is not UTF-8 text"));
    };
    RecordBatch::try_new(batch.schema(), columns)
        .map_err(|error| format!("retargeting episode bundle dataset: {error}"))
}

fn retarget_video_media_relations(
    batch: RecordBatch,
    namespace: &str,
) -> Result<RecordBatch, String> {
    let episode_ids = utf8_values(&batch, column::EPISODE_ID)?;
    let media_relations = episode_ids
        .iter()
        .map(|episode_id| sql::video_dataset_output(namespace, episode_id))
        .collect::<Vec<_>>();
    let index = batch
        .schema()
        .index_of(column::MEDIA_RELATION)
        .map_err(|error| {
            format!(
                "episode bundle is missing column {}: {error}",
                column::MEDIA_RELATION
            )
        })?;
    let mut columns = batch.columns().to_vec();
    let source = &columns[index];
    columns[index] = if source.as_any().is::<StringArray>() {
        Arc::new(StringArray::from_iter_values(
            media_relations.iter().map(String::as_str),
        )) as ArrayRef
    } else if source.as_any().is::<LargeStringArray>() {
        Arc::new(LargeStringArray::from_iter_values(
            media_relations.iter().map(String::as_str),
        )) as ArrayRef
    } else if source.as_any().is::<StringViewArray>() {
        Arc::new(StringViewArray::from_iter_values(
            media_relations.iter().map(String::as_str),
        )) as ArrayRef
    } else {
        return Err(format!(
            "episode bundle column {} is not UTF-8 text",
            column::MEDIA_RELATION
        ));
    };
    RecordBatch::try_new(batch.schema(), columns)
        .map_err(|error| format!("retargeting episode bundle media relations: {error}"))
}

fn reject_existing_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode_ids: &[String],
) -> Result<(), String> {
    let episodes = sql::relation(namespace, Table::Episodes);
    if !table_exists(connection, &episodes)? {
        return Ok(());
    }
    for chunk in episode_ids.chunks(100) {
        let candidates = chunk
            .iter()
            .map(|episode_id| sql::string_literal(episode_id))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT {episode_id} FROM {episodes} WHERE {episode_id} IN ({candidates})",
            episode_id = column::EPISODE_ID,
        );
        let existing = run_query(connection, &query, chunk.len())?;
        if let Some(value) = existing
            .rows
            .first()
            .and_then(|row| row.get(column::EPISODE_ID))
            .and_then(serde_json::Value::as_str)
        {
            return Err(format!(
                "episode {value:?} already exists; episode import never overwrites existing episodes"
            ));
        }
    }
    Ok(())
}

fn validate_dataset_id(dataset_id: &str) -> Result<(), String> {
    if !spiral::is_dataset_id(dataset_id) {
        return Err("dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'".to_owned());
    }
    Ok(())
}

fn validate_episode_id(episode_id: &str) -> Result<(), String> {
    if episode_id.is_empty()
        || episode_id.len() > 128
        || !episode_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(format!(
            "episode bundle contains invalid episode ID {episode_id:?}"
        ));
    }
    Ok(())
}

fn parse(bundle: &[u8]) -> Result<Vec<(Table, Vec<u8>)>, String> {
    parse_bundle(bundle, PROJECT_SPEC)
}

fn parse_bundle(bundle: &[u8], spec: BundleSpec) -> Result<Vec<(Table, Vec<u8>)>, String> {
    if bundle.len() > spec.max_bytes {
        return Err(format!(
            "{} file exceeds the {} MiB import limit",
            spec.label,
            spec.max_bytes / (1024 * 1024),
        ));
    }
    let mut source = Cursor::new(bundle);
    let mut magic = [0_u8; 8];
    source
        .read_exact(&mut magic)
        .map_err(|_| format!("{} file is truncated", spec.label))?;
    if &magic != spec.magic {
        return Err(format!("file is not a Spiral Robotics {}", spec.label));
    }
    let manifest_len = read_u32(&mut source)? as usize;
    if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
        return Err(format!("{} manifest has an invalid size", spec.label));
    }
    let mut manifest = vec![0_u8; manifest_len];
    source
        .read_exact(&mut manifest)
        .map_err(|_| format!("{} manifest is truncated", spec.label))?;
    let manifest: Manifest = serde_json::from_slice(&manifest)
        .map_err(|error| format!("{} manifest is invalid: {error}", spec.label))?;
    if manifest.format != spec.format || manifest.format_version != FORMAT_VERSION {
        return Err(format!(
            "{} file uses an unsupported format version",
            spec.label
        ));
    }
    if manifest.schema_version != spiral::SCHEMA_VERSION {
        return Err(format!(
            "{} schema {} cannot be imported by schema {}",
            spec.label,
            manifest.schema_version,
            spiral::SCHEMA_VERSION
        ));
    }

    let relation_count = manifest.relations.len();
    let mut expected = manifest.relations.into_iter().collect::<BTreeSet<_>>();
    if expected.len() != relation_count {
        return Err(format!(
            "{} manifest contains duplicate relations",
            spec.label
        ));
    }
    if let Some(name) = expected
        .iter()
        .find(|name| table_named_in(name, spec.tables).is_none())
    {
        return Err(format!(
            "{} manifest contains unknown relation {name:?}",
            spec.label
        ));
    }
    let mut seen = BTreeSet::new();
    let mut entries = BTreeMap::new();
    loop {
        let name_len = read_u16(&mut source)? as usize;
        if name_len == 0 {
            break;
        }
        if name_len > MAX_ENTRY_NAME_BYTES {
            return Err(format!("{} relation name is too long", spec.label));
        }
        let mut name = vec![0_u8; name_len];
        source
            .read_exact(&mut name)
            .map_err(|_| format!("{} relation name is truncated", spec.label))?;
        let name = String::from_utf8(name)
            .map_err(|_| format!("{} relation name is not UTF-8", spec.label))?;
        if !expected.remove(&name) || !seen.insert(name.clone()) {
            return Err(format!(
                "{} contains unexpected or duplicate relation {name:?}",
                spec.label
            ));
        }
        let table = table_named_in(&name, spec.tables)
            .ok_or_else(|| format!("{} contains unknown relation {name:?}", spec.label))?;
        let payload_len = read_u64(&mut source)?;
        let payload_len = usize::try_from(payload_len).map_err(|_| {
            format!(
                "{} relation {name:?} is too large for this machine",
                spec.label
            )
        })?;
        let remaining = bundle.len().saturating_sub(source.position() as usize);
        if payload_len > remaining {
            return Err(format!("{} relation {name:?} is truncated", spec.label));
        }
        let mut payload = vec![0_u8; payload_len];
        source
            .read_exact(&mut payload)
            .map_err(|_| format!("{} relation {name:?} is truncated", spec.label))?;
        entries.insert(name, (table, payload));
    }
    if !expected.is_empty() {
        return Err(format!(
            "{} is missing relations: {}",
            spec.label,
            expected.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if source.position() != bundle.len() as u64 {
        return Err(format!("{} file has trailing bytes", spec.label));
    }
    Ok(entries.into_values().collect())
}

fn table_named_in(name: &str, tables: &[Table]) -> Option<Table> {
    tables.iter().copied().find(|table| table.name() == name)
}

struct LimitedPayload {
    bytes: Vec<u8>,
    limit: usize,
    label: &'static str,
}

impl LimitedPayload {
    fn new(limit: usize, label: &'static str) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            label,
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
                "{} export exceeds the {} MiB portable-file limit",
                self.label,
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
fn write_u16(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value = u16::try_from(value).map_err(|_| "project entry name is too long".to_owned())?;
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
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    use arrow_array::{
        ArrayRef, BinaryArray, Float64Array, Int64Array, LargeBinaryArray, StringArray,
        UInt32Array, UInt64Array,
    };
    use arrow_schema::{DataType, Field, Schema};

    use super::*;
    use crate::backend::recording::episodes::{EpisodeCompletion, EpisodeStart};
    use crate::backend::recording::spiral::{dataset_batch, episode_completion_batch};
    use crate::backend::storage::manager::{
        StorageManager, ensure_dataset_namespace, execute_update_sql, ingest_batch, run_query,
        table_exists,
    };
    use crate::backend::storage::repository::SpiralRepository;
    use crate::backend::storage::settings::{
        BackendStateStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
    };

    static SPIRAL_E2E: Mutex<()> = Mutex::new(());

    fn envelope(manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        envelope_for(PROJECT_MAGIC, manifest, entries)
    }

    fn envelope_for(magic: &[u8; 8], manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bundle = Vec::new();
        bundle.extend_from_slice(magic);
        write_u32(&mut bundle, manifest.len()).unwrap();
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            write_u16(&mut bundle, name.len()).unwrap();
            bundle.extend_from_slice(name.as_bytes());
            write_u64(&mut bundle, payload.len()).unwrap();
            bundle.extend_from_slice(payload);
        }
        bundle.extend_from_slice(&0_u16.to_le_bytes());
        bundle
    }

    fn manifest(relations: Vec<String>) -> Manifest {
        manifest_for(PROJECT_FORMAT, relations)
    }

    fn manifest_for(format: &str, relations: Vec<String>) -> Manifest {
        Manifest {
            format: format.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            exported_ms: 1,
            relations,
        }
    }

    fn text_batch(columns: &[(&str, &[&str])]) -> RecordBatch {
        let rows = columns.first().map_or(0, |(_, values)| values.len());
        assert!(columns.iter().all(|(_, values)| values.len() == rows));
        record_batch(
            columns
                .iter()
                .map(|(name, _)| Field::new(*name, DataType::Utf8, false))
                .collect(),
            columns
                .iter()
                .map(|(_, values)| Arc::new(StringArray::from(values.to_vec())) as ArrayRef)
                .collect(),
        )
    }

    #[test]
    fn empty_project_envelope_is_valid() {
        assert_eq!(
            parse(&envelope(manifest(Vec::new()), &[])).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn manifest_and_payload_relations_must_match_exactly() {
        let name = Table::Datasets.name().to_owned();
        let missing = envelope(manifest(vec![name.clone()]), &[]);
        assert!(parse(&missing).unwrap_err().contains("missing relations"));

        let unexpected = envelope(manifest(Vec::new()), &[(name.as_str(), b"arrow")]);
        assert!(
            parse(&unexpected)
                .unwrap_err()
                .contains("unexpected or duplicate")
        );
    }

    #[test]
    fn duplicate_manifest_relations_are_rejected() {
        let name = Table::Datasets.name().to_owned();
        let bundle = envelope(manifest(vec![name.clone(), name]), &[]);
        assert!(parse(&bundle).unwrap_err().contains("duplicate relations"));
    }

    #[test]
    fn project_and_episode_bundle_formats_are_not_interchangeable() {
        let project = envelope(manifest(Vec::new()), &[]);
        assert!(
            parse_bundle(&project, EPISODES_SPEC)
                .unwrap_err()
                .contains("not a Spiral Robotics episode bundle")
        );

        let episodes = envelope_for(
            EPISODES_MAGIC,
            manifest_for(EPISODES_FORMAT, Vec::new()),
            &[],
        );
        assert!(
            parse(&episodes)
                .unwrap_err()
                .contains("not a Spiral Robotics project")
        );
    }

    #[test]
    fn episode_bundle_rejects_project_only_relations() {
        let robots = Table::Robots.name().to_owned();
        let bundle = envelope_for(
            EPISODES_MAGIC,
            manifest_for(EPISODES_FORMAT, vec![robots]),
            &[],
        );
        assert!(
            parse_bundle(&bundle, EPISODES_SPEC)
                .unwrap_err()
                .contains("unknown relation")
        );
    }

    #[test]
    fn episode_bundle_requires_unique_ids_and_closed_references() {
        let dataset = text_batch(&[(column::DATASET_ID, &["source"])]);
        let duplicate_episodes = text_batch(&[
            (column::EPISODE_ID, &["episode_1", "episode_1"]),
            (column::DATASET_ID, &["source", "source"]),
        ]);
        let duplicate = vec![
            (Table::Datasets, vec![dataset.clone()]),
            (Table::Episodes, vec![duplicate_episodes]),
        ];
        assert!(
            validate_episode_relations(&duplicate)
                .unwrap_err()
                .contains("repeats episode ID")
        );

        let episodes = text_batch(&[
            (column::EPISODE_ID, &["episode_1"]),
            (column::DATASET_ID, &["source"]),
        ]);
        let unknown_child = text_batch(&[(column::EPISODE_ID, &["episode_2"])]);
        let dangling = vec![
            (Table::Datasets, vec![dataset]),
            (Table::Episodes, vec![episodes]),
            (Table::JointSamples, vec![unknown_child]),
        ];
        assert!(
            validate_episode_relations(&dangling)
                .unwrap_err()
                .contains("references unknown episode")
        );
    }

    #[test]
    fn episode_bundle_requires_one_consistent_source_dataset() {
        let decoded = vec![
            (
                Table::Datasets,
                vec![text_batch(&[(column::DATASET_ID, &["source"])])],
            ),
            (
                Table::Episodes,
                vec![text_batch(&[
                    (column::EPISODE_ID, &["episode_1"]),
                    (column::DATASET_ID, &["other"]),
                ])],
            ),
        ];
        assert!(
            validate_episode_relations(&decoded)
                .unwrap_err()
                .contains("outside source dataset")
        );
    }

    #[test]
    fn imported_video_assets_are_retargeted_to_the_destination_namespace() {
        let batch = text_batch(&[
            (column::EPISODE_ID, &["episode_a", "episode-b"]),
            (
                column::MEDIA_RELATION,
                &["source.video_a.assets", "source.video_b.assets"],
            ),
        ]);

        let retargeted = retarget_video_media_relations(batch, "destination").unwrap();

        assert_eq!(
            utf8_values(&retargeted, column::MEDIA_RELATION).unwrap(),
            vec![
                sql::video_dataset_output("destination", "episode_a"),
                sql::video_dataset_output("destination", "episode-b"),
            ]
        );
        assert_eq!(
            video_episode_ids(&[(Table::VideoAssets, vec![retargeted])]).unwrap(),
            BTreeSet::from(["episode-b".to_owned(), "episode_a".to_owned()])
        );
    }

    #[test]
    fn forged_payload_length_is_rejected_before_allocation() {
        let name = Table::Datasets.name().to_owned();
        let mut bundle = envelope(manifest(vec![name.clone()]), &[(name.as_str(), b"")]);
        let length_offset = bundle.len() - 2 - 8;
        bundle[length_offset..length_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let error = parse(&bundle).unwrap_err();
        assert!(error.contains("too large for this machine") || error.contains("truncated"));
    }

    #[test]
    fn export_payload_writer_enforces_its_bound() {
        let mut payload = LimitedPayload::new(3, "project");
        payload.write_all(b"abc").unwrap();
        assert!(
            payload
                .write_all(b"d")
                .unwrap_err()
                .to_string()
                .contains("portable-file limit")
        );
    }

    /// Run with a real SpiralDB stack:
    ///
    /// ```text
    /// SPIRAL_PROJECT_ID=project-id \
    /// cargo test -p spiral-robotics-studio-backend \
    ///   backend::storage::bundle::tests::spiral_export_import_round_trip_overwrites_existing_data \
    ///   -- --ignored --exact
    /// ```
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_export_import_round_trip_overwrites_existing_data() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::StudioSettings),
                    settings_batch("exported-setting", r#"{"source":"export"}"#, 10),
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::TrainingCheckpoints),
                    checkpoint_batch(),
                )
            })
            .expect("seeding the exported Spiral project must succeed");

        let exported = storage
            .export_project_bundle()
            .expect("exporting the Spiral project must succeed");
        let exported_payloads = relation_payloads(&exported);
        assert_eq!(
            exported_payloads.keys().cloned().collect::<Vec<_>>(),
            vec![
                Table::StudioSettings.name().to_owned(),
                Table::TrainingCheckpoints.name().to_owned(),
            ]
        );

        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::StudioSettings),
                    settings_batch("replacement-setting", r#"{"source":"replacement"}"#, 20),
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::RolloutRuns),
                    snapshot_batch("replacement-run"),
                )
            })
            .expect("seeding conflicting Spiral data must succeed");

        let replacement = storage
            .export_project_bundle()
            .expect("exporting the replacement state must succeed");
        assert_ne!(relation_payloads(&replacement), exported_payloads);

        let summary = storage
            .import_project_bundle(&exported)
            .expect("importing the Spiral project must succeed");
        assert_eq!(summary.relations, 2);
        assert_eq!(summary.rows, 2);

        let restored = storage
            .export_project_bundle()
            .expect("re-exporting the imported Spiral project must succeed");
        assert_eq!(relation_payloads(&restored), exported_payloads);
        repository
            .run_blocking(|connection| {
                assert!(!table_exists(
                    connection,
                    &sql::relation(repository.namespace(), Table::RolloutRuns)
                )?);
                Ok(())
            })
            .expect("checking overwrite semantics must succeed");
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_episode_export_import_round_trip_merges_without_overwrite() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_episode_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral episode E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                seed_episode(
                    connection,
                    repository.namespace(),
                    "source",
                    "episode_exported",
                    10,
                )
            })
            .expect("seeding the source episode must succeed");

        let exported = storage
            .export_episode_bundle("source")
            .expect("exporting source episodes must succeed");
        let exported_relations = parse_bundle(&exported, EPISODES_SPEC)
            .expect("the exported episode bundle must parse")
            .into_iter()
            .map(|(table, _)| table)
            .collect::<Vec<_>>();
        assert_eq!(exported_relations, vec![Table::Datasets, Table::Episodes]);

        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())?;
                seed_episode(
                    connection,
                    repository.namespace(),
                    "target",
                    "episode_existing",
                    20,
                )
            })
            .expect("seeding the target dataset must succeed");

        let summary = storage
            .import_episode_bundle("target", &exported)
            .expect("importing episodes into the target dataset must succeed");
        assert_eq!(summary.dataset_id, "target");
        assert_eq!(summary.episodes, 1);
        assert_eq!(summary.first_episode.as_deref(), Some("episode_exported"));

        repository
            .run_blocking(|connection| {
                let episodes = sql::relation(repository.namespace(), Table::Episodes);
                let result = run_query(
                    connection,
                    &format!(
                        "SELECT {episode_id}, {dataset_id} FROM {episodes} ORDER BY {episode_id}",
                        episode_id = column::EPISODE_ID,
                        dataset_id = column::DATASET_ID,
                    ),
                    10,
                )?;
                assert_eq!(result.rows.len(), 2);
                assert!(result.rows.iter().all(|row| {
                    row.get(column::DATASET_ID)
                        .and_then(serde_json::Value::as_str)
                        == Some("target")
                }));
                Ok(())
            })
            .expect("the imported and pre-existing episodes must both remain");

        let collision = storage
            .import_episode_bundle("target", &exported)
            .expect_err("a repeated episode import must refuse the ID collision");
        assert!(collision.contains("never overwrites existing episodes"));
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, disposable project access, and FFmpeg"]
    fn spiral_project_import_rebuilds_managed_video_datasets() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_video_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral video import E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());
        let episode_id = "episode_video_import";
        let start = EpisodeStart {
            id: episode_id.to_owned(),
            dataset_id: "video-transfer".to_owned(),
            started_ms: 10,
            sample_hz: 2.0,
            source: "transfer-e2e".to_owned(),
            note: None,
            arms: vec![spiral_robotics_contracts::api::EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("test".to_owned()),
                joints: vec!["joint".to_owned()],
            }],
            condition: Vec::new(),
        };
        let completion = EpisodeCompletion {
            duration_seconds: 1.0,
            recording_gaps: Vec::new(),
            max_follow_error_degrees: None,
            discarded: false,
            condition: Vec::new(),
        };
        let mp4 = test_mp4();

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::Datasets),
                    dataset_batch(&start).map_err(|error| error.to_string())?,
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::Episodes),
                    episode_completion_batch(&start, &completion, 0)
                        .map_err(|error| error.to_string())?,
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::VideoAssets),
                    video_asset_batch(episode_id, &mp4),
                )
            })
            .expect("seeding durable video rows must succeed without derived resources");
        let exported = storage
            .export_project_bundle()
            .expect("exporting the video project must succeed");
        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())
            })
            .expect("clearing the relational fixture before import must succeed");

        storage
            .import_project_bundle(&exported)
            .expect("importing a video project must rebuild its managed datasets");
        storage
            .import_project_bundle(&exported)
            .expect("re-importing a video project must replace its managed dataset outputs");

        repository
            .run_blocking(|connection| {
                let output = sql::video_dataset_output(repository.namespace(), episode_id);
                let rows = run_query(connection, &sql::count_rows(&output), 1)?;
                assert_eq!(
                    rows.rows[0].get("rows").and_then(serde_json::Value::as_u64),
                    Some(1)
                );
                let frames = run_query(
                    connection,
                    &format!(
                        "SELECT COUNT(*) AS count FROM {output} AS asset, \
                         LATERAL spql.video.frames(asset.media, \
                         spql.video.frame_indices(ARRAY[0])) AS frame"
                    ),
                    1,
                )?;
                assert_eq!(
                    frames.rows[0]
                        .get("count")
                        .and_then(serde_json::Value::as_u64),
                    Some(1)
                );
                Ok(())
            })
            .expect("the rebuilt managed dataset must serve an indexed video frame");
    }

    fn required_project() -> String {
        std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| BackendStateStore::open_default().ok()?.project_id())
            .expect(
                "SPIRAL_PROJECT_ID or a project saved in Studio Settings is required for the Spiral E2E test",
            )
    }

    fn stored_credentials() -> Option<crate::backend::storage::settings::SpiralCredentials> {
        BackendStateStore::open_default().ok()?.credentials()
    }

    fn configured_spiral_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_endpoint())
            .unwrap_or_else(environment_spiral_endpoint)
    }

    fn configured_spiral_spfs_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_spfs_endpoint())
            .unwrap_or_else(environment_spiral_spfs_endpoint)
    }

    fn relation_payloads(bundle: &[u8]) -> BTreeMap<String, Vec<u8>> {
        parse(bundle)
            .expect("the exported project must parse")
            .into_iter()
            .map(|(table, payload)| (table.name().to_owned(), payload))
            .collect()
    }

    fn settings_batch(key: &str, value: &str, updated_ms: i64) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::KEY, DataType::Utf8, false),
                Field::new(spiral::column::VALUE_JSON, DataType::Utf8, false),
                Field::new(spiral::column::UPDATED_MS, DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![key])) as ArrayRef,
                Arc::new(StringArray::from(vec![value])) as ArrayRef,
                Arc::new(Int64Array::from(vec![updated_ms])) as ArrayRef,
            ],
        )
    }

    fn checkpoint_batch() -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::RUN_ID, DataType::Utf8, false),
                Field::new(spiral::column::RUN_NAME, DataType::Utf8, false),
                Field::new(spiral::column::POLICY, DataType::Utf8, false),
                Field::new(spiral::column::STEP, DataType::UInt64, false),
                Field::new(spiral::column::CREATED_MS, DataType::Int64, false),
                Field::new(spiral::column::VALIDATION_LOSS, DataType::Float64, true),
                Field::new(spiral::column::CONFIG_JSON, DataType::Utf8, false),
                Field::new(spiral::column::CHECKPOINT, DataType::LargeBinary, false),
            ],
            vec![
                Arc::new(StringArray::from(vec!["exported-run"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["Exported run"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["act"])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![42])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Float64Array::from(vec![Some(0.125)])) as ArrayRef,
                Arc::new(StringArray::from(vec![r#"{"chunk_size":16}"#])) as ArrayRef,
                Arc::new(LargeBinaryArray::from_iter_values([
                    b"checkpoint-binary".as_slice()
                ])) as ArrayRef,
            ],
        )
    }

    fn snapshot_batch(run_id: &str) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::RUN_ID, DataType::Utf8, false),
                Field::new(spiral::column::SNAPSHOT_JSON, DataType::Utf8, false),
                Field::new(spiral::column::UPDATED_MS, DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![run_id])) as ArrayRef,
                Arc::new(StringArray::from(vec![r#"{"source":"replacement"}"#])) as ArrayRef,
                Arc::new(Int64Array::from(vec![20])) as ArrayRef,
            ],
        )
    }

    fn seed_episode(
        connection: &mut RepositoryConnection,
        namespace: &str,
        dataset_id: &str,
        episode_id: &str,
        started_ms: i64,
    ) -> Result<(), String> {
        let start = EpisodeStart {
            id: episode_id.to_owned(),
            dataset_id: dataset_id.to_owned(),
            started_ms,
            sample_hz: 60.0,
            source: "transfer-e2e".to_owned(),
            note: None,
            arms: Vec::new(),
            condition: Vec::new(),
        };
        let completion = EpisodeCompletion {
            duration_seconds: 0.0,
            recording_gaps: Vec::new(),
            max_follow_error_degrees: None,
            discarded: false,
            condition: Vec::new(),
        };
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::Datasets),
            dataset_batch(&start).map_err(|error| error.to_string())?,
        )?;
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::Episodes),
            episode_completion_batch(&start, &completion, 0).map_err(|error| error.to_string())?,
        )
    }

    fn test_mp4() -> Vec<u8> {
        let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
        let output = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=16x16:r=2:d=1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "frag_keyframe+empty_moov+default_base_moof",
                "-f",
                "mp4",
                "pipe:1",
            ])
            .output()
            .unwrap_or_else(|error| panic!("could not run {ffmpeg:?}: {error}"));
        assert!(
            output.status.success(),
            "FFmpeg fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty(), "FFmpeg fixture emitted no MP4");
        output.stdout
    }

    fn video_asset_batch(episode_id: &str, payload: &[u8]) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::EPISODE_ID, DataType::Utf8, false),
                Field::new(spiral::column::STREAM_ID, DataType::Utf8, false),
                Field::new(spiral::column::SEGMENT_INDEX, DataType::UInt32, false),
                Field::new(spiral::column::TIMELINE_OFFSET_NS, DataType::UInt64, true),
                Field::new(spiral::column::CAMERA_ID, DataType::Utf8, false),
                Field::new(spiral::column::PAYLOAD, DataType::Binary, false),
                Field::new(spiral::column::BYTE_SIZE, DataType::UInt64, false),
                Field::new(spiral::column::WIDTH, DataType::UInt32, false),
                Field::new(spiral::column::HEIGHT, DataType::UInt32, false),
                Field::new(spiral::column::FPS, DataType::UInt32, false),
                Field::new(spiral::column::FRAMES, DataType::UInt64, false),
                Field::new(spiral::column::MEDIA_RELATION, DataType::Utf8, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![episode_id])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera.main"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![0])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(0)])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera-e2e"])) as ArrayRef,
                Arc::new(BinaryArray::from_iter_values([payload])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![payload.len() as u64])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![16])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![16])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![2])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![2])) as ArrayRef,
                Arc::new(StringArray::from(vec!["source.wrong.assets"])) as ArrayRef,
            ],
        )
    }

    fn record_batch(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .expect("the E2E fixture batch must match its schema")
    }

    fn drop_studio_relations(
        connection: &mut RepositoryConnection,
        namespace: &str,
    ) -> Result<(), String> {
        for episode_id in stored_video_episode_ids(connection, namespace)? {
            if episode_video_dataset_exists(connection, namespace, &episode_id)? {
                drop_episode_video_dataset(connection, namespace, &episode_id)?;
            }
        }
        for table in Table::ALL.into_iter().rev() {
            let relation = sql::relation(namespace, table);
            if table_exists(connection, &relation)? {
                execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
            }
        }
        Ok(())
    }

    struct TestNamespace {
        repository: Arc<SpiralRepository>,
    }

    impl TestNamespace {
        fn new(repository: Arc<SpiralRepository>) -> Self {
            Self { repository }
        }
    }

    impl Drop for TestNamespace {
        fn drop(&mut self) {
            let namespace = self.repository.namespace().to_owned();
            let _ = self.repository.run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, &namespace)?;
                if table_exists(connection, &namespace)? {
                    execute_update_sql(connection, &format!("DROP TABLE {namespace}"))?;
                }
                Ok(())
            });
        }
    }
}
