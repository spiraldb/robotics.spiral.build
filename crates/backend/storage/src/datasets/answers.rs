//! Each dataset's answers table, `dataset_answers_<id>`: made from the
//! dataset's first refresh, given a row by every append that adds an episode,
//! and dropped with it; and the columns fill runs write into it.
//!
//! A fill writes one column at a time, for the episodes it computed: their
//! answers, held as Arrow, are ingested into a staging table, the column is
//! added the first time, merged in by `episode_id`, and the staging table is
//! dropped. Spiral adds a column only as nullable and spelled in SQL, with
//! every nested field nullable, so an answer's Arrow type is spelled out for
//! it and every answer cast to what the table stores.
use super::{EpisodeUuids, StudioDataset};
use crate::{ProjectConnection, ProjectStore, StorageManager};
use arrow_array::{Array, ArrayRef, ListArray, RecordBatch, StructArray};
use arrow_buffer::OffsetBuffer;
use arrow_schema::{DataType, Field, Fields, Schema};
use spiral_robotics_common::curation::{AnswerColumn, AnswerTable};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::sql::{SqlLiteral, UnquotedSqlIdentifier};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The field every stored answer is wrapped in: a NULL struct is an episode
/// not asked yet, a NULL `value` one the model could not answer.
const VALUE: &str = "value";
/// The column of a fill statement's result holding its answer.
const ANSWER: &str = "answer";
impl StudioDataset {
    /// Create the answers table with one row per episode the dataset holds
    /// now, and no answer yet.
    pub(crate) async fn create_answers(
        &self,
        connection: &mut ProjectConnection,
    ) -> Result<(), String> {
        connection
            .execute_update_sql(&format!(
                "CREATE TABLE {} AS SELECT id AS episode_id FROM spql.mcap.episodes({})",
                self.answers().quoted(),
                SqlLiteral::text(self.dataset())
            ))
            .await
            .map_err(|error| format!("creating {}: {error}", self.answers().name()))
    }

    /// Give every episode the dataset holds now and the answers table does
    /// not a row with no answer. Spiral inserts only rows that name every
    /// column, so they are read first.
    pub(crate) async fn add_answer_rows(
        &self,
        connection: &mut ProjectConnection,
    ) -> Result<(), String> {
        let table = self.answers();
        let schema = connection
            .relation_schema(&table.name())
            .await?
            .ok_or_else(|| format!("{} has no answers table.", self.dataset()))?;
        let (columns, values): (Vec<_>, Vec<_>) = schema
            .fields()
            .iter()
            .map(|field| {
                let value = if field.name() == "episode_id" {
                    "s.id"
                } else {
                    "NULL"
                };
                (field.name().as_str(), value)
            })
            .unzip();
        connection
            .execute_update_sql(&format!(
                "MERGE INTO {} t USING (SELECT id FROM spql.mcap.episodes({})) s ON t.episode_id = s.id \
                 WHEN NOT MATCHED THEN INSERT ({}) VALUES ({})",
                table.quoted(),
                SqlLiteral::text(self.dataset()),
                columns.join(", "),
                values.join(", ")
            ))
            .await
            .map_err(|error| format!("adding episodes to {}: {error}", table.name()))
    }
}

impl StorageManager {
    /// What fill runs read and write of the answers table `table`.
    pub fn dataset_answers(&self, table: AnswerTable) -> DatasetAnswers<'_> {
        DatasetAnswers {
            store: self.project_store(),
            table,
        }
    }
}

/// One dataset's answers table, as fill runs read and write its columns.
pub struct DatasetAnswers<'a> {
    store: &'a ProjectStore,
    table: AnswerTable,
}

