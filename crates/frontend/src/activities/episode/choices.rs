//! What the operator has chosen to show of an inspected episode's streams:
//! which fields are plotted, which cameras are open, and the schemas those
//! choices read.
use super::{
    messages::{MessageBuffer, MessageField},
    streams::{RecordedStream, StreamCatalog, StreamKind},
};
use crate::components::scoped_task::ScopedTask;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::MessageRead;
use std::collections::{BTreeMap, BTreeSet};

const MAX_PLOTTED_FIELDS_PER_STREAM: usize = 16;

#[derive(Clone, PartialEq)]
pub(super) enum StreamSchema {
    Loading,
    Ready(MessageField),
    Failed(String),
}

/// Numeric leaves, in schema order; lists count once and plot every index.
pub(super) trait PlottableLeaves {
    fn plottable_leaves(&self) -> Vec<MessageField>;
    /// What ticking a whole stream draws: measurements first. Floating-point
    /// and array fields are what a recording is about; integer and boolean
    /// scalars (counters, stamps, flags) stay one click away unless the
    /// stream has nothing else. This is a type rule, not a name rule.
    fn default_plot_leaves(&self) -> Vec<MessageField>;
}

impl PlottableLeaves for MessageField {
    fn default_plot_leaves(&self) -> Vec<MessageField> {
        let leaves = self.plottable_leaves();
        let mut preferred = leaves
            .iter()
            .filter(|leaf| leaf.is_floating() || leaf.is_list_item())
            .cloned()
            .collect::<Vec<_>>();
        if preferred.is_empty() {
            preferred = leaves;
        }
        preferred.truncate(MAX_PLOTTED_FIELDS_PER_STREAM);
        preferred
    }
    fn plottable_leaves(&self) -> Vec<MessageField> {
        let children = self.children();
        if children.is_empty() {
            return if self.plottable() {
                vec![self.clone()]
            } else {
                vec![]
            };
        }
        children
            .iter()
            .flat_map(PlottableLeaves::plottable_leaves)
            .collect()
    }
}

/// Which fields of which streams are drawn, and which traces are hidden.
#[derive(Clone, PartialEq, Default)]
struct PlotSelection {
    fields: BTreeMap<String, Vec<MessageField>>,
    hidden_traces: BTreeSet<String>,
    requested: BTreeMap<String, bool>,
}

impl PlotSelection {
    /// The plot an episode opens with when the operator has chosen nothing.
    /// Never applied after the operator has made a plot choice, and never
    /// for a stream without numeric fields. Reports whether it was adopted.
    fn adopt_default(&mut self, key: &str, fields: Vec<MessageField>) -> bool {
        if !self.requested.is_empty() || fields.is_empty() {
            return false;
        }
        self.fields.insert(key.to_owned(), fields);
        true
    }

    fn trace_key(stream: &str, trace: &str) -> String {
        format!("{stream}\u{1f}{trace}")
    }
}

/// Whether none, some, or every plottable field of a stream is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PlotState {
    None,
    Some,
    All,
}

/// Everything the operator has chosen to show for the inspected episode.
///
/// Reads spawned here belong to the inspection's scope, so closing a panel
/// does not abandon a schema load another panel is waiting on.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct StreamChoices {
    scope: ScopeId,
    generation: Signal<u64>,
    plots: Signal<PlotSelection>,
    schemas: Signal<BTreeMap<String, StreamSchema>>,
    /// Cameras the operator closed; every other camera is shown.
    hidden_cameras: Signal<BTreeSet<String>>,
    /// How many times the operator asked for something to be plotted, so a
    /// Plots panel waiting behind another tab can come forward.
    plot_asks: Signal<usize>,
}

impl StreamChoices {
    pub(super) fn use_choices() -> Self {
        use_hook(|| Self {
            scope: dioxus::dioxus_core::current_scope_id(),
            generation: Signal::new(0),
            plots: Signal::new(PlotSelection::default()),
            schemas: Signal::new(BTreeMap::new()),
            hidden_cameras: Signal::new(BTreeSet::new()),
            plot_asks: Signal::new(0),
        })
    }

    /// The choices an episode opens with: every camera shown and the busiest
    /// measurement plotted.
    pub(super) fn open(&mut self, catalog: &StreamCatalog) {
        self.reset();
        self.plot_default_stream(catalog);
    }

    /// Forget whatever was chosen before; every camera starts visible.
    fn reset(&mut self) {
        let generation = self.generation.peek().wrapping_add(1);
        self.generation.set(generation);
        self.plots.set(PlotSelection::default());
        self.schemas.set(BTreeMap::new());
        self.hidden_cameras.set(BTreeSet::new());
    }

