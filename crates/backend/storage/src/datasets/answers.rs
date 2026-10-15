//! Each dataset's answer columns, a table each,
//! `datasets.<id>.answers.<column>`: `episode_id` and `value`, one row per
//! episode a fill run asked, NULL where the model could not answer; and the
//! rows fill runs append to them.
//!
//! A fill run makes a column's table, empty, when it starts filling the
//! column, its `value` typed for the call's answers with every nested field
//! nullable, so every answer of the call casts to it however its fields
//! declare their nullability. It then appends what it answered, a set at a
//! time. An episode the table already holds is not written again, so two
//! runs filling one column at once, such as two previews of one call, still
//! leave one row per episode. The tables go with their dataset.
use super::EpisodeUuids;
use crate::{ProjectConnection, ProjectStore, StorageManager};
use arrow_array::{Array, ArrayRef, ListArray, RecordBatch, make_array};
use arrow_buffer::OffsetBuffer;
use arrow_data::transform::MutableArrayData;
use arrow_schema::{DataType, Field, FieldRef, Schema, SchemaRef};
use spiral_robotics_common::curation::AnswerColumn;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetTable, EpisodeId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The column of an answer column's table holding each episode's answer.
const VALUE: &str = "value";
/// The column of a dataset statement's result holding its answer.
const ANSWER: &str = "answer";

impl StorageManager {
    /// What fill runs read and write of `dataset`'s answer column `column`.
    pub fn stored_answers(
        &self,
        dataset: &DatasetSelection,
        column: &AnswerColumn,
    ) -> StoredAnswers<'_> {
        StoredAnswers {
            store: self.project_store(),
            table: dataset.answers(column),
        }
    }

    /// The answer columns `dataset` holds a table for: each one a fill run
    /// has started filling.
    pub async fn answer_columns(
        &self,
        dataset: &DatasetSelection,
    ) -> Result<BTreeSet<AnswerColumn>, String> {
        self.project_store()
            .dataset_catalog()
            .datasets()
            .await?
            .answer_columns(dataset)
    }
}

/// One answer column's table, as fill runs read and write it.
pub struct StoredAnswers<'a> {
    store: &'a ProjectStore,
    table: DatasetTable,
}

/// What a column's table holds now: its schema, and each episode it has a
/// row for.
struct WrittenRows {
    schema: SchemaRef,
    episodes: BTreeSet<EpisodeId>,
}

impl StoredAnswers<'_> {
    /// The episodes the column holds an answer for, failed ones included;
    /// `None` before its table exists.
    pub async fn filled(&self) -> Result<Option<BTreeSet<EpisodeId>>, String> {
        let mut connection = self.store.connect().await?;
        let written = self.written(&mut connection).await?;
        Ok(written.map(|written| written.episodes))
    }

    /// Make the column's table, empty, for answers of `answer_type`.
    pub async fn create(&self, answer_type: &DataType) -> Result<(), String> {
        let schema = Arc::new(AnswerSchema::of(answer_type));
        self.store
            .connect_exclusive()
            .await?
            .create_or_append_batches(&self.table.target(), vec![RecordBatch::new_empty(schema)])
            .await
    }

    /// Append each of `answers` whose episode the table holds no row for.
    pub async fn write(&self, answers: ColumnAnswers) -> Result<(), String> {
        if answers.is_empty() {
            return Ok(());
        }
        let mut connection = self.store.connect_exclusive().await?;
        let written = self
            .written(&mut connection)
            .await?
            .ok_or_else(|| format!("{} does not exist.", self.table.target()))?;
        match answers.rows(&written)? {
            Some(rows) => {
                connection
                    .create_or_append_batches(&self.table.target(), vec![rows])
                    .await
            }
            None => Ok(()),
        }
    }

    /// What the table holds, read on `connection`; `None` before it exists.
    async fn written(
        &self,
        connection: &mut ProjectConnection,
    ) -> Result<Option<WrittenRows>, String> {
        let Some(schema) = connection.relation_schema(&self.table.target()).await? else {
            return Ok(None);
        };
        let batches = connection
            .read_batches(&format!("SELECT episode_id FROM {}", self.table))
            .await?;
        let mut episodes = BTreeSet::new();
        for batch in batches {
            let ids = batch.column_by_name("episode_id").ok_or("no episode_id")?;
            episodes.extend(EpisodeUuids::read(ids.as_ref())?);
        }
        Ok(Some(WrittenRows {
            schema: Arc::new(schema),
            episodes,
        }))
    }
}