impl DatasetAnswers<'_> {
    /// The episodes `column` holds an answer for, failed ones included; none
    /// before the column exists.
    pub async fn filled(&self, column: &AnswerColumn) -> Result<BTreeSet<EpisodeId>, String> {
        let mut connection = self.store.connect().await?;
        if Self::stored_type(&self.schema(&mut connection).await?, column).is_none() {
            return Ok(BTreeSet::new());
        }
        let batches = connection
            .read_batches(&format!(
                "SELECT episode_id FROM {} WHERE {column} IS NOT NULL",
                self.table.quoted()
            ))
            .await?;
        let mut filled = BTreeSet::new();
        for batch in batches {
            let ids = batch.column_by_name("episode_id").ok_or("no episode_id")?;
            filled.extend(EpisodeUuids::read(ids.as_ref())?);
        }
        Ok(filled)
    }

    /// Write `answers` into `column`, adding the column the first time.
    pub async fn write(&self, column: &AnswerColumn, answers: ColumnAnswers) -> Result<(), String> {
        if answers.is_empty() {
            return Ok(());
        }
        let mut connection = self.store.connect_exclusive().await?;
        let stored = Self::stored_type(&self.schema(&mut connection).await?, column);
        let batch = answers.batch(column, stored.as_ref())?;
        let added = match stored {
            Some(_) => None,
            None => Some(StoredType::sql(batch.schema().field(1).data_type())?),
        };
        let staging = format!("{}_staging_{column}", self.table.name());
        connection
            .replace_or_create_batches(&staging, vec![batch])
            .await?;
        let staging = format!("\"{}\"", staging.replace('"', "\"\""));
        let table = self.table.quoted();
        let written = async {
            if let Some(spelled) = added {
                connection
                    .execute_update_sql(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {spelled}"
                    ))
                    .await?;
            }
            connection
                .execute_update_sql(&format!(
                    "MERGE INTO {table} t USING {staging} s ON t.episode_id = s.episode_id \
                     WHEN MATCHED THEN UPDATE SET {column} = s.{column}"
                ))
                .await
        }
        .await;
        let dropped = connection
            .execute_update_sql(&format!("DROP TABLE IF EXISTS {staging}"))
            .await;
        written
            .and(dropped)
            .map_err(|error| format!("writing {column} to {}: {error}", self.table.name()))
    }

    async fn schema(&self, connection: &mut ProjectConnection) -> Result<Schema, String> {
        connection
            .relation_schema(&self.table.name())
            .await?
            .ok_or_else(|| {
                format!(
                    "{} does not exist; only datasets Studio imported keep model answers.",
                    self.table.name()
                )
            })
    }

    /// The type of what `column` stores, when the table has it.
    fn stored_type(schema: &Schema, column: &AnswerColumn) -> Option<DataType> {
        schema
            .field_with_name(column.as_str())
            .ok()
            .map(|field| field.data_type().clone())
    }
}

/// One episode's answer to one call: one row of the answer's type, NULL
/// where the model could not answer.
#[derive(Clone, Debug)]
pub struct EpisodeAnswer(ArrayRef);

impl EpisodeAnswer {
    /// What a fill statement answered for the episode it binds: its one
    /// row's `answer`. A statement that answers no row gives no answer.
    pub fn from_statement(schema: &Schema, batches: &[RecordBatch]) -> Result<Self, String> {
        let answer = schema
            .index_of(ANSWER)
            .map_err(|_| "A fill statement must answer in `answer`.".to_owned())?;
        let answers = batches
            .iter()
            .map(|batch| batch.column(answer).clone())
            .collect::<Vec<_>>();
        match answers.iter().map(|rows| rows.len()).sum::<usize>() {
            0 => Ok(Self::failed(schema.field(answer).data_type())),
            1 => Self::concatenated(&answers).map(Self),
            rows => Err(format!(
                "A fill statement answered {rows} rows for one episode."
            )),
        }
    }

    /// Every row of `rows` as one list answer, in order.
    pub fn listing(rows: ArrayRef) -> Self {
        let field = Arc::new(Field::new_list_field(rows.data_type().clone(), true));
        let offsets = OffsetBuffer::from_lengths([rows.len()]);
        Self(Arc::new(ListArray::new(field, offsets, rows, None)))
    }

