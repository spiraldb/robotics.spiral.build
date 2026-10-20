//! An MCAP dataset's `files` table, one row per episode: its file's key and
//! an `spql.Blob` holding the file's bytes inline, for a file Studio
//! holds, or pointing at its pinned Hugging Face URI. A project bundle
//! carries every MCAP dataset's files as their Blobs hold them.
use arrow_array::{
    Array, ArrayRef, BinaryViewArray, RecordBatch, StringArray, StringViewArray, StructArray,
    UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema, SchemaRef};
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::datasets::DatasetSelection;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One MCAP episode's file: its key in the dataset, its size and SHA-256,
/// and its row of `files`.
pub(crate) struct McapFile {
    key: String,
    size: u64,
    sha256: String,
    uri: Option<String>,
    row: RecordBatch,
}

impl McapFile {
    /// `bytes`, which the dataset holds inline as `key`.
    pub(crate) fn held(key: &str, bytes: &[u8]) -> Result<Self, String> {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let row = Self::row_of(key, McapSourceBlob::held(bytes, &digest)?)?;
        Ok(Self {
            key: key.to_owned(),
            size: bytes.len() as u64,
            sha256,
            uri: None,
            row,
        })
    }

    /// `bytes`, read from the pinned Hugging Face `uri` the dataset points
    /// at as `key`.
    pub(crate) fn pinned(key: &str, uri: String, bytes: &[u8]) -> Result<Self, String> {
        let size = bytes.len() as u64;
        let row = Self::row_of(key, McapSourceBlob::pinned(&uri, size)?)?;
        Ok(Self {
            key: key.to_owned(),
            size,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            uri: Some(uri),
            row,
        })
    }

    /// The table's columns: each file's key, and the Blob holding it.
    pub(crate) fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("recording", DataType::Utf8, false),
            McapSourceBlob::field("source"),
        ]))
    }

    fn row_of(key: &str, source: ArrayRef) -> Result<RecordBatch, String> {
        RecordBatch::try_new(
            Self::schema(),
            vec![Arc::new(StringArray::from(vec![key])), source],
        )
        .map_err(|error| format!("building the files row of {key}: {error}"))
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }
    pub(crate) fn size(&self) -> u64 {
        self.size
    }
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Where it was read from, for a file the dataset does not hold.
    pub(crate) fn uri(&self) -> Option<&str> {
        self.uri.as_deref()
    }
    pub(crate) fn row(&self) -> &RecordBatch {
        &self.row
    }
}

/// An `spql.Blob`, as Spiral lays it out since spiraldb#7629: bytes held
/// inline, or a URI to read them from, and the bytes' multihash digest.
struct McapSourceBlob;

impl McapSourceBlob {
    const MEDIA_TYPE: &str = "application/mcap";
    /// The multihash prefix of a SHA2-256 digest: its code, then its length.
    const SHA2_256_MULTIHASH: [u8; 2] = [0x12, 0x20];

    fn fields() -> Fields {
        vec![
            Field::new("bytes", DataType::BinaryView, true),
            Field::new("uri", DataType::Utf8View, true),
            Field::new("offset", DataType::UInt64, true),
            Field::new("size", DataType::UInt64, true),
            Field::new("mime_type", DataType::Utf8View, true),
            Field::new("version", DataType::Utf8View, true),
            Field::new("etag", DataType::Utf8View, true),
            Field::new("digest", DataType::BinaryView, true),
        ]
        .into()
    }

    fn field(name: &str) -> Field {
        Field::new(name, DataType::Struct(Self::fields()), true)
            .with_metadata([(EXTENSION_TYPE_NAME_KEY.to_owned(), "spql.Blob".to_owned())].into())
    }

    /// One Blob holding `bytes`, whose SHA-256 is `sha256`.
    fn held(bytes: &[u8], sha256: &[u8; 32]) -> Result<ArrayRef, String> {
        let digest = [Self::SHA2_256_MULTIHASH.as_slice(), sha256].concat();
        Self::array(vec![
            Arc::new(BinaryViewArray::from(vec![Some(bytes)])),
            Self::no_text(),
            Self::no_number(),
            Arc::new(UInt64Array::from(vec![bytes.len() as u64])),
            Arc::new(StringViewArray::from(vec![Self::MEDIA_TYPE])),
            Self::no_text(),
            Self::no_text(),
            Arc::new(BinaryViewArray::from(vec![Some(digest.as_slice())])),
        ])
    }