/// One episode's answer to one call: one row of the answer's type, NULL
/// where the model could not answer.
#[derive(Clone, Debug)]
pub struct EpisodeAnswer(ArrayRef);

impl EpisodeAnswer {
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

    /// The answer copied into buffers of its own, so it keeps nothing of
    /// whatever it was read with and its size is its own: its strings and
    /// bytes as plain arrays, since a copied view array still points into
    /// every buffer of the read.
    fn alone(&self) -> Result<ArrayRef, String> {
        let read = self.0.to_data();
        let mut alone = MutableArrayData::new(vec![&read], false, 1);
        alone.extend(0, 0, 1);
        let alone = make_array(alone.freeze());
        let plain = Self::without_views(alone.data_type());
        arrow_cast::cast(&alone, &plain).map_err(|error| format!("copying an answer: {error}"))
    }

    /// `data_type` with every string or bytes view it nests a plain string
    /// or bytes.
    fn without_views(data_type: &DataType) -> DataType {
        let field = |field: &FieldRef| {
            let plain = Self::without_views(field.data_type());
            Arc::new(field.as_ref().clone().with_data_type(plain))
        };
        match data_type {
            DataType::Utf8View => DataType::Utf8,
            DataType::BinaryView => DataType::Binary,
            DataType::List(item) => DataType::List(field(item)),
            DataType::LargeList(item) => DataType::LargeList(field(item)),
            DataType::Struct(fields) => DataType::Struct(fields.iter().map(field).collect()),
            other => other.clone(),
        }
    }

    fn concatenated(arrays: &[ArrayRef]) -> Result<ArrayRef, String> {
        let arrays = arrays.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        arrow_select::concat::concat(&arrays).map_err(|error| error.to_string())
    }
}

/// What one statement over the whole dataset answered: one answer per
/// episode it measured, all of one type.
pub struct StatementAnswers {
    answer_type: DataType,
    answers: BTreeMap<EpisodeId, EpisodeAnswer>,
}

impl StatementAnswers {
    /// Each row's `episode_id` and `answer`. An episode answered twice is an
    /// error: a dataset statement measures each episode once.
    pub fn from_statement(schema: &Schema, batches: &[RecordBatch]) -> Result<Self, String> {
        let episode = schema
            .index_of("episode_id")
            .map_err(|_| Self::missing("episode_id"))?;
        let answer = schema.index_of(ANSWER).map_err(|_| Self::missing(ANSWER))?;
        let mut answers = BTreeMap::new();
        for batch in batches {
            let episodes = EpisodeUuids::read(batch.column(episode).as_ref())?;
            for (row, id) in episodes.into_iter().enumerate() {
                let answered = EpisodeAnswer(batch.column(answer).slice(row, 1));
                if answers.insert(id.clone(), answered).is_some() {
                    return Err(format!("A dataset statement answered episode {id} twice."));
                }
            }
        }
        Ok(Self {
            answer_type: schema.field(answer).data_type().clone(),
            answers,
        })
    }

    /// The type of the answers a statement of `schema` answers.
    pub fn answer_type(schema: &Schema) -> Result<DataType, String> {
        let answer = schema.field_with_name(ANSWER);
        Ok(answer
            .map_err(|_| Self::missing(ANSWER))?
            .data_type()
            .clone())
    }

    /// `episode`'s answer; a failed one when the statement measured nothing
    /// for it, so it is not asked again.
    pub fn answer(&mut self, episode: &EpisodeId) -> EpisodeAnswer {
        self.answers
            .remove(episode)
            .unwrap_or_else(|| EpisodeAnswer::failed(&self.answer_type))
    }

    fn missing(name: &str) -> String {
        format!("A dataset statement must answer `{name}`.")
    }
}

/// The answers one fill holds for one column until it writes them: each
/// episode it computed, beside its answer, and how many bytes they hold.
#[derive(Default)]
pub struct ColumnAnswers {
    episodes: Vec<EpisodeId>,
    answers: Vec<ArrayRef>,
    bytes: usize,
}

impl ColumnAnswers {
    /// The most answer bytes one write carries. Spiral refuses to change
    /// more than 16 MiB of a hosted table in one statement, so a fill writes
    /// what it holds each time it reaches this, well below that.
    const WRITE_BUDGET_BYTES: usize = 4 * 1024 * 1024;