    /// No answer of `answer_type`: the model could not answer.
    pub fn failed(answer_type: &DataType) -> Self {
        Self(arrow_array::new_null_array(answer_type, 1))
    }

    pub fn is_failed(&self) -> bool {
        self.0.is_null(0)
    }

    /// The answer, as its one row.
    pub fn value(&self) -> &ArrayRef {
        &self.0
    }

    fn concatenated(arrays: &[ArrayRef]) -> Result<ArrayRef, String> {
        let arrays = arrays.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        arrow_select::concat::concat(&arrays).map_err(|error| error.to_string())
    }
}

/// The answers one fill holds for one column until it writes them: each
/// episode it computed, beside its answer.
#[derive(Default)]
pub struct ColumnAnswers {
    episodes: Vec<EpisodeId>,
    answers: Vec<ArrayRef>,
}

impl ColumnAnswers {
    pub fn hold(&mut self, episode: EpisodeId, answer: EpisodeAnswer) {
        self.episodes.push(episode);
        self.answers.push(answer.0);
    }
    pub fn len(&self) -> usize {
        self.episodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }

    /// The staging rows: `episode_id` and `column`, each answer wrapped as
    /// `STRUCT(value)` and cast to `stored`, the column's type in the table,
    /// or for a new column to the answers' own type with every field
    /// nullable, as Spiral will store it.
    fn batch(
        self,
        column: &AnswerColumn,
        stored: Option<&DataType>,
    ) -> Result<RecordBatch, String> {
        let first = self.answers.first().ok_or("No answers to write.")?;
        let value_type = match stored {
            Some(DataType::Struct(fields)) if fields.len() == 1 && fields[0].name() == VALUE => {
                fields[0].data_type().clone()
            }
            Some(other) => return Err(format!("{column} stores {other}, not an answer.")),
            None => StoredType::nullable(first.data_type()),
        };
        let answers = self
            .answers
            .iter()
            .map(|answer| arrow_cast::cast(answer, &value_type))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("{column}'s answers differ in type: {error}"))?;
        let values = EpisodeAnswer::concatenated(&answers)?;
        let wrapped = StructArray::try_new(
            Fields::from(vec![Field::new(VALUE, value_type, true)]),
            vec![values],
            None,
        )
        .map_err(|error| error.to_string())?;
        let schema = Schema::new(vec![
            EpisodeUuids::field("episode_id"),
            Field::new(column.as_str(), wrapped.data_type().clone(), true),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![EpisodeUuids::array(&self.episodes)?, Arc::new(wrapped)],
        )
        .map_err(|error| error.to_string())
    }
}

/// An answer's type as the answers table stores it.
struct StoredType;

impl StoredType {
    /// `data_type` with every nested field nullable, as Spiral adds a column.
    fn nullable(data_type: &DataType) -> DataType {
        let field = |field: &Field| {
            Arc::new(Field::new(
                field.name(),
                Self::nullable(field.data_type()),
                true,
            ))
        };
        match data_type {
            DataType::List(item) => DataType::List(field(item)),
            DataType::LargeList(item) => DataType::LargeList(field(item)),
            DataType::Struct(fields) => {
                DataType::Struct(fields.iter().map(|item| field(item)).collect())
            }
            other => other.clone(),
        }
    }

