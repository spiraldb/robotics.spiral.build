//! One dataset at a glance: how much was recorded, and what the checks over
//! every episode's streams found.
use super::DatasetClient;
use super::dataset_checks::{CheckedEpisode, DatasetCheckReport};
use super::relations::CatalogRefresh;
use crate::activities::episode::EpisodeListing;
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, LongDurationPresentation,
    UtcTimestampPresentation, WallClockPresentation,
};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetFormat, DatasetSelection};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, Fact, FactLayout, FactList, InlineConfirmation, KindMark,
};
use std::collections::HashMap;

/// Totals over every episode, from one aggregate query.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct DatasetFacts {
    episodes: Option<u64>,
    duration_ns: Option<u64>,
    bytes: Option<u64>,
    first_start: Option<u64>,
    last_end: Option<u64>,
}

impl DatasetFacts {
    /// The totals `listing`'s format records: an MCAP dataset's size is
    /// the size of its files, and a LeRobot episode has no file of its own.
    fn sql(listing: &EpisodeListing) -> String {
        let bytes = match listing.format() {
            DatasetFormat::Mcap => " sum(source_bytes) AS bytes,",
            DatasetFormat::LeRobot => "",
        };
        format!(
            "SELECT count(*) AS episodes, sum(duration_ns) AS duration_ns,{bytes} \
             min(message_start_time) AS first_start, max(message_end_time) AS last_end \
             FROM {}",
            listing.dataset().episodes()
        )
    }
    /// The totals of `dataset`, read as its format records them.
    async fn read(listing: &EpisodeListing) -> Result<Self, String> {
        let snapshot = DatasetClient::current()
            .run_spiral_query(Self::sql(listing), 1)
            .await
            .map_err(|error| error.to_string())?;
        let row = snapshot
            .into_rows()
            .into_iter()
            .next()
            .ok_or("Dataset totals returned no row")?;
        serde_json::from_value::<Self>(serde_json::Value::Object(row))
            .map_err(|error| format!("Reading dataset totals: {error}"))
    }
    fn episodes_label(&self) -> Option<String> {
        self.episodes.map(GroupedCountPresentation::grouped_label)
    }
    fn recorded_label(&self) -> Option<String> {
        self.duration_ns
            .map(|duration| (duration as f64 / 1e9).long_duration_label())
    }
    fn average_seconds(&self) -> Option<f64> {
        let (total, episodes) = (self.duration_ns?, self.episodes.filter(|count| *count > 0)?);
        Some(total as f64 / episodes as f64 / 1e9)
    }
    fn average_label(&self) -> Option<String> {
        self.average_seconds().map(ClockPresentation::clock_label)
    }
    fn size_label(&self) -> Option<String> {
        self.bytes.map(ByteCountPresentation::byte_label)
    }
    /// The days the recordings span, when their clock is a calendar's; a
    /// simulator counting from zero has no span worth a cell.
    fn span_label(&self) -> Option<String> {
        let (first, last) = (self.first_start?, self.last_end?);
        if !first.has_wall_clock() {
            return None;
        }
        let (first, last) = (first.utc_date_label(), last.utc_date_label());
        Some(if first == last {
            first
        } else {
            format!("{first} → {last}")
        })
    }
}

/// The totals last read for each dataset, kept for the project: a view
/// that mounts again — the Dataset node after a step is
/// deleted, Data after Curation — paints the last answer at once while it
/// reads again, rather than a column of "Counting…".
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetOverviewMemory {
    facts: Signal<HashMap<DatasetSelection, DatasetFacts>>,
}

impl DatasetOverviewMemory {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            facts: Signal::new(HashMap::new()),
        })
    }
    fn current() -> Self {
        consume_context()
    }
    fn facts(self, dataset: &DatasetSelection) -> Option<DatasetFacts> {
        self.facts.read().get(dataset).cloned()
    }
    /// Keep `answer` as `dataset`'s, unless it already is.
    fn remember(mut self, dataset: &DatasetSelection, answer: &DatasetFacts) {
        if self.facts.peek().get(dataset) != Some(answer) {
            self.facts.write().insert(dataset.clone(), answer.clone());
        }
    }
}