    /// Hold `episode`'s answer, copied out of whatever it was read with, so
    /// it keeps no more than its own row and is counted once.
    pub fn hold(&mut self, episode: EpisodeId, answer: EpisodeAnswer) -> Result<(), String> {
        let alone = answer.alone()?;
        self.bytes += alone.get_array_memory_size();
        self.episodes.push(episode);
        self.answers.push(alone);
        Ok(())
    }
    /// Whether what it holds fills one write.
    pub fn is_full(&self) -> bool {
        self.bytes >= Self::WRITE_BUDGET_BYTES
    }
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }

    /// The table's rows for each answer of an episode `written` holds no
    /// row for, cast to the table's `value` type; none when it holds every
    /// one already.
    fn rows(self, written: &WrittenRows) -> Result<Option<RecordBatch>, String> {
        let (episodes, answers): (Vec<_>, Vec<_>) = self
            .episodes
            .into_iter()
            .zip(self.answers)
            .filter(|(episode, _)| !written.episodes.contains(episode))
            .unzip();
        if episodes.is_empty() {
            return Ok(None);
        }
        let value_type = written
            .schema
            .field_with_name(VALUE)
            .map_err(|_| "An answer column's table holds no value.".to_owned())?
            .data_type();
        let answers = answers
            .iter()
            .map(|answer| arrow_cast::cast(answer, value_type))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("An answer is not of its column's type: {error}"))?;
        let (ids, values) = (
            EpisodeUuids::array(&episodes)?,
            EpisodeAnswer::concatenated(&answers)?,
        );
        let columns = written
            .schema
            .fields()
            .iter()
            .map(|field| match field.name().as_str() {
                "episode_id" => Ok(ids.clone()),
                VALUE => Ok(values.clone()),
                other => Err(format!("An answer column's table holds {other}.")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        RecordBatch::try_new(written.schema.clone(), columns)
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

/// The schema of an answer column's table.
struct AnswerSchema;

impl AnswerSchema {
    /// The table holding answers of `answer_type`: `episode_id`, and the
    /// answer's type with every nested field nullable as `value`, NULL where
    /// the model could not answer.
    fn of(answer_type: &DataType) -> Schema {
        Schema::new(vec![
            EpisodeUuids::field("episode_id"),
            Field::new(VALUE, Self::nullable(answer_type), true),
        ])
    }

    /// `data_type` with every nested field nullable.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{
        Float32Array, Float64Array, Int64Array, StringArray, StringViewArray, StructArray,
        UInt32Array, UInt64Array,
    };
    use arrow_schema::Fields;

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

    /// The rows of a new table of `answer_type`'s answers, none written.
    fn new_table(answer_type: &DataType) -> WrittenRows {
        WrittenRows {
            schema: Arc::new(AnswerSchema::of(answer_type)),
            episodes: BTreeSet::new(),
        }
    }

    #[test]
    fn a_columns_table_stores_its_answers_with_every_nested_field_nullable() {
        let answer = EpisodeAnswer::listing(subtasks(&["reach", "grasp"]));
        let schema = AnswerSchema::of(answer.0.data_type());
        assert_eq!(
            schema
                .fields()
                .iter()
                .map(|f| f.name().as_str())
                .collect::<Vec<_>>(),
            ["episode_id", "value"]
        );
        assert!(!schema.field(0).is_nullable() && schema.field(1).is_nullable());
        let DataType::List(item) = schema.field(1).data_type() else {
            panic!("a list")
        };
        let DataType::Struct(fields) = item.data_type() else {
            panic!("of structs")
        };
        assert!(fields.iter().all(|field| field.is_nullable()));
    }

    #[test]
    fn a_failed_answer_is_a_row_whose_value_is_null_and_a_written_episode_is_skipped() {
        let mut answers = ColumnAnswers::default();
        let answered = EpisodeAnswer::listing(subtasks(&["reach"]));
        let failed = EpisodeAnswer::failed(answered.0.data_type());
        assert!(failed.is_failed() && !answered.is_failed());
        let table = new_table(answered.0.data_type());
        answers.hold(episode(1), answered).unwrap();
        answers.hold(episode(2), failed).unwrap();
        let rows = answers.rows(&table).unwrap().unwrap();
        assert_eq!(rows.schema(), table.schema);
        assert_eq!(
            EpisodeUuids::read(rows.column(0).as_ref()).unwrap(),
            [episode(1), episode(2)]
        );
        assert!(rows.column(1).is_valid(0) && rows.column(1).is_null(1));
        // An episode the table holds is not written again.
        let written = WrittenRows {
            episodes: BTreeSet::from([episode(1)]),
            ..new_table(&rows.schema().field(1).data_type().clone())
        };
        let mut again = ColumnAnswers::default();
        again
            .hold(episode(1), EpisodeAnswer::listing(subtasks(&["grasp"])))
            .unwrap();
        assert!(again.rows(&written).unwrap().is_none());
    }

    #[test]
    fn an_answer_nesting_lists_in_lists_is_cast_to_its_tables_type() {
        let mut answers = ColumnAnswers::default();
        let tracked = EpisodeAnswer::listing(pictures(&[2, 0, 1]));
        let table = new_table(tracked.0.data_type());
        answers
            .hold(episode(1), EpisodeAnswer::failed(tracked.0.data_type()))
            .unwrap();
        answers.hold(episode(2), tracked).unwrap();
        let rows = answers.rows(&table).unwrap().unwrap();
        let episodes = rows.column(1).as_any().downcast_ref::<ListArray>().unwrap();
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
    fn answers_fill_one_write_by_what_each_holds_alone() {
        // 64 episodes' scores of 256 KiB each, read as one 16 MiB column.
        let scores = DataType::List(Arc::new(Field::new_list_field(DataType::Float32, false)));
        let schema = Schema::new(vec![
            EpisodeUuids::field("episode_id"),
            Field::new(ANSWER, scores, true),
        ]);
        let frames = 64 * 1024;
        let read = ListArray::new(
            Arc::new(Field::new_list_field(DataType::Float32, false)),
            OffsetBuffer::from_lengths(vec![frames; 64]),
            Arc::new(Float32Array::from(vec![0.5; 64 * frames])),
            None,
        );
        let episodes = (0..64).map(episode).collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![EpisodeUuids::array(&episodes).unwrap(), Arc::new(read)],
        )
        .unwrap();
        let mut answered = StatementAnswers::from_statement(&schema, &[batch]).unwrap();
        let mut held = ColumnAnswers::default();
        for episode in &episodes[..15] {
            held.hold(episode.clone(), answered.answer(episode))
                .unwrap();
        }
        assert!(
            !held.is_full(),
            "each answer counts its own row, not the read's"
        );
        held.hold(episodes[15].clone(), answered.answer(&episodes[15]))
            .unwrap();
        assert!(held.is_full(), "16 answers of 256 KiB fill a write");
    }

    #[test]
    fn answers_holding_strings_fill_one_write_by_their_own_strings() {
        // 64 episodes' part names, 64 KiB each, read as one 4 MiB string
        // view column, as Spiral reads strings.
        let parts = 64;
        let name = "p".repeat(1024);
        let names =
            StringViewArray::from_iter_values(std::iter::repeat_n(name.as_str(), 64 * parts));
        let read = ListArray::new(
            Arc::new(Field::new_list_field(DataType::Utf8View, false)),
            OffsetBuffer::from_lengths(vec![parts; 64]),
            Arc::new(names),
            None,
        );
        let mut held = ColumnAnswers::default();
        for row in 0..16 {
            held.hold(
                episode(row),
                EpisodeAnswer(Arc::new(read.slice(row as usize, 1))),
            )
            .unwrap();
        }
        assert!(
            !held.is_full(),
            "each answer counts its own strings, not the read's"
        );
        // Held as plain strings, they are written as the table's views.
        let table = new_table(read.data_type());
        let rows = held.rows(&table).unwrap().unwrap();
        assert_eq!(rows.schema(), table.schema);
        let written = rows.column(1).as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!((written.len(), written.value(15).len()), (16, parts));
    }

    #[test]
    fn a_dataset_statement_answers_each_episode_it_measured_once() {
        let schema = Schema::new(vec![
            EpisodeUuids::field("episode_id"),
            Field::new(ANSWER, DataType::Float64, true),
        ]);
        let rows = |episodes: Vec<EpisodeId>, values: Vec<f64>| {
            RecordBatch::try_new(
                Arc::new(schema.clone()),
                vec![
                    EpisodeUuids::array(&episodes).unwrap(),
                    Arc::new(Float64Array::from(values)),
                ],
            )
            .unwrap()
        };
        let mut answered = StatementAnswers::from_statement(
            &schema,
            &[
                rows(vec![episode(1)], vec![0.5]),
                rows(vec![episode(2)], vec![0.25]),
            ],
        )
        .unwrap();
        let second = answered.answer(&episode(2));
        let second = second
            .value()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(second.value(0), 0.25);
        assert!(!answered.answer(&episode(1)).is_failed());
        // An episode it measured nothing for is answered as failed.
        assert!(answered.answer(&episode(3)).is_failed());
        assert!(
            StatementAnswers::from_statement(
                &schema,
                &[rows(vec![episode(1), episode(1)], vec![0.5, 0.5])]
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn a_column_is_made_empty_and_holds_one_row_per_episode_asked() {
        let journal = crate::datasets::TestJournal::start().await;
        let (store, _) = crate::db::UnquotedMemory::store_with_catalog(journal.endpoint()).await;
        let store = Arc::new(store);
        let storage = StorageManager::new(
            store.clone(),
            crate::StudioStore::in_memory().project(&store.connection_settings()),
        );
        let dataset = DatasetSelection::new("demo").unwrap();
        let column = AnswerColumn::of("break_down", &["m"]).unwrap();
        let answers = storage.stored_answers(&dataset, &column);
        assert_eq!(answers.filled().await.unwrap(), None, "no table yet");
        let answer_type = EpisodeAnswer::listing(subtasks(&["reach"]))
            .0
            .data_type()
            .clone();
        answers.create(&answer_type).await.unwrap();
        assert_eq!(answers.filled().await.unwrap(), Some(BTreeSet::new()));
        let mut first = ColumnAnswers::default();
        first
            .hold(
                episode(1),
                EpisodeAnswer::listing(subtasks(&["reach", "grasp"])),
            )
            .unwrap();
        first
            .hold(episode(2), EpisodeAnswer::failed(&answer_type))
            .unwrap();
        answers.write(first).await.unwrap();
        // Another run asked about episode 2 as well: only episode 3 is new.
        let mut second = ColumnAnswers::default();
        second
            .hold(episode(2), EpisodeAnswer::listing(subtasks(&["lift"])))
            .unwrap();
        second
            .hold(episode(3), EpisodeAnswer::listing(subtasks(&["place"])))
            .unwrap();
        answers.write(second).await.unwrap();
        let read = store
            .connect()
            .await
            .unwrap()
            .read_batches(&format!(
                "SELECT episode_id, CAST(spql.list.len(value) AS BIGINT) AS subtasks FROM {} ORDER BY episode_id",
                dataset.answers(&column)
            ))
            .await
            .unwrap();
        let read = arrow_select::concat::concat_batches(&read[0].schema(), &read).unwrap();
        assert_eq!(
            EpisodeUuids::read(read.column(0).as_ref()).unwrap(),
            [episode(1), episode(2), episode(3)]
        );
        let subtasks = read
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(
            subtasks.iter().collect::<Vec<_>>(),
            [Some(2), None, Some(1)],
            "the failed answer stays failed"
        );
        journal.name(&[dataset.episodes(), dataset.answers(&column)]);
        assert_eq!(
            storage.answer_columns(&dataset).await.unwrap(),
            BTreeSet::from([column])
        );
    }

    #[tokio::test]
    #[ignore = "requires SPIRAL_MCAP_TEST_PROJECT and SPIRAL_ACCESS_TOKEN (endpoints from SPIRAL__SERVER__URL and SPIRAL__SPFS__URL); creates and drops a probe dataset with its answer columns"]
    async fn answer_columns_live_with_their_dataset_and_take_what_fills_write() {
        use crate::datasets::HostedProject;
        use spiral_robotics_common::datasets::{CameraVideo, EpisodeSelection};
        use spiral_robotics_common::relations::EpisodeQueryRequest;
        let (store, storage) = HostedProject::storage();
        let name = format!("probe_answers_{}", uuid::Uuid::new_v4().simple());
        let dataset = DatasetSelection::new(name.clone()).unwrap();
        let similarity = AnswerColumn::of("similarity", &["probe"]).unwrap();
        // An assertion that fails still lets the probe be dropped.
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            storage.record_probe_episode(&name, "one").await?;
            storage.record_probe_episode(&name, "two").await?;
            assert!(
                storage.answer_columns(&dataset).await?.is_empty(),
                "adding episodes writes no answers"
            );
            let episodes = storage
                .queries()
                .episode_ids(&format!(
                    "SELECT id AS episode_id FROM {} ORDER BY id",
                    dataset.episodes()
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
            let camera = storage
                .inspect_dataset(dataset.clone())
                .await?
                .topics()
                .iter()
                .find(|topic| topic.name() == "/camera/top/video")
                .ok_or("the probe records its camera")?
                .revision()
                .clone();
            let packets = CameraVideo::new(&dataset, &camera).packets();
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

            // A judgment's list, one value per subtask, fills a column.
            let similarities = DataType::List(Arc::new(Field::new_list_field(DataType::Float64, true)));
            let answers = storage.stored_answers(&dataset, &similarity);
            assert_eq!(answers.filled().await?, None);
            answers.create(&similarities).await?;
            assert_eq!(answers.filled().await?, Some(BTreeSet::new()));
            let mut first = ColumnAnswers::default();
            first.hold(
                episodes[0].clone(),
                EpisodeAnswer::listing(Arc::new(Float64Array::from(vec![0.25, 0.5]))),
            )?;
            answers.write(first).await?;
            assert_eq!(answers.filled().await?, Some(BTreeSet::from([episodes[0].clone()])));
            // A failed answer is written too, so it is never asked again.
            let mut second = ColumnAnswers::default();
            second.hold(episodes[1].clone(), EpisodeAnswer::failed(&similarities))?;
            answers.write(second).await?;
            assert_eq!(answers.filled().await?.map(|filled| filled.len()), Some(2));
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT a.episode_id, spql.list.get(a.value, CAST(1 AS BIGINT)) AS second FROM {} a ORDER BY a.episode_id",
                    dataset.answers(&similarity)
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
            let scored_answers = storage.stored_answers(&dataset, &scores);
            scored_answers
                .create(EpisodeAnswer::listing(frame(0)).value().data_type())
                .await?;
            let mut scored = ColumnAnswers::default();
            for (episode, index) in episodes.iter().zip([0, 1]) {
                scored.hold(episode.clone(), EpisodeAnswer::listing(frame(index)))?;
            }
            scored_answers.write(scored).await?;
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT p.frame_index, p.timestamp_ns FROM {} a CROSS JOIN LATERAL UNNEST(a.value) p",
                    dataset.answers(&scores)
                ))
                .await?;
            assert_eq!(read.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
            assert_eq!(read[0].schema().field(0).data_type(), &DataType::UInt32);

            // Track objects' pictures, each nesting the objects it tracks.
            let tracks = AnswerColumn::of("track_objects", &["probe"]).unwrap();
            let tracked_answers = storage.stored_answers(&dataset, &tracks);
            tracked_answers
                .create(EpisodeAnswer::listing(pictures(&[1])).value().data_type())
                .await?;
            let mut tracked = ColumnAnswers::default();
            for (episode, objects) in episodes.iter().zip([2, 0]) {
                tracked.hold(episode.clone(), EpisodeAnswer::listing(pictures(&[objects, 1])))?;
            }
            tracked_answers.write(tracked).await?;
            assert_eq!(tracked_answers.filled().await?.map(|filled| filled.len()), Some(2));
            let read = store
                .connect()
                .await?
                .read_batches(&format!(
                    "SELECT p.timestamp_ns FROM {} a CROSS JOIN LATERAL UNNEST(a.value) p",
                    dataset.answers(&tracks)
                ))
                .await?;
            assert_eq!(read.iter().map(RecordBatch::num_rows).sum::<usize>(), 4);
            assert_eq!(
                storage.answer_columns(&dataset).await?,
                BTreeSet::from([similarity.clone(), scores, tracks])
            );

            // A later episode is not asked yet: it has no row.
            storage.record_probe_episode(&name, "three").await?;
            assert_eq!(answers.filled().await?.map(|filled| filled.len()), Some(2));

            Ok::<_, String>(())
        }))
        .await;
        storage.drop_probe_dataset(&name).await.unwrap();
        let mut connection = store.connect().await.unwrap();
        assert!(
            !connection
                .table_exists(&dataset.answers(&similarity).target())
                .await
                .unwrap(),
            "dropping the dataset drops its answer columns"
        );
        match outcome {
            Ok(outcome) => outcome.expect("answer columns lifecycle"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