    /// One Blob pointing at `uri`, `size` bytes long.
    fn pinned(uri: &str, size: u64) -> Result<ArrayRef, String> {
        Self::array(vec![
            Arc::new(BinaryViewArray::from(vec![None::<&[u8]>])),
            Arc::new(StringViewArray::from(vec![uri])),
            Self::no_number(),
            Arc::new(UInt64Array::from(vec![size])),
            Arc::new(StringViewArray::from(vec![Self::MEDIA_TYPE])),
            Self::no_text(),
            Self::no_text(),
            Arc::new(BinaryViewArray::from(vec![None::<&[u8]>])),
        ])
    }

    fn no_text() -> ArrayRef {
        Arc::new(StringViewArray::from(vec![None::<&str>]))
    }

    fn no_number() -> ArrayRef {
        Arc::new(UInt64Array::from(vec![None::<u64>]))
    }

    fn array(members: Vec<ArrayRef>) -> Result<ArrayRef, String> {
        StructArray::try_new(Self::fields(), members, None)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("building a Spiral Blob: {error}"))
    }
}

/// Every MCAP dataset's files as a project bundle carries them, in one
/// stream whose rows each name their dataset: a file the dataset holds by
/// its bytes, one it points at by its pinned Hugging Face URI, each with its
/// size.
pub(crate) struct McapSources;

/// One file a project bundle carries: its key, and its bytes or where they
/// are pinned.
#[derive(Debug, PartialEq)]
pub(crate) struct BundledFile<'a> {
    key: &'a str,
    source: BundledSource<'a>,
}

/// Where a bundled file's bytes are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BundledSource<'a> {
    /// In the bundle, since the dataset held them inline.
    Held(&'a [u8]),
    /// At the pinned Hugging Face `uri` the dataset points at, `size` bytes.
    Pinned { uri: &'a str, size: u64 },
}

impl<'a> BundledFile<'a> {
    pub(crate) fn key(&self) -> &'a str {
        self.key
    }
    pub(crate) fn source(&self) -> BundledSource<'a> {
        self.source
    }
}

impl McapSources {
    pub(crate) fn bundle_schema() -> Schema {
        Schema::new(vec![
            Field::new("dataset_name", DataType::Utf8, false),
            Field::new("recording", DataType::Utf8, false),
            Field::new("bytes", DataType::Binary, true),
            Field::new("uri", DataType::Utf8, true),
            Field::new("size", DataType::UInt64, false),
        ])
    }

    /// `dataset`'s files, each one's Blob read as it is: the bytes it holds,
    /// or the URI it points at, which is never downloaded.
    pub(crate) fn export_sql(dataset: &DatasetSelection) -> String {
        format!(
            "SELECT recording, source.bytes AS bytes, source.uri AS uri, source.size AS size FROM {}",
            dataset.files()
        )
    }