    fn spawn<F: Future<Output = ()> + 'static>(&self, future: F) {
        ScopedTask::spawn(self.scope, future);
    }

    pub(super) fn plotted_fields(&self, key: &str) -> Vec<MessageField> {
        self.plots
            .read()
            .fields
            .get(key)
            .cloned()
            .unwrap_or_default()
    }
    /// Streams with at least one drawn field, in topic order.
    pub(super) fn plotted_streams(&self) -> Vec<String> {
        self.plots
            .read()
            .fields
            .iter()
            .filter(|(_, fields)| !fields.is_empty())
            .map(|(key, _)| key.clone())
            .collect()
    }
    pub(super) fn is_field_plotted(&self, key: &str, field: &MessageField) -> bool {
        self.plots
            .read()
            .fields
            .get(key)
            .is_some_and(|fields| fields.contains(field))
    }
    pub(super) fn toggle_field(&mut self, key: &str, field: MessageField) {
        let mut plots = self.plots.write();
        plots.requested.insert(key.to_owned(), false);
        let fields = plots.fields.entry(key.to_owned()).or_default();
        if let Some(index) = fields.iter().position(|existing| existing == &field) {
            fields.remove(index);
        } else if fields.len() < MAX_PLOTTED_FIELDS_PER_STREAM {
            fields.push(field);
            *self.plot_asks.write() += 1;
        }
    }
    /// How many times the operator has asked to plot something.
    pub(super) fn plot_asks(&self) -> usize {
        *self.plot_asks.read()
    }
    pub(super) fn plot_state(&self, key: &str) -> PlotState {
        let plotted = self.plotted_fields(key);
        if plotted.is_empty() {
            return PlotState::None;
        }
        let Some(StreamSchema::Ready(root)) = self.schema(key) else {
            return PlotState::Some;
        };
        let default = root.default_plot_leaves();
        if !default.is_empty() && default.iter().all(|leaf| plotted.contains(leaf)) {
            PlotState::All
        } else {
            PlotState::Some
        }
    }
    /// Draw every numeric field of a stream, loading its schema first if needed.
    pub(super) fn set_stream_plotted(&mut self, stream: &RecordedStream, plotted: bool) {
        let key = stream.key().to_owned();
        self.plots.write().requested.insert(key.clone(), plotted);
        if !plotted {
            self.plots.write().fields.remove(&key);
            return;
        }
        *self.plot_asks.write() += 1;
        let mut choices = *self;
        let generation = *self.generation.peek();
        let read = stream.read().clone();
        self.spawn(async move {
            let Some(root) = choices.schema_for(&key, read).await else {
                return;
            };
            if *choices.generation.peek() != generation {
                return;
            }
            let mut plots = choices.plots.write();
            if plots.requested.get(&key) == Some(&true) {
                plots.fields.insert(key, root.default_plot_leaves());
            }
        });
    }
    /// Reads every data stream's schema in one query and plots the busiest
    /// stream with numeric fields, so Plots is never empty on open. The other
    /// schemas serve the fold and the disabled checks.
    fn plot_default_stream(&mut self, catalog: &StreamCatalog) {
        let ranked = catalog.data_by_traffic();
        if ranked.is_empty() {
            return;
        }
        {
            let mut schemas = self.schemas.write();
            for stream in &ranked {
                schemas.insert(stream.key().to_owned(), StreamSchema::Loading);
            }
        }
        let generation = *self.generation.peek();
        let mut choices = *self;
        self.spawn(async move {
            let reads = ranked
                .iter()
                .map(|stream| stream.read().clone())
                .collect::<Vec<_>>();
            let answer = MessageField::values_of(&reads).await;
            if *choices.generation.peek() != generation {
                return;
            }
            let mut schemas = choices.schemas.write();
            let roots = match answer {
                Ok(roots) => roots,
                Err(error) => {
                    for stream in &ranked {
                        schemas
                            .insert(stream.key().to_owned(), StreamSchema::Failed(error.clone()));
                    }
                    return;
                }
            };
            if let Some((stream, leaves)) = ranked
                .iter()
                .zip(&roots)
                .map(|(stream, root)| (stream, root.default_plot_leaves()))
                .find(|(_, leaves)| !leaves.is_empty())
            {
                choices.plots.write().adopt_default(stream.key(), leaves);
            }
            for (stream, root) in ranked.iter().zip(roots) {
                schemas.insert(stream.key().to_owned(), StreamSchema::Ready(root));
            }
        });
    }
    /// A stream with nothing to plot: at most one message, or a schema known
    /// to have no numeric fields. Cameras are never quiet. This is a count
    /// rule and a type rule, never a name rule.
    pub(super) fn stream_is_quiet(&self, stream: &RecordedStream) -> bool {
        if stream.kind() == StreamKind::Video {
            return false;
        }
        stream.samples().is_some_and(|samples| samples <= 1)
            || matches!(
                self.schemas.read().get(stream.key()),
                Some(StreamSchema::Ready(root)) if root.plottable_leaves().is_empty()
            )
    }

    pub(super) fn trace_hidden(&self, key: &str, trace: &str) -> bool {
        self.plots
            .read()
            .hidden_traces
            .contains(&PlotSelection::trace_key(key, trace))
    }
    /// The traces of stream `key` the operator hid.
    pub(super) fn hidden_traces(&self, key: &str) -> Vec<String> {
        let stream = PlotSelection::trace_key(key, "");
        self.plots
            .read()
            .hidden_traces
            .iter()
            .filter_map(|trace| trace.strip_prefix(&stream))
            .map(str::to_owned)
            .collect()
    }
    pub(super) fn toggle_trace(&mut self, key: &str, trace: &str) {
        let trace_key = PlotSelection::trace_key(key, trace);
        let mut plots = self.plots.write();
        plots.requested.insert(key.to_owned(), false);
        if !plots.hidden_traces.remove(&trace_key) {
            plots.hidden_traces.insert(trace_key);
        }
    }

    pub(super) fn schema(&self, key: &str) -> Option<StreamSchema> {
        self.schemas.read().get(key).cloned()
    }
    /// Start reading a stream's schema once; later calls observe the result.
    pub(super) fn ensure_schema(&mut self, stream: &RecordedStream) {
        if self.schemas.peek().contains_key(stream.key()) {
            return;
        }
        let key = stream.key().to_owned();
        let read = stream.read().clone();
        let mut choices = *self;
        self.spawn(async move {
            choices.schema_for(&key, read).await;
        });
    }
    async fn schema_for(&mut self, key: &str, read: MessageRead) -> Option<MessageField> {
        if let Some(StreamSchema::Ready(root)) = self.schemas.peek().get(key) {
            return Some(root.clone());
        }
        let generation = *self.generation.peek();
        self.schemas
            .write()
            .insert(key.to_owned(), StreamSchema::Loading);
        let result = match MessageBuffer::load(read).await {
            Ok(buffer) => buffer.value_field(),
            Err(error) => Err(error),
        };
        if *self.generation.peek() != generation {
            return None;
        }
        let state = match &result {
            Ok(root) => StreamSchema::Ready(root.clone()),
            Err(error) => StreamSchema::Failed(error.clone()),
        };
        self.schemas.write().insert(key.to_owned(), state);
        result.ok()
    }

    pub(super) fn camera_open(&self, key: &str) -> bool {
        !self.hidden_cameras.read().contains(key)
    }
    pub(super) fn set_camera(&mut self, key: &str, open: bool) {
        let mut hidden = self.hidden_cameras.write();
        if open {
            hidden.remove(key);
        } else {
            hidden.insert(key.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Fields};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_default_plot_is_adopted_once_and_never_over_an_operator_choice() {
        let numeric = MessageField::root(&Field::new("value", DataType::Float64, false));
        let text = MessageField::root(&Field::new("value", DataType::Utf8, false));
        let mut selection = PlotSelection::default();
        // A stream without numeric fields is never the default, however busy.
        assert!(!selection.adopt_default("text", text.default_plot_leaves()));
        assert!(selection.adopt_default("dense", numeric.default_plot_leaves()));
        assert_eq!(
            selection
                .fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["dense"]
        );
        // An explicit uncheck wins even if the automatic schema read completes later.
        selection.requested.insert("dense".into(), false);
        selection.fields.clear();
        assert!(!selection.adopt_default("dense", numeric.default_plot_leaves()));
        assert!(selection.fields.is_empty());
        // A fresh episode gets a fresh default; the previous user's choice is not global.
        let mut next_episode = PlotSelection::default();
        assert!(next_episode.adopt_default("dense", numeric.default_plot_leaves()));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn plottable_leaves_walk_structs_and_count_lists_once() {
        let root = Field::new(
            "value",
            DataType::Struct(Fields::from(vec![
                Field::new(
                    "timestamp",
                    DataType::Struct(Fields::from(vec![
                        Field::new("seconds", DataType::Int64, false),
                        Field::new("nanos", DataType::Int32, false),
                    ])),
                    false,
                ),
                Field::new(
                    "position",
                    DataType::List(std::sync::Arc::new(Field::new(
                        "item",
                        DataType::Float64,
                        false,
                    ))),
                    false,
                ),
                Field::new("label", DataType::Utf8, true),
            ])),
            false,
        );
        let field = MessageField::root(&root);
        let labels = field
            .plottable_leaves()
            .iter()
            .map(MessageField::label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["timestamp.seconds", "timestamp.nanos", "position[]"]
        );
        let defaults = field
            .default_plot_leaves()
            .iter()
            .map(MessageField::label)
            .collect::<Vec<_>>();
        assert_eq!(defaults, ["position[]"]);
        let counters = MessageField::root(&Field::new(
            "value",
            DataType::Struct(Fields::from(vec![Field::new(
                "count",
                DataType::UInt32,
                false,
            )])),
            false,
        ));
        assert_eq!(counters.default_plot_leaves().len(), 1);
    }
}