/// A dataset's totals over its check report: the view Data's Overview tab
/// and Curation's Dataset node both show.
#[component]
pub(in crate::activities) fn DatasetOverview(
    listing: EpisodeListing,
    /// Opens an episode a finding names, where the finding is.
    on_open: EventHandler<CheckedEpisode>,
    /// Given, the head offers Delete, and this hears once the dataset is
    /// gone; Data's Overview gives it, Curation's Dataset node does not.
    on_deleted: Option<EventHandler<()>>,
) -> Element {
    // The totals belong to this dataset alone: choosing another one
    // re-queries, and each answer carries the dataset it was read for, since
    // the previous dataset's stays readable until the new one arrives.
    let read = use_resource(use_reactive((&listing,), |(listing,)| async move {
        let answer = DatasetFacts::read(&listing).await;
        (listing.dataset().clone(), answer)
    }));
    let dataset = listing.dataset().clone();
    // Once read, the totals are their dataset's last answer; until this
    // dataset's are read, its own last answer stands in for them.
    let memory = DatasetOverviewMemory::current();
    use_effect(move || {
        if let Some((read_for, Ok(answer))) = &*read.read() {
            memory.remember(read_for, answer);
        }
    });
    let facts = read
        .read()
        .as_ref()
        .filter(|(read_for, _)| *read_for == dataset)
        .map(|(_, answer)| answer.clone())
        .or_else(|| memory.facts(&dataset).map(Ok));
    let name = dataset.display_name();
    let label = dataset.label();
    // The dataset Delete is asking about: the Overview shows one dataset after
    // another, and choosing another one leaves the question unasked.
    let mut confirming = use_signal(|| None::<DatasetSelection>);
    let asking = confirming.read().as_ref() == Some(&dataset);
    let delete_id = format!("data-overview-delete-{}", dataset.id());
    let asked = dataset.clone();
    rsx! {
        section { class: "data-overview", "aria-label": "Dataset overview",
            div { class: "data-overview-body",
                header { class: "data-overview-head",
                    KindMark { class: "data-overview-mark", letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                    h2 { title: "{label}", "{name}" }
                    if on_deleted.is_some() && !asking {
                        Button { id: delete_id.clone(), purpose: ActionPurpose::Delete, variant: ButtonVariant::Link,
                            size: ButtonSize::Compact, class: "data-overview-head-action",
                            title: "Delete {label}",
                            onclick: move |_| confirming.set(Some(asked.clone())), "Delete" }
                    }
                }
                if let Some(on_deleted) = on_deleted.filter(|_| asking) {
                    DeleteConfirmation { dataset: dataset.clone(), return_focus: delete_id,
                        on_done: move |deleted: bool| {
                            confirming.set(None);
                            if deleted {
                                on_deleted.call(());
                            }
                        },
                    }
                }
                match &facts {
                    Some(Err(error)) => rsx! { p { class: "data-overview-alert", role: "alert", "{error}" } },
                    Some(Ok(facts)) => {
                        let shown = |label: Option<String>| label.unwrap_or_else(|| "—".to_owned());
                        rsx! {
                            FactList { class: "data-overview-facts", layout: FactLayout::Strip,
                                Fact { label: "Episodes", {shown(facts.episodes_label())} }
                                Fact { label: "Recorded", {shown(facts.recorded_label())} }
                                // A LeRobot dataset's episodes have no files of
                                // their own to size.
                                if let Some(size) = facts.size_label() {
                                    Fact { label: "Size", "{size}" }
                                }
                            }
                        }
                    },
                    None => rsx! { p { class: "data-overview-alert", role: "status", "Counting recordings…" } },
                }
                section { class: "data-overview-section", "aria-label": "Checks",
                    DatasetCheckReport { listing: listing.clone(), on_open }
                }
            }
        }
    }
}

/// Whether to delete the dataset, asked under its head: confirmed, every
/// table of it is dropped, and the catalog is read again. `on_done` says
/// whether it was deleted.
#[component]
fn DeleteConfirmation(
    dataset: DatasetSelection,
    return_focus: String,
    on_done: EventHandler<bool>,
) -> Element {
    let studio = StudioProjectState::current();
    let mut pending = use_signal(|| false);
    let name = dataset.display_name().to_owned();
    rsx! {
        InlineConfirmation {
            id: "data-overview-delete-confirmation",
            class: "data-overview-confirmation",
            copy: ConfirmationCopy::new(
                format!("Delete dataset {name}? Its episodes, recordings and answers will be permanently removed."),
                if pending() { "Deleting…" } else { "Delete dataset" },
                "Keep dataset",
            ),
            response: ConfirmationResponse::new(
                ActionPurpose::Delete,
                move |_| {
                    pending.set(true);
                    let dataset = dataset.clone();
                    spawn(async move {
                        let outcome = DatasetClient::current().delete_dataset(&dataset).await;
                        pending.set(false);
                        let deleted = outcome.is_ok();
                        match outcome {
                            Ok(()) => {
                                CatalogRefresh::request();
                                studio.report_success(format!("Deleted {}", dataset.display_name()));
                            }
                            Err(error) => studio.report(error.to_string()),
                        }
                        on_done.call(deleted);
                    });
                },
                move |_| on_done.call(false),
            ),
            dismissal: ConfirmationDismissal::new(return_focus).with_pending(pending()),
        }
    }
}

/// Every dataset in the project, before one is chosen: one ledger row each,
/// with the totals that tell them apart. Import stays in the rail; an empty
/// project offers it here too, as the one thing to do.
#[component]
pub(super) fn DatasetsAtAGlance(
    datasets: Vec<EpisodeListing>,
    on_select: EventHandler<EpisodeListing>,
    on_import: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "data-glance", "aria-label": "Datasets",
            header { class: "data-glance-head",
                h2 { "Datasets" }
                // An empty project says once what a dataset is; a listed one
                // shows it.
                if datasets.is_empty() {
                    p { "A dataset holds episodes: MCAP recordings, each file one episode, or a LeRobot dataset's." }
                }
            }
            if datasets.is_empty() {
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    onclick: move |_| on_import.call(()), "Import a dataset" }
            } else {
                DatasetLedger { datasets, on_select }
            }
        }
    }
}