    /// Rows [`Self::export_sql`] read, as a project bundle carries them:
    /// each under the dataset it belongs to.
    pub(crate) fn bundled(
        dataset: &DatasetSelection,
        batch: &RecordBatch,
    ) -> Result<RecordBatch, String> {
        let schema = Arc::new(Self::bundle_schema());
        let mut columns = vec![Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
            dataset.id(),
            batch.num_rows(),
        ))) as ArrayRef];
        // Every column after the dataset's name is the query's, in the
        // bundle's types.
        for field in schema.fields().iter().skip(1) {
            let column = batch
                .column_by_name(field.name())
                .ok_or_else(|| format!("MCAP source export has no {}", field.name()))?;
            columns.push(
                arrow_cast::cast(column, field.data_type())
                    .map_err(|error| format!("MCAP source export: {error}"))?,
            );
        }
        RecordBatch::try_new(schema, columns)
            .map_err(|error| format!("MCAP source export: {error}"))
    }

    /// The files a project bundle carries, under each dataset its manifest
    /// `names`; a dataset with no files has none.
    pub(crate) fn files<'a>(
        batches: &'a [RecordBatch],
        names: &[DatasetSelection],
    ) -> Result<BTreeMap<DatasetSelection, Vec<BundledFile<'a>>>, String> {
        let mut files = names
            .iter()
            .map(|name| (name.clone(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        for batch in batches {
            let columns = crate::db::ArrowColumns::new(batch, "MCAP source backup");
            let bytes = batch
                .column_by_name("bytes")
                .ok_or("MCAP backup has no bytes")?;
            for row in 0..batch.num_rows() {
                let dataset = DatasetSelection::new(columns.text("dataset_name", row)?)?;
                let key = columns.text("recording", row)?;
                let source = match (
                    Self::bytes(bytes.as_ref(), row)?,
                    columns.optional_text("uri", row)?,
                ) {
                    (Some(bytes), None) => BundledSource::Held(bytes),
                    (None, Some(uri)) => BundledSource::Pinned {
                        uri,
                        size: columns
                            .optional_unsigned("size", row)?
                            .ok_or_else(|| format!("bundled file {key} has no size"))?,
                    },
                    _ => {
                        return Err(format!(
                            "bundled file {key} must carry either its bytes or its pinned URI"
                        ));
                    }
                };
                files
                    .get_mut(&dataset)
                    .ok_or("MCAP source names do not match the project manifest")?
                    .push(BundledFile { key, source });
            }
        }
        Ok(files)
    }

    /// The bytes at `row`, if it has any.
    fn bytes(array: &dyn Array, row: usize) -> Result<Option<&[u8]>, String> {
        if array.is_null(row) {
            return Ok(None);
        }
        if let Some(array) = array.as_any().downcast_ref::<arrow_array::BinaryArray>() {
            return Ok(Some(array.value(row)));
        }
        if let Some(array) = array.as_any().downcast_ref::<BinaryViewArray>() {
            return Ok(Some(array.value(row)));
        }
        if let Some(array) = array
            .as_any()
            .downcast_ref::<arrow_array::LargeBinaryArray>()
        {
            return Ok(Some(array.value(row)));
        }
        Err("MCAP source bytes must be binary".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_restores_every_file_into_its_own_dataset() {
        let uri = "hf://datasets/org/recordings@abcdef/two.mcap";
        let rows = |bytes: Vec<Option<&[u8]>>, uris: Vec<Option<&str>>| {
            RecordBatch::try_new(
                Arc::new(McapSources::bundle_schema()),
                vec![
                    Arc::new(StringArray::from(vec!["first", "second"])),
                    Arc::new(StringArray::from(vec!["one.mcap", "two.mcap"])),
                    Arc::new(arrow_array::BinaryArray::from(bytes)),
                    Arc::new(StringArray::from(uris)),
                    Arc::new(UInt64Array::from(vec![10, 11])),
                ],
            )
            .unwrap()
        };
        let batch = rows(
            vec![Some(b"first file".as_slice()), None],
            vec![None, Some(uri)],
        );
        let names = ["first", "second", "empty"].map(|name| DatasetSelection::new(name).unwrap());
        let restored = McapSources::files(std::slice::from_ref(&batch), &names).unwrap();
        assert_eq!(
            restored
                .keys()
                .map(DatasetSelection::id)
                .collect::<Vec<_>>(),
            ["empty", "first", "second"]
        );
        let file = |key, source| BundledFile { key, source };
        assert_eq!(
            restored[&names[0]],
            [file("one.mcap", BundledSource::Held(b"first file"))]
        );
        assert_eq!(
            restored[&names[1]],
            [file("two.mcap", BundledSource::Pinned { uri, size: 11 })]
        );
        assert!(restored[&names[2]].is_empty());
        assert!(McapSources::files(&[batch], &names[..1]).is_err());
        for (bytes, uris) in [
            (
                vec![Some(b"both".as_slice()), None],
                vec![Some(uri), Some(uri)],
            ),
            (vec![Some(b"neither".as_slice()), None], vec![None, None]),
        ] {
            let error = McapSources::files(&[rows(bytes, uris)], &names).unwrap_err();
            assert!(
                error.contains("either its bytes or its pinned URI"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_files_row_holds_its_bytes_or_points_at_its_pinned_uri() {
        let held = McapFile::held("take.mcap", b"recording").unwrap();
        let pinned = McapFile::pinned(
            "episodes/take.mcap",
            "hf://datasets/org/recordings@abcdef/episodes/take.mcap".into(),
            b"recording",
        )
        .unwrap();
        assert_eq!(
            (held.size(), held.sha256()),
            (pinned.size(), pinned.sha256())
        );
        assert_eq!(held.uri(), None);
        for file in [&held, &pinned] {
            assert_eq!(file.row().schema(), McapFile::schema());
        }
        let blob = |file: &McapFile, member: &str| {
            let source = file.row().column_by_name("source").unwrap();
            let source = source.as_any().downcast_ref::<StructArray>().unwrap();
            source.column_by_name(member).unwrap().clone()
        };
        let bytes = blob(&held, "bytes");
        assert_eq!(
            McapSources::bytes(bytes.as_ref(), 0).unwrap(),
            Some(b"recording".as_slice())
        );
        assert!(blob(&pinned, "bytes").is_null(0));
        let uri = blob(&pinned, "uri");
        assert_eq!(
            uri.as_any()
                .downcast_ref::<StringViewArray>()
                .unwrap()
                .value(0),
            "hf://datasets/org/recordings@abcdef/episodes/take.mcap"
        );
    }
}