    /// `data_type` spelled as Spiral's `ALTER TABLE` reads a column type.
    fn sql(data_type: &DataType) -> Result<String, String> {
        Ok(match data_type {
            DataType::Boolean => "boolean".into(),
            DataType::Int8 => "tinyint".into(),
            DataType::Int16 => "smallint".into(),
            DataType::Int32 => "integer".into(),
            DataType::Int64 => "bigint".into(),
            DataType::UInt8 => "uint8".into(),
            DataType::UInt16 => "uint16".into(),
            DataType::UInt32 => "uint32".into(),
            DataType::UInt64 => "uint64".into(),
            DataType::Float32 => "real".into(),
            DataType::Float64 => "double".into(),
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "varchar".into(),
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => "varbinary".into(),
            DataType::List(item) | DataType::LargeList(item) => {
                format!("{}[]", Self::sql(item.data_type())?)
            }
            DataType::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        UnquotedSqlIdentifier::try_from(field.name().as_str())
                            .map_err(|_| format!("An answer field is named {:?}.", field.name()))?;
                        Ok(format!(
                            "{} {}",
                            field.name(),
                            Self::sql(field.data_type())?
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                format!("STRUCT({})", fields.join(", "))
            }
            other => return Err(format!("An answer of type {other} cannot be stored.")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{
        Float32Array, Float64Array, Int64Array, StringArray, UInt32Array, UInt64Array,
    };

    fn subtasks(descriptions: &[&str]) -> ArrayRef {
        let fields = Fields::from(vec![
            Field::new("start_ns", DataType::UInt64, false),
            Field::new("description", DataType::Utf8, false),
        ]);
        let starts = (0..descriptions.len() as u64).collect::<Vec<_>>();
        Arc::new(
            StructArray::try_new(
                fields,
                vec![
                    Arc::new(UInt64Array::from(starts)),
                    Arc::new(StringArray::from(descriptions.to_vec())),
                ],
                None,
            )
            .unwrap(),
        )
    }

    /// Track objects' pictures, the `n`th tracking `tracked[n]` objects, as
    /// a service declares them: every field it fills non-nullable.
    fn pictures(tracked: &[usize]) -> ArrayRef {
        let object = Fields::from(vec![
            Field::new("id", DataType::UInt32, false),
            Field::new("score", DataType::Float32, false),
            Field::new("x", DataType::Float32, false),
            Field::new("y", DataType::Float32, false),
            Field::new("width", DataType::Float32, false),
            Field::new("height", DataType::Float32, false),
        ]);
        let count = tracked.iter().sum::<usize>();
        let fraction = || -> ArrayRef { Arc::new(Float32Array::from(vec![0.5; count])) };
        let objects = StructArray::try_new(
            object.clone(),
            vec![
                Arc::new(UInt32Array::from_iter_values(0..count as u32)),
                fraction(),
                fraction(),
                fraction(),
                fraction(),
                fraction(),
            ],
            None,
        )
        .unwrap();
        let item = Arc::new(Field::new_list_field(DataType::Struct(object), false));
        let objects = ListArray::new(
            item,
            OffsetBuffer::from_lengths(tracked.iter().copied()),
            Arc::new(objects),
            None,
        );
        let times = (0..tracked.len() as u64).map(|picture| picture * 200_000_000);
        let fields = Fields::from(vec![
            Field::new("timestamp_ns", DataType::UInt64, false),
            Field::new("objects", objects.data_type().clone(), false),
        ]);
        Arc::new(
            StructArray::try_new(
                fields,
                vec![
                    Arc::new(UInt64Array::from_iter_values(times)),
                    Arc::new(objects),
                ],
                None,
            )
            .unwrap(),
        )
    }

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    #[test]
    fn an_answer_is_spelled_for_spiral_with_every_field_nullable() {
        let answer = EpisodeAnswer::listing(subtasks(&["reach", "grasp"]));
        let stored = StoredType::nullable(answer.0.data_type());
        assert_eq!(
            StoredType::sql(&stored).unwrap(),
            "STRUCT(start_ns uint64, description varchar)[]"
        );
        let DataType::List(item) = &stored else {
            panic!("a list")
        };
        let DataType::Struct(fields) = item.data_type() else {
            panic!("of structs")
        };
        assert!(fields.iter().all(|field| field.is_nullable()));
        assert!(StoredType::sql(&DataType::Date32).is_err());
    }

    #[test]
    fn answers_are_wrapped_so_a_failed_one_is_a_null_value_not_a_missing_row() {
        let column = AnswerColumn::of("breakdown_task", &["m"]).unwrap();
        let mut answers = ColumnAnswers::default();
        let answered = EpisodeAnswer::listing(subtasks(&["reach"]));
        let failed = EpisodeAnswer::failed(answered.0.data_type());
        assert!(failed.is_failed() && !answered.is_failed());
        answers.hold(episode(1), answered);
        answers.hold(episode(2), failed);
        let batch = answers.batch(&column, None).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(
            EpisodeUuids::read(batch.column(0).as_ref()).unwrap(),
            [episode(1), episode(2)]
        );
        let wrapped = batch
            .column(1)
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        assert_eq!(wrapped.null_count(), 0, "both episodes were asked");
        assert!(wrapped.column(0).is_valid(0) && wrapped.column(0).is_null(1));
        assert_eq!(
            StoredType::sql(batch.schema().field(1).data_type()).unwrap(),
            "STRUCT(value STRUCT(start_ns uint64, description varchar)[])"
        );
        // Written into a column that exists, answers take its type.
        let stored = batch.schema().field(1).data_type().clone();
        let mut again = ColumnAnswers::default();
        again.hold(episode(3), EpisodeAnswer::listing(subtasks(&["grasp"])));
        assert_eq!(
            again
                .batch(&column, Some(&stored))
                .unwrap()
                .schema()
                .field(1)
                .data_type(),
            &stored
        );
    }

    #[test]
    fn an_answer_nesting_lists_in_lists_is_spelled_and_cast_for_spiral() {
        let column = AnswerColumn::of("track_objects", &["m"]).unwrap();
        let mut answers = ColumnAnswers::default();
        let tracked = EpisodeAnswer::listing(pictures(&[2, 0, 1]));
        answers.hold(episode(1), EpisodeAnswer::failed(tracked.0.data_type()));
        answers.hold(episode(2), tracked);
        let batch = answers.batch(&column, None).unwrap();
        assert_eq!(
            StoredType::sql(batch.schema().field(1).data_type()).unwrap(),
            "STRUCT(value STRUCT(timestamp_ns uint64, objects STRUCT(id uint32, score real, x real, y real, width real, height real)[])[])"
        );
        let wrapped = batch
            .column(1)
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        let episodes = wrapped
            .column(0)
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        assert!(episodes.is_null(0) && episodes.is_valid(1));
        let pictures = episodes.value(1);
        let pictures = pictures.as_any().downcast_ref::<StructArray>().unwrap();
        let objects = pictures
            .column(1)
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let lengths = (0..objects.len()).map(|picture| objects.value_length(picture));
        assert_eq!(lengths.collect::<Vec<_>>(), [2, 0, 1]);
    }

    #[test]
    fn a_statement_answers_in_one_row() {
        let one = Schema::new(vec![
            EpisodeUuids::field("episode_id"),
            Field::new(ANSWER, DataType::Float64, true),
        ]);
        let row = |values: Vec<Option<f64>>| {
            RecordBatch::try_new(
                Arc::new(one.clone()),
                vec![
                    EpisodeUuids::array(&vec![episode(1); values.len()]).unwrap(),
                    Arc::new(Float64Array::from(values)),
                ],
            )
            .unwrap()
        };
        let answered = EpisodeAnswer::from_statement(&one, &[row(vec![Some(0.5)])]).unwrap();
        assert!(!answered.is_failed());
        assert!(
            EpisodeAnswer::from_statement(&one, &[])
                .unwrap()
                .is_failed()
        );
        assert!(EpisodeAnswer::from_statement(&one, &[row(vec![Some(0.5), None])]).is_err());
    }

    #[tokio::test]
    #[ignore = "requires SPIRAL_MCAP_TEST_PROJECT and SPIRAL_ACCESS_TOKEN (endpoints from SPIRAL__SERVER__URL and SPIRAL__SPFS__URL); creates and drops a probe dataset with its answers table"]
    async fn answers_live_with_their_dataset_and_take_the_columns_fills_write() {
        use crate::datasets::HostedProject;
        use spiral_robotics_common::datasets::{CameraVideo, DatasetSelection, EpisodeSelection};
        use spiral_robotics_common::relations::EpisodeQueryRequest;
        let (store, storage) = HostedProject::storage();
        let name = format!("probe_answers_{}", uuid::Uuid::new_v4().simple());
        let definition = StudioDataset::new(&name);
        let table = definition.answers().clone();
        let dataset = DatasetSelection::new(vec![definition.dataset().to_owned()]).unwrap();
        let rows = async |store: &crate::ProjectStore| -> Result<usize, String> {
            let batches = store
                .connect()
                .await?
                .read_batches(&format!("SELECT episode_id FROM {}", table.quoted()))
                .await?;
            Ok(batches.iter().map(RecordBatch::num_rows).sum())
        };
        // An assertion that fails still lets the probe be dropped.
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            // The first file makes the Dataset and its answers table.
            storage.record_probe_episode(&name, "one").await?;
            assert_eq!(rows(&store).await?, 1);
            storage.record_probe_episode(&name, "two").await?;
            assert_eq!(rows(&store).await?, 2, "an append adds its episode's row");
            let episodes = storage
                .queries()
                .episode_ids(&format!(
                    "SELECT id AS episode_id FROM spql.mcap.episodes({}) ORDER BY id",
                    SqlLiteral::text(definition.dataset())
                ))
                .await?;
            assert_eq!(episodes.len(), 2);
            let bound = |episode: &EpisodeId, statement: String| {
                EpisodeQueryRequest::new(
                    EpisodeSelection::new(dataset.clone(), episode.clone()),
                    statement,
                    1,
                )
            };

            // The progress service reads the bound episode's packets as recorded.
            let packets = CameraVideo::new(definition.dataset(), "/camera/top/video").packets();
            let (schema, batches) = storage
                .queries()
                .batches_for_episode(&bound(&episodes[0], packets))
                .await?;
            assert_eq!(
                schema.fields().iter().map(|f| f.name().as_str()).collect::<Vec<_>>(),
                ["payload", "decode_index", "pts_ns"]
            );
            let packets = arrow_select::concat::concat_batches(&schema, &batches)
                .map_err(|error| error.to_string())?;
            let order = arrow_cast::cast(packets.column(1), &DataType::Int64).unwrap();
            let order = order.as_any().downcast_ref::<Int64Array>().unwrap();
            assert_eq!(order.values().to_vec(), (0..8).collect::<Vec<i64>>());
            let times = arrow_cast::cast(packets.column(2), &DataType::Int64).unwrap();
            let times = times.as_any().downcast_ref::<Int64Array>().unwrap();
            assert_eq!(times.value(1) - times.value(0), 33_333_333);

            // A statement's answer, and one answered per position, fill columns.
            let answers = storage.dataset_answers(table.clone());
            let similarity = AnswerColumn::of("similarity", &["probe"]).unwrap();
            assert!(answers.filled(&similarity).await?.is_empty());
            let listed = format!(
                "WITH listed AS (SELECT e.id AS episode_id, [CAST(0.25 AS DOUBLE), CAST(0.5 AS DOUBLE)] AS scores FROM spql.mcap.episodes({}) e WHERE e.id = $1) \
                 SELECT l.episode_id, g._ AS position, spql.list.get(l.scores, g._) AS answer FROM listed l CROSS JOIN LATERAL spql.range(CAST(spql.list.len(l.scores) AS BIGINT)) g",
                SqlLiteral::text(definition.dataset())
            );
            let (schema, batches) = storage
                .queries()
                .batches_for_episode(&bound(&episodes[0], listed))
                .await?;
            let mut first = ColumnAnswers::default();
            first.hold(
                episodes[0].clone(),
                EpisodeAnswer::from_statement(&schema, &batches)?,
            );
            answers.write(&similarity, first).await?;
            assert_eq!(answers.filled(&similarity).await?, BTreeSet::from([episodes[0].clone()]));
            // A failed answer is written too, so it is never asked again.
            let mut second = ColumnAnswers::default();
            second.hold(
                episodes[1].clone(),
                EpisodeAnswer::failed(&DataType::List(Arc::new(Field::new_list_field(
                    DataType::Float64,
                    true,
                )))),
            );
            answers.write(&similarity, second).await?;
            assert_eq!(answers.filled(&similarity).await?.len(), 2);
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT a.episode_id, spql.list.get(a.{similarity}.value, CAST(1 AS BIGINT)) AS second FROM {} a ORDER BY a.episode_id",
                    table.quoted()
                ))
                .await?;
            let second = read[0].column(1).as_any().downcast_ref::<Float64Array>().unwrap();
            assert_eq!((second.value(0), second.is_null(1)), (0.5, true));

            // The progress service's scores, typed as its contract types them.
            let scores = AnswerColumn::of("score_progress", &["probe"]).unwrap();
            let frame = |index: u32| -> ArrayRef {
                Arc::new(
                    StructArray::try_new(
                        Fields::from(vec![
                            Field::new("frame_index", DataType::UInt32, false),
                            Field::new("timestamp_ns", DataType::UInt64, false),
                            Field::new("velocity", DataType::Float32, false),
                            Field::new("progress", DataType::Float32, true),
                        ]),
                        vec![
                            Arc::new(arrow_array::UInt32Array::from(vec![index])),
                            Arc::new(UInt64Array::from(vec![u64::from(index) * 33_333_333])),
                            Arc::new(arrow_array::Float32Array::from(vec![0.5])),
                            Arc::new(arrow_array::Float32Array::from(vec![None::<f32>])),
                        ],
                        None,
                    )
                    .unwrap(),
                )
            };
            let mut scored = ColumnAnswers::default();
            for (episode, index) in episodes.iter().zip([0, 1]) {
                scored.hold(episode.clone(), EpisodeAnswer::listing(frame(index)));
            }
            answers.write(&scores, scored).await?;
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT p.frame_index, p.timestamp_ns FROM {} a CROSS JOIN LATERAL UNNEST(a.{scores}.value) p",
                    table.quoted()
                ))
                .await?;
            assert_eq!(read.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
            assert_eq!(read[0].schema().field(0).data_type(), &DataType::UInt32);

            // Track objects' pictures, each nesting the objects it tracks.
            let tracks = AnswerColumn::of("track_objects", &["probe"]).unwrap();
            let mut tracked = ColumnAnswers::default();
            for (episode, objects) in episodes.iter().zip([2, 0]) {
                tracked.hold(episode.clone(), EpisodeAnswer::listing(pictures(&[objects, 1])));
            }
            answers.write(&tracks, tracked).await?;
            assert_eq!(answers.filled(&tracks).await?.len(), 2);
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT p.timestamp_ns FROM {} a CROSS JOIN LATERAL UNNEST(a.{tracks}.value) p",
                    table.quoted()
                ))
                .await?;
            assert_eq!(read.iter().map(RecordBatch::num_rows).sum::<usize>(), 4);

            // A later episode gets a row naming every column the table has.
            storage.record_probe_episode(&name, "three").await?;
            assert_eq!(rows(&store).await?, 3);
            assert_eq!(answers.filled(&similarity).await?.len(), 2);

            Ok::<_, String>(())
        }))
        .await;
        storage.drop_probe_dataset(&name).await.unwrap();
        let mut connection = store.connect().await.unwrap();
        assert!(
            !connection.table_exists(&table.name()).await.unwrap(),
            "retiring drops the answers table"
        );
        match outcome {
            Ok(outcome) => outcome.expect("answers lifecycle"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