/// The project's datasets as one ledger, a row each with the totals that tell
/// them apart, and a row pressed to choose it: how every surface that asks
/// for a dataset offers them — Data's home, and a curation that has none yet.
#[component]
pub(crate) fn DatasetLedger(
    datasets: Vec<EpisodeListing>,
    on_select: EventHandler<EpisodeListing>,
) -> Element {
    rsx! {
        div { class: "data-ledger-card",
            table { class: "data-ledger data-glance-ledger", "aria-label": "Datasets",
                thead { tr {
                    th { "Dataset" }
                    th { class: "is-number", "Episodes" }
                    th { class: "is-number", "Recorded" }
                    th { class: "is-number", "Average" }
                    th { class: "is-number", "Size" }
                    th { "Span" }
                } }
                tbody {
                    for (index, listing) in datasets.into_iter().enumerate() {
                        DatasetRow { key: "{listing.dataset().id()}", listing, order: index, on_select }
                    }
                }
            }
        }
    }
}

#[component]
fn DatasetRow(
    listing: EpisodeListing,
    order: usize,
    on_select: EventHandler<EpisodeListing>,
) -> Element {
    // The totals are read as the dataset's format records them.
    let facts = use_resource(use_reactive((&listing,), |(listing,)| async move {
        DatasetFacts::read(&listing).await.ok()
    }));
    let dataset = listing.dataset().clone();
    let memory = DatasetOverviewMemory::current();
    let remembered = dataset.clone();
    use_effect(move || {
        if let Some(Some(answer)) = &*facts.read() {
            memory.remember(&remembered, answer);
        }
    });
    let facts = facts
        .read()
        .clone()
        .flatten()
        .or_else(|| memory.facts(&dataset));
    let read = facts.is_some();
    let cell = |label: fn(&DatasetFacts) -> Option<String>| facts.as_ref().and_then(label);
    let chosen = listing.clone();
    let open = listing.clone();
    rsx! {
        tr { class: "data-glance-row", style: "--i: {order}", onclick: move |_| on_select.call(chosen.clone()),
            td {
                button { r#type: "button", class: "data-glance-name", title: "{dataset.label()}",
                    onclick: move |event: MouseEvent| { event.stop_propagation(); on_select.call(open.clone()); },
                    KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                    span { "{dataset.display_name()}" }
                }
            }
            LedgerCell { class: "is-number", reading: cell(DatasetFacts::episodes_label), read }
            LedgerCell { class: "is-number", reading: cell(DatasetFacts::recorded_label), read }
            LedgerCell { class: "is-number", reading: cell(DatasetFacts::average_label), read }
            LedgerCell { class: "is-number", reading: cell(DatasetFacts::size_label), read }
            LedgerCell { class: "data-glance-span", reading: cell(DatasetFacts::span_label), read }
        }
    }
}

