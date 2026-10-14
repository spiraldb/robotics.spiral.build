//! Typed message pages shared by schema inspection, numeric plots and recorded media.
use crate::activities::data::DatasetClient;
use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalPoint, TimelineSignalSeries,
};
use arrow_array::{
    Array, FixedSizeListArray, LargeListArray, ListArray, RecordBatch, StructArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use spiral_robotics_common::{
    datasets::{MessageCursor, MessageRead, ValueSchemaRead},
    relations::EpisodeQueryRequest,
};
use std::{collections::BTreeMap, io::Cursor, rc::Rc, sync::Arc};

/// Trace colours within one lane: the category tints, then the stage tints.
const TRACE_COLORS: [&str; 10] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#c98f00", "#d9679a", "#7560b8", "#3d8f86", "#b5701f",
    "#5b7fb8", "#65645f",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum FieldStep {
    Name(String),
    Items,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::activities) struct MessageField {
    steps: Vec<FieldStep>,
    projection: Vec<String>,
    field: Field,
}
impl MessageField {
    pub(in crate::activities) fn root(field: &Field) -> Self {
        Self {
            steps: vec![FieldStep::Name(field.name().clone())],
            projection: vec![field.name().clone()],
            field: field.clone(),
        }
    }
    /// Each read's `value` root, in order, from one episode query.
    pub(in crate::activities) async fn values_of(
        reads: &[MessageRead],
    ) -> Result<Vec<Self>, String> {
        let read = ValueSchemaRead::new(reads)?;
        let response = DatasetClient::current()
            .run_episode_arrow_query(&EpisodeQueryRequest::new(
                read.episode().clone(),
                read.sql(),
                1,
            ))
            .await
            .map_err(|error| error.to_string())?;
        let schema = arrow_ipc::reader::StreamReader::try_new(Cursor::new(response), None)
            .map_err(|error| error.to_string())?
            .schema();
        (0..reads.len())
            .map(|index| {
                let value = schema
                    .field_with_name(&ValueSchemaRead::column(index))
                    .map_err(|error| error.to_string())?;
                Ok(Self::root(
                    &Field::new("value", value.data_type().clone(), value.is_nullable())
                        .with_metadata(value.metadata().clone()),
                ))
            })
            .collect()
    }
    pub(in crate::activities) fn children(&self) -> Vec<Self> {
        match self.field.data_type() {
            DataType::Struct(fields) => fields
                .iter()
                .map(|field| {
                    let mut child = self.clone();
                    child.steps.push(FieldStep::Name(field.name().clone()));
                    if !self.steps.contains(&FieldStep::Items) {
                        child.projection.push(field.name().clone());
                    }
                    child.field = field.as_ref().clone();
                    child
                })
                .collect(),
            DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
                let mut child = self.clone();
                child.steps.push(FieldStep::Items);
                child.field = item.as_ref().clone();
                vec![child]
            }
            _ => vec![],
        }
    }
    pub(in crate::activities) fn label(&self) -> String {
        self.indexed_label(&[])
    }
    fn indexed_label(&self, indices: &[usize]) -> String {
        let mut label = String::new();
        let mut indices = indices.iter();
        for step in self.steps.iter().skip(1) {
            match step {
                FieldStep::Name(name)
                    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !name.is_empty() =>
                {
                    if !label.is_empty() {
                        label.push('.');
                    }
                    label.push_str(name);
                }
                FieldStep::Name(name) => label.push_str(&format!(
                    "[{}]",
                    serde_json::to_string(name).expect("field name")
                )),
                FieldStep::Items => match indices.next() {
                    Some(index) => label.push_str(&format!("[{index}]")),
                    None => label.push_str("[]"),
                },
            }
        }
        if label.is_empty() {
            "value".into()
        } else {
            label
        }
    }
    pub(in crate::activities) fn name(&self) -> &str {
        match self.steps.last() {
            Some(FieldStep::Name(name)) => name,
            _ => "[]",
        }
    }
    pub(in crate::activities) fn type_label(&self) -> String {
        let kind = match self.field.data_type() {
            DataType::Struct(_) => "struct".into(),
            DataType::List(_) | DataType::LargeList(_) => "list".into(),
            DataType::FixedSizeList(_, length) => format!("list[{length}]"),
            kind => kind.to_string(),
        };
        if self.field.is_nullable() {
            format!("{kind}?")
        } else {
            kind
        }
    }
    pub(in crate::activities) fn plottable(&self) -> bool {
        self.field.data_type().is_numeric() || self.field.data_type() == &DataType::Boolean
    }
    pub(in crate::activities) fn is_floating(&self) -> bool {
        self.field.data_type().is_floating()
    }
    /// An element of a recorded list, drawn as one trace per index.
    pub(in crate::activities) fn is_list_item(&self) -> bool {
        self.steps.contains(&FieldStep::Items)
    }
    pub(in crate::activities) fn projection(&self) -> Vec<String> {
        self.projection.clone()
    }
    fn cells(&self, array: &dyn Array, row: usize) -> Result<Vec<FieldCell>, String> {
        FieldCell::read(array, row, &self.steps[self.projection.len()..])
    }
}
struct FieldCell {
    indices: Vec<usize>,
    numeric: Option<f64>,
    display: String,
}
impl FieldCell {
    fn read(array: &dyn Array, row: usize, steps: &[FieldStep]) -> Result<Vec<Self>, String> {
        if array.is_null(row) {
            return Ok(if steps.contains(&FieldStep::Items) {
                vec![]
            } else {
                vec![Self {
                    indices: vec![],
                    numeric: None,
                    display: "null".into(),
                }]
            });
        }
        match steps.first() {
            Some(FieldStep::Name(name)) => {
                let structure = array
                    .as_any()
                    .downcast_ref::<StructArray>()
                    .ok_or("Message field is not a struct")?;
                Self::read(
                    structure
                        .column_by_name(name)
                        .ok_or("Message field disappeared")?
                        .as_ref(),
                    row,
                    &steps[1..],
                )
            }
            Some(FieldStep::Items) => {
                let values = if let Some(list) = array.as_any().downcast_ref::<ListArray>() {
                    list.value(row)
                } else if let Some(list) = array.as_any().downcast_ref::<LargeListArray>() {
                    list.value(row)
                } else if let Some(list) = array.as_any().downcast_ref::<FixedSizeListArray>() {
                    list.value(row)
                } else {
                    return Err("Message field is not a list".into());
                };
                if values.len() > 64 {
                    return Err("A plotted list exceeds 64 elements; choose a smaller field".into());
                }
                let mut cells = vec![];
                for index in 0..values.len() {
                    let mut children = Self::read(values.as_ref(), index, &steps[1..])?;
                    if children.is_empty() {
                        children.push(Self {
                            indices: vec![],
                            numeric: None,
                            display: "null".into(),
                        });
                    }
                    for child in &mut children {
                        child.indices.insert(0, index);
                    }
                    cells.extend(children);
                }
                Ok(cells)
            }
            None => {
                let display = arrow_cast::display::array_value_to_string(array, row)
                    .map_err(|e| e.to_string())?;
                let value = arrow_cast::cast(&array.slice(row, 1), &DataType::Float64)
                    .map_err(|e| e.to_string())?;
                let numeric = value
                    .as_any()
                    .downcast_ref::<arrow_array::Float64Array>()
                    .and_then(|values| (!values.is_null(0)).then(|| values.value(0)))
                    .filter(|v| v.is_finite());
                Ok(vec![Self {
                    indices: vec![],
                    numeric,
                    display,
                }])
            }
        }
    }
    fn label(&self, field: &MessageField) -> String {
        field.indexed_label(&self.indices)
    }
}
#[derive(Clone, PartialEq)]
pub(in crate::activities) struct MessageBuffer {
    schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
}
impl MessageBuffer {
    /// A schema read's empty buffer, or every message in a window, one keyset
    /// page per episode query.
    pub(in crate::activities) async fn load(mut request: MessageRead) -> Result<Rc<Self>, String> {
        let first = MessagePage::read(&request).await?;
        let mut loaded = Self {
            schema: first.schema,
            batches: first.batches,
        };
        let mut next = first.next;
        let mut bytes = first.bytes;
        while let Some(cursor) = next {
            let window = request.window().ok_or("Unexpected schema continuation")?;
            if window.after() == Some(&cursor) {
                return Err("Message cursor did not advance".into());
            }
            request = request
                .clone()
                .with_window(window.clone().with_cursor(Some(cursor)));
            let page = MessagePage::read(&request).await?;
            bytes += page.bytes;
            if bytes > 128 * 1024 * 1024 {
                return Err("Selected message window exceeds 128 MiB; narrow the window or select fewer fields".into());
            }
            if page.schema.fields() != loaded.schema.fields() {
                return Err("Message schema changed between pages".into());
            }
            loaded.batches.extend(page.batches);
            if loaded.rows() > 36_000 {
                return Err(
                    "Selected message window exceeds 36,000 samples; narrow the window".into(),
                );
            }
            next = page.next;
        }
        Ok(Rc::new(loaded))
    }
    pub(in crate::activities) fn value_field(&self) -> Result<MessageField, String> {
        self.schema
            .field_with_name("value")
            .map(MessageField::root)
            .map_err(|e| e.to_string())
    }
    fn rows(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }
    fn time(batch: &RecordBatch, row: usize) -> Result<u64, String> {
        let times = batch
            .column_by_name("log_time")
            .and_then(|a| a.as_any().downcast_ref::<UInt64Array>())
            .ok_or("Missing native log_time")?;
        if times.is_null(row) {
            return Err("Null native log_time".into());
        }
        Ok(times.value(row))
    }
    /// The log time of the first message, if there is one.
    pub(in crate::activities) fn first_time(&self) -> Option<u64> {
        let batch = self.batches.iter().find(|batch| batch.num_rows() > 0)?;
        Self::time(batch, 0).ok()
    }
    fn at(&self, absolute: u64) -> Result<Option<(&RecordBatch, usize)>, String> {
        for batch in self.batches.iter().rev() {
            let mut low = 0;
            let mut high = batch.num_rows();
            while low < high {
                let middle = (low + high) / 2;
                if Self::time(batch, middle)? <= absolute {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            if low > 0 {
                return Ok(Some((batch, low - 1)));
            }
        }
        Ok(None)
    }
    pub(in crate::activities) fn json_at(
        &self,
        absolute: u64,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some((batch, row)) = self.at(absolute)? else {
            return Ok(None);
        };
        let mut writer = arrow_json::ArrayWriter::new(Vec::new());
        writer
            .write(&batch.slice(row, 1))
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
        let mut rows: Vec<serde_json::Value> =
            serde_json::from_slice(&writer.into_inner()).map_err(|error| error.to_string())?;
        Ok(rows.pop())
    }
    pub(in crate::activities) fn values(
        &self,
        fields: &[MessageField],
        absolute: u64,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut result = BTreeMap::new();
        if let Some((batch, row)) = self.at(absolute)? {
            for (index, field) in fields.iter().enumerate() {
                for cell in field.cells(
                    batch
                        .column_by_name(&format!("field_{index}"))
                        .ok_or("Missing projected field")?
                        .as_ref(),
                    row,
                )? {
                    result.insert(cell.label(field), cell.display);
                }
            }
        }
        Ok(result)
    }
    /// One lane per selected field; a list field draws one series per index.
    pub(in crate::activities) fn lanes(
        &self,
        fields: &[MessageField],
        origin: u64,
    ) -> Result<Vec<TimelineSignalLane>, String> {
        let mut per_field = vec![Vec::<(String, Vec<TimelineSignalPoint>)>::new(); fields.len()];
        let mut times = vec![];
        for batch in &self.batches {
            for row in 0..batch.num_rows() {
                let seconds =
                    (i128::from(Self::time(batch, row)?) - i128::from(origin)) as f64 / 1e9;
                for (_, points) in per_field.iter_mut().flatten() {
                    points.push(TimelineSignalPoint::new(seconds, None));
                }
                for (index, field) in fields.iter().enumerate() {
                    for cell in field.cells(
                        batch
                            .column_by_name(&format!("field_{index}"))
                            .ok_or("Missing projected field")?
                            .as_ref(),
                        row,
                    )? {
                        let label = cell.label(field);
                        let series = &mut per_field[index];
                        let position = match series.iter().position(|(name, _)| *name == label) {
                            Some(position) => position,
                            None => {
                                series.push((
                                    label,
                                    times
                                        .iter()
                                        .copied()
                                        .chain([seconds])
                                        .map(|t| TimelineSignalPoint::new(t, None))
                                        .collect(),
                                ));
                                series.len() - 1
                            }
                        };
                        *series[position].1.last_mut().expect("current sample") =
                            TimelineSignalPoint::new(seconds, cell.numeric);
                    }
                }
                times.push(seconds);
                if per_field.iter().map(Vec::len).sum::<usize>() > 64 {
                    return Err("Plot exceeds 64 traces; select fewer fields".into());
                }
            }
        }
        Ok(fields
            .iter()
            .zip(per_field)
            .map(|(field, series)| {
                let label = field.label();
                TimelineSignalLane::new(&label, &label, TRACE_COLORS[0]).with_series(
                    series
                        .into_iter()
                        .enumerate()
                        .map(|(index, (name, points))| {
                            TimelineSignalSeries::new(name, points)
                                .with_color(TRACE_COLORS[index % TRACE_COLORS.len()])
                        })
                        .collect(),
                )
            })
            .collect())
    }
    pub(in crate::activities) fn batches(&self) -> &[RecordBatch] {
        &self.batches
    }
}

/// One episode query's worth of a read: at most its window's limit, and
/// where the next page starts when the query found one row more.
struct MessagePage {
    schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
    next: Option<MessageCursor>,
    bytes: usize,
}
impl MessagePage {
    async fn read(request: &MessageRead) -> Result<Self, String> {
        let response = DatasetClient::current()
            .run_episode_arrow_query(&EpisodeQueryRequest::new(
                request.episode(),
                request.sql()?,
                request.max_rows(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        let bytes = response.len();
        let reader = arrow_ipc::reader::StreamReader::try_new(Cursor::new(response), None)
            .map_err(|e| e.to_string())?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let limit = request.window().map_or(0, |window| window.limit());
        let mut page = Self::keep(schema, batches, limit)?;
        page.bytes = bytes;
        Ok(page)
    }
    /// The first `limit` rows; one more means the read continues after them.
    fn keep(
        schema: Arc<Schema>,
        mut batches: Vec<RecordBatch>,
        limit: usize,
    ) -> Result<Self, String> {
        let mut kept = 0;
        let mut more = false;
        for batch in &mut batches {
            let keep = batch.num_rows().min(limit - kept);
            if keep < batch.num_rows() {
                more = true;
                *batch = batch.slice(0, keep);
            }
            kept += keep;
        }
        batches.retain(|batch| batch.num_rows() > 0);
        let next = match batches.last().filter(|_| more) {
            Some(last) => Some(Self::cursor(last, last.num_rows() - 1)?),
            None => None,
        };
        Ok(Self {
            schema,
            batches,
            next,
            bytes: 0,
        })
    }
    /// The keyset position of one message.
    fn cursor(batch: &RecordBatch, row: usize) -> Result<MessageCursor, String> {
        let position = |name: &str| {
            let column = batch
                .column_by_name(name)
                .ok_or_else(|| format!("Missing native {name}"))?;
            let value = arrow_cast::cast(&column.slice(row, 1), &DataType::UInt64)
                .map_err(|e| e.to_string())?;
            value
                .as_any()
                .downcast_ref::<UInt64Array>()
                .filter(|values| !values.is_null(0))
                .map(|values| values.value(0))
                .ok_or_else(|| format!("Null native {name}"))
        };
        Ok(MessageCursor::new(
            position("log_time")?,
            position("ordinal")?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, Float64Array, types::Float64Type};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn unfamiliar_sensor_fields_preserve_precision_nulls_and_ragged_indices() {
        let temperature = MessageField::root(&Field::new(
            "value",
            DataType::Struct(
                vec![
                    Arc::new(Field::new("temperature", DataType::Float64, true)),
                    Arc::new(Field::new("counter", DataType::UInt64, false)),
                    Arc::new(Field::new(
                        "readings",
                        DataType::List(Arc::new(Field::new("item", DataType::Float64, true))),
                        true,
                    )),
                ]
                .into(),
            ),
            false,
        ));
        let children = temperature.children();
        let fields = vec![
            children[0].clone(),
            children[1].clone(),
            children[2].children()[0].clone(),
        ];
        assert_eq!(fields[2].label(), "readings[]");
        assert_eq!(fields[2].projection(), vec!["value", "readings"]);
        let origin = 1_700_000_000_000_000_000u64;
        let batch = RecordBatch::try_from_iter(vec![
            (
                "log_time",
                Arc::new(UInt64Array::from(vec![origin, origin + 1, origin + 1])) as ArrayRef,
            ),
            (
                "field_0",
                Arc::new(Float64Array::from(vec![Some(20.0), None, Some(22.0)])),
            ),
            ("field_1", Arc::new(UInt64Array::from(vec![u64::MAX; 3]))),
            (
                "field_2",
                Arc::new(ListArray::from_iter_primitive::<Float64Type, _, _>(vec![
                    Some(vec![Some(1.0), None]),
                    None,
                    Some(vec![Some(3.0)]),
                ])),
            ),
        ])
        .unwrap();
        let buffer = MessageBuffer {
            schema: batch.schema(),
            batches: vec![batch],
        };
        assert!(buffer.at(origin - 1).unwrap().is_none());
        assert!(buffer.json_at(origin - 1).unwrap().is_none());
        assert_eq!(
            buffer.json_at(origin + 1).unwrap().unwrap()["field_0"],
            22.0
        );
        assert!(buffer.json_at(origin).unwrap().unwrap()["field_2"][1].is_null());
        let values = buffer.values(&fields, origin + 1).unwrap();
        assert_eq!(values["counter"], u64::MAX.to_string());
        assert_eq!(values["temperature"], "22.0");
        assert_eq!(values["readings[0]"], "3.0");
        assert!(!values.contains_key("readings[1]"));
        let lanes = buffer.lanes(&fields, origin).unwrap();
        let labels = lanes
            .iter()
            .map(TimelineSignalLane::label)
            .collect::<Vec<_>>();
        assert_eq!(labels, ["temperature", "counter", "readings[]"]);
        let temperature = &lanes[0].series()[0];
        assert_eq!(temperature.points()[1].value(), None);
        assert_eq!(temperature.points()[2].seconds(), 1e-9);
        let readings = lanes[2].series();
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[1].name(), "readings[1]");
        assert_ne!(readings[0].color(), readings[1].color());
        assert!(
            readings[1]
                .points()
                .iter()
                .all(|point| point.value().is_none())
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn pages_keep_exact_values_and_continue_after_a_tied_timestamp() {
        let batch = RecordBatch::try_from_iter(vec![
            (
                "log_time",
                Arc::new(UInt64Array::from(vec![u64::MAX; 3])) as ArrayRef,
            ),
            ("ordinal", Arc::new(UInt64Array::from(vec![4, 5, 6]))),
            (
                "field_0",
                Arc::new(UInt64Array::from(vec![Some(u64::MAX), None, Some(1)])),
            ),
        ])
        .unwrap();
        let split = vec![batch.slice(0, 1), batch.slice(1, 2)];
        let page = MessagePage::keep(batch.schema(), split, 2).unwrap();
        let cursor = page.next.unwrap();
        assert_eq!(cursor.time(), u64::MAX);
        assert_eq!(cursor.ordinal(), 5);
        assert_eq!(
            page.batches
                .iter()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            2
        );
        let exact = page.batches[0]
            .column_by_name("field_0")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .value(0);
        assert_eq!(exact, u64::MAX);
        let last = MessagePage::keep(batch.schema(), vec![batch.slice(2, 1)], 2).unwrap();
        assert!(last.next.is_none());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn field_labels_do_not_confuse_literal_punctuation_with_paths() {
        let root = MessageField::root(&Field::new(
            "value",
            DataType::Struct(
                vec![Arc::new(Field::new("sensor.a[]", DataType::Float64, true))].into(),
            ),
            false,
        ));
        assert_eq!(root.children()[0].label(), "[\"sensor.a[]\"]");
    }
}