/// One reading of a dataset's ledger row. Until the totals are `read` it is
/// an ellipsis, and a total the dataset has none of — a span without a wall
/// clock — is a dash; both rest in faint ink so the values stand out.
#[component]
fn LedgerCell(class: &'static str, reading: Option<String>, read: bool) -> Element {
    let quiet = reading.is_none();
    let shown = reading.unwrap_or_else(|| if read { "—" } else { "…" }.to_owned());
    rsx! {
        td { class, "data-quiet": quiet.then_some("true"), "{shown}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn dataset_facts_read_as_instrument_values() {
        let facts: DatasetFacts = serde_json::from_value(serde_json::json!({
            "episodes": 40, "duration_ns": 3_549_898_920_704_u64, "bytes": 3_509_376_768_u64,
            "first_start": 1_743_701_335_734_982_912_u64, "last_end": 1_776_208_497_899_157_760_u64
        }))
        .unwrap();
        assert_eq!(facts.episodes_label().as_deref(), Some("40"));
        assert_eq!(facts.recorded_label().as_deref(), Some("59 min 10 s"));
        assert_eq!(facts.size_label().as_deref(), Some("3.3 GiB"));
        assert_eq!(facts.average_label().as_deref(), Some("1:28.7"));
        assert_eq!(
            facts.span_label().as_deref(),
            Some("2025-04-03 → 2026-04-14")
        );
        assert!((facts.average_seconds().unwrap() - 88.747).abs() < 0.001);
        let sparse: DatasetFacts =
            serde_json::from_value(serde_json::json!({"episodes": 0})).unwrap();
        assert_eq!(sparse.span_label(), None);
        assert_eq!(sparse.average_label(), None);
        let simulated: DatasetFacts =
            serde_json::from_value(serde_json::json!({"first_start": 5, "last_end": 9})).unwrap();
        assert_eq!(simulated.span_label(), None);
        let mcap = EpisodeListing::mcap_fixture("bench");
        assert_eq!(
            DatasetFacts::sql(&mcap),
            "SELECT count(*) AS episodes, sum(duration_ns) AS duration_ns, sum(source_bytes) AS bytes, \
             min(message_start_time) AS first_start, max(message_end_time) AS last_end \
             FROM \"datasets.bench.episodes\""
        );
        let lerobot = EpisodeListing::new(
            DatasetSelection::new("so101").unwrap(),
            DatasetFormat::LeRobot,
        );
        assert!(!DatasetFacts::sql(&lerobot).contains("source"));
    }
}
