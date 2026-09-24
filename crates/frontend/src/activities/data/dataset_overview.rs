//! One dataset at a glance: how much was recorded, how long its episodes run,
//! and which streams exist.
use super::DatasetClient;
use crate::components::icons::{CameraStreamIcon, SignalStreamIcon};
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, LongDurationPresentation,
    UtcTimestampPresentation, WallClockPresentation,
};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection, DatasetTopic};
use spiral_robotics_common::sql::SqlLiteral;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Fact, FactLayout, FactList, KindMark,
};

/// Totals over every member, from one aggregate query.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct DatasetFacts {
    episodes: Option<u64>,
    duration_ns: Option<u64>,
    shortest_ns: Option<u64>,
    longest_ns: Option<u64>,
    bytes: Option<u64>,
    first_start: Option<u64>,
    last_end: Option<u64>,
}

impl DatasetFacts {
    fn sql(dataset: &DatasetSelection) -> String {
        format!(
            "SELECT count(*) AS episodes, sum(duration_ns) AS duration_ns, min(duration_ns) AS shortest_ns, \
             max(duration_ns) AS longest_ns, sum(source.size) AS bytes, \
             min(message_start_time) AS first_start, max(message_end_time) AS last_end \
             FROM spql.mcap.episodes({})",
            SqlLiteral::text(&dataset.sql_name())
        )
    }
    fn episodes_label(&self) -> String {
        self.episodes
            .map_or_else(|| "—".into(), GroupedCountPresentation::grouped_label)
    }
    fn recorded_label(&self) -> String {
        self.duration_ns.map_or_else(
            || "—".into(),
            |duration| (duration as f64 / 1e9).long_duration_label(),
        )
    }
    fn average_seconds(&self) -> Option<f64> {
        let (total, episodes) = (self.duration_ns?, self.episodes.filter(|count| *count > 0)?);
        Some(total as f64 / episodes as f64 / 1e9)
    }
    /// The shortest and longest episode, when both are known.
    fn length_range(&self) -> Option<(u64, u64)> {
        Some((self.shortest_ns?, self.longest_ns?))
    }
    fn size_label(&self) -> String {
        self.bytes
            .map_or_else(|| "—".into(), ByteCountPresentation::byte_label)
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

/// How many episodes run how long, counted by the engine in equal slices
/// between the shortest and the longest, so the shape is exact at any size.
#[derive(Clone, Debug, PartialEq)]
struct LengthDistribution {
    shortest_ns: u64,
    width_ns: u64,
    counts: Vec<u64>,
    average_seconds: Option<f64>,
}

impl LengthDistribution {
    const BINS: u64 = 36;

    fn width_ns((shortest, longest): (u64, u64)) -> u64 {
        (longest.saturating_sub(shortest) / Self::BINS).max(1)
    }
    fn sql(dataset: &DatasetSelection, range: (u64, u64)) -> String {
        let bin = format!(
            "floor((duration_ns - {}) / {})",
            range.0,
            Self::width_ns(range)
        );
        format!(
            "SELECT {bin} AS bin, count(*) AS episodes FROM spql.mcap.episodes({}) \
             WHERE duration_ns IS NOT NULL GROUP BY {bin} ORDER BY bin",
            SqlLiteral::text(&dataset.sql_name())
        )
    }
    /// Read the grouped rows. The longest episode lands one slice past the
    /// last and joins it.
    fn new(range: (u64, u64), rows: &[(f64, u64)], average_seconds: Option<f64>) -> Self {
        let mut counts = vec![0; Self::BINS as usize];
        for (bin, episodes) in rows {
            let index = (bin.max(0.0) as usize).min(counts.len() - 1);
            counts[index] += episodes;
        }
        Self {
            shortest_ns: range.0,
            width_ns: Self::width_ns(range),
            counts,
            average_seconds,
        }
    }
    fn edge_seconds(&self, index: usize) -> f64 {
        (self.shortest_ns + self.width_ns * index as u64) as f64 / 1e9
    }
    fn tallest(&self) -> u64 {
        self.counts.iter().copied().max().unwrap_or(1).max(1)
    }
    /// Where a length falls across the plot, 0–100.
    fn percent(&self, seconds: f64) -> f64 {
        let (shortest, longest) = (self.edge_seconds(0), self.edge_seconds(self.counts.len()));
        if longest > shortest {
            ((seconds - shortest) / (longest - shortest) * 100.0).clamp(0.0, 100.0)
        } else {
            50.0
        }
    }
}

#[component]
pub(crate) fn DatasetOverview(
    dataset: DatasetSelection,
    inspection: Option<Result<DatasetInspection, String>>,
) -> Element {
    // Both answers belong to this dataset alone: choosing another one
    // re-queries instead of keeping the first answer under a new name.
    let facts = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        let snapshot = DatasetClient::current()
            .run_spiral_query(DatasetFacts::sql(&dataset), 1)
            .await
            .map_err(|error| error.to_string())?;
        let row = snapshot
            .into_rows()
            .into_iter()
            .next()
            .ok_or("Dataset totals returned no row")?;
        serde_json::from_value::<DatasetFacts>(serde_json::Value::Object(row))
            .map_err(|error| format!("Reading dataset totals: {error}"))
    }));
    // The slices follow from the totals: the shortest and longest episode
    // bound them, and the engine counts each one.
    let distribution = use_resource(use_reactive((&dataset,), move |(dataset,)| async move {
        let totals = match &*facts.read() {
            Some(Ok(totals)) => totals.clone(),
            _ => return None,
        };
        let range = totals.length_range()?;
        let result = DatasetClient::current()
            .run_spiral_query(
                LengthDistribution::sql(&dataset, range),
                LengthDistribution::BINS as usize + 1,
            )
            .await
            .map_err(|error| error.to_string())
            .map(|snapshot| {
                let rows = snapshot
                    .into_rows()
                    .into_iter()
                    .filter_map(|row| {
                        Some((row.get("bin")?.as_f64()?, row.get("episodes")?.as_u64()?))
                    })
                    .collect::<Vec<_>>();
                LengthDistribution::new(range, &rows, totals.average_seconds())
            });
        Some(result)
    }));
    let name = dataset.display_name();
    let sql_name = dataset.sql_name();
    let topics = inspection
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|detail| detail.topics().to_vec())
        .unwrap_or_default();
    let cameras = topics.iter().filter(|topic| topic.is_video()).count();
    rsx! {
        section { class: "data-overview", "aria-label": "Dataset overview",
            div { class: "data-overview-body",
            header { class: "data-overview-head",
                KindMark { class: "data-overview-mark", letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                h2 { title: "{sql_name}", "{name}" }
            }
            match &*facts.read() {
                Some(Err(error)) => rsx! { p { class: "data-overview-alert", role: "alert", "{error}" } },
                Some(Ok(facts)) => rsx! {
                    FactList { class: "data-overview-facts", layout: FactLayout::Strip,
                        Fact { label: "Episodes", "{facts.episodes_label()}" }
                        Fact { label: "Recorded", "{facts.recorded_label()}" }
                        Fact { label: "Average length", {facts.average_seconds().map_or_else(|| "—".into(), ClockPresentation::clock_label)} }
                        Fact { label: "Size", "{facts.size_label()}" }
                        if let Some(span) = facts.span_label() {
                            Fact { label: "Span", "{span}" }
                        }
                        Fact { label: "Streams",
                            if topics.is_empty() { "—" } else { "{topics.len()} · {cameras} cameras" }
                        }
                    }
                },
                None => rsx! { p { class: "data-overview-alert", role: "status", "Counting recordings…" } },
            }
            section { class: "data-overview-section", "aria-label": "Episode lengths",
                h3 { "Episode lengths" }
                match (&*facts.read(), &*distribution.read()) {
                    (Some(Ok(facts)), _) if facts.length_range().is_none() => rsx! { p { class: "data-rail-note", "No episode records a length yet." } },
                    (_, Some(Some(Ok(distribution)))) => rsx! { EpisodeLengthChart { distribution: distribution.clone() } },
                    (_, Some(Some(Err(error)))) => rsx! { p { class: "data-rail-note", role: "alert", "{error}" } },
                    _ => rsx! { div { class: "data-lengths is-loading", role: "status", "aria-label": "Counting episode lengths" } },
                }
            }
            section { class: "data-overview-section", "aria-label": "Streams",
                h3 { "Streams" }
                match inspection {
                    None => rsx! { p { class: "data-rail-note", role: "status", "Reading dataset definition…" } },
                    Some(Err(error)) => rsx! { p { class: "data-rail-note", role: "alert", "{error}" } },
                    Some(Ok(detail)) if detail.topics().is_empty() => rsx! { p { class: "data-rail-note", "This dataset declares no streams yet." } },
                    Some(Ok(detail)) => rsx! { StreamLedger { detail } },
                }
            }
            }
            // Where the episodes come from, as a query can name them.
            footer { class: "data-overview-footer",
                span { "MCAP recordings" }
                code { "spql.mcap.episodes('{sql_name}')" }
            }
        }
    }
}

/// Every dataset in the project, before one is chosen: one ledger row each,
/// with the totals that tell them apart. Import stays in the rail; an empty
/// project offers it here too, as the one thing to do.
#[component]
pub(super) fn DatasetsAtAGlance(
    datasets: Vec<DatasetSelection>,
    on_select: EventHandler<DatasetSelection>,
    on_import: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "data-glance", "aria-label": "Datasets",
            header { class: "data-glance-head",
                h2 { "Datasets" }
                p {
                    if datasets.is_empty() {
                        "A dataset is a folder of MCAP recordings; each file is one episode."
                    } else {
                        "Choose a dataset to see its episodes and streams."
                    }
                }
            }
            if datasets.is_empty() {
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    onclick: move |_| on_import.call(()), "Import MCAP recordings" }
            } else {
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
                            for (index, dataset) in datasets.into_iter().enumerate() {
                                DatasetRow { key: "{dataset.sql_name()}", dataset, order: index, on_select }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DatasetRow(
    dataset: DatasetSelection,
    order: usize,
    on_select: EventHandler<DatasetSelection>,
) -> Element {
    let facts = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        let row = DatasetClient::current()
            .run_spiral_query(DatasetFacts::sql(&dataset), 1)
            .await
            .ok()?
            .into_rows()
            .into_iter()
            .next()?;
        serde_json::from_value::<DatasetFacts>(serde_json::Value::Object(row)).ok()
    }));
    let facts = facts.read().clone().flatten();
    let cell =
        |read: fn(&DatasetFacts) -> String| facts.as_ref().map_or_else(|| "…".to_owned(), read);
    let chosen = dataset.clone();
    let open = dataset.clone();
    rsx! {
        tr { class: "data-glance-row", style: "--i: {order}", onclick: move |_| on_select.call(chosen.clone()),
            td {
                button { r#type: "button", class: "data-glance-name", title: "{dataset.sql_name()}",
                    onclick: move |event: MouseEvent| { event.stop_propagation(); on_select.call(open.clone()); },
                    KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                    span { "{dataset.display_name()}" }
                }
            }
            td { class: "is-number", {cell(DatasetFacts::episodes_label)} }
            td { class: "is-number", {cell(DatasetFacts::recorded_label)} }
            td { class: "is-number",
                {facts.as_ref().map_or_else(|| "…".to_owned(), |facts| facts.average_seconds().map_or_else(|| "—".into(), ClockPresentation::clock_label))}
            }
            td { class: "is-number", {cell(DatasetFacts::size_label)} }
            td { class: "data-glance-span",
                {facts.as_ref().map_or_else(|| "…".to_owned(), |facts| facts.span_label().unwrap_or_else(|| "—".into()))}
            }
        }
    }
}

/// How many episodes run how long: one bar per slice of the range, with the
/// average marked, so a cluster of aborted takes or one runaway recording
/// shows before any episode is opened.
#[component]
fn EpisodeLengthChart(distribution: LengthDistribution) -> Element {
    let tallest = distribution.tallest() as f64;
    let bins = distribution.counts.len();
    let (shortest, longest) = (
        distribution.edge_seconds(0),
        distribution.edge_seconds(bins),
    );
    let total = distribution.counts.iter().sum::<u64>();
    rsx! {
        figure { class: "data-lengths",
            div { class: "data-lengths-plot", role: "img",
                "aria-label": "{total} episodes from {shortest.clock_label()} to {longest.clock_label()}",
                for (index, episodes) in distribution.counts.iter().enumerate() {
                    div { key: "{index}", class: "data-lengths-bin",
                        title: "{distribution.edge_seconds(index).clock_label()}–{distribution.edge_seconds(index + 1).clock_label()} · {episodes} episodes",
                        i { style: "--i: {index}; height: {*episodes as f64 / tallest * 100.0:.1}%", "data-empty": *episodes == 0 }
                    }
                }
                if let Some(average) = distribution.average_seconds {
                    span { class: "data-lengths-average", style: "left: {distribution.percent(average):.2}%",
                        "data-side": if distribution.percent(average) > 70.0 { "left" } else { "right" },
                        span { "avg {average.clock_label()}" }
                    }
                }
            }
            figcaption { class: "data-lengths-axis",
                span { "{shortest.clock_label()}" }
                span { "{longest.clock_label()}" }
            }
        }
    }
}

/// Every stream the dataset declares, cameras first, with how many messages
/// it holds and whether every episode carries it.
#[component]
fn StreamLedger(detail: DatasetInspection) -> Element {
    let mut topics = detail.topics().to_vec();
    topics.sort_by_key(|topic| !topic.is_video());
    // Coverage earns a column only when some stream is missing from some
    // episode; "all" on every row says nothing.
    let partial = detail
        .members()
        .is_some_and(|total| topics.iter().any(|topic| topic.members() != total));
    rsx! {
        div { class: "data-ledger-card",
        table { class: "data-ledger", "aria-label": "Streams",
            thead { tr { th { "Stream" } th { "Schema" } th { class: "is-number", "Messages" } if partial { th { class: "is-number", "Episodes" } } } }
            tbody {
                for topic in topics.iter() {
                    tr { key: "{topic.name()}-{topic.revision().map(|r| r.variant().to_owned()).unwrap_or_default()}",
                        td { class: "data-ledger-stream",
                            StreamGlyph { topic: topic.clone() }
                            span { "{topic.name()}" }
                        }
                        td { class: "data-ledger-schema", "{topic.schemas().join(\", \")}" }
                        td { class: "is-number", {topic.samples().map(GroupedCountPresentation::grouped_label).unwrap_or_else(|| "—".into())} }
                        // A stream in every episode is the rule; the exception
                        // is what an operator is looking for.
                        if partial {
                            td { class: "is-number",
                                match detail.members() {
                                    Some(total) if total == topic.members() => rsx! { span { class: "data-ledger-all", title: "In every episode", "all" } },
                                    Some(total) => rsx! { span { class: "data-ledger-partial", "{topic.members()} of {total}" } },
                                    None => rsx! { "{topic.members()}" },
                                }
                            }
                        }
                    }
                }
            }
        }
        }
    }
}

#[component]
fn StreamGlyph(topic: DatasetTopic) -> Element {
    if topic.is_video() {
        rsx! { span { class: "data-ledger-glyph is-camera", title: "Camera", CameraStreamIcon {} } }
    } else {
        rsx! { span { class: "data-ledger-glyph", title: "Data", SignalStreamIcon {} } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn dataset_facts_read_as_instrument_values() {
        let facts: DatasetFacts = serde_json::from_value(serde_json::json!({
            "episodes": 40, "duration_ns": 3_549_898_920_704_u64, "bytes": 3_509_376_768_u64,
            "shortest_ns": 4_600_000_000_u64, "longest_ns": 175_400_000_000_u64,
            "first_start": 1_743_701_335_734_982_912_u64, "last_end": 1_776_208_497_899_157_760_u64
        }))
        .unwrap();
        assert_eq!(facts.episodes_label(), "40");
        assert_eq!(facts.recorded_label(), "59 min 10 s");
        assert_eq!(facts.size_label(), "3.3 GiB");
        assert_eq!(
            facts.span_label().as_deref(),
            Some("2025-04-03 → 2026-04-14")
        );
        assert_eq!(facts.length_range(), Some((4_600_000_000, 175_400_000_000)));
        assert!((facts.average_seconds().unwrap() - 88.747).abs() < 0.001);
        let sparse: DatasetFacts =
            serde_json::from_value(serde_json::json!({"episodes": 0})).unwrap();
        assert_eq!(sparse.span_label(), None);
        let simulated: DatasetFacts =
            serde_json::from_value(serde_json::json!({"first_start": 5, "last_end": 9})).unwrap();
        assert_eq!(simulated.span_label(), None);
        let dataset = DatasetSelection::new(vec!["owner's data".into()]).unwrap();
        assert!(DatasetFacts::sql(&dataset).contains("'owner''s data'"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_length_distribution_counts_every_episode_in_its_slice() {
        let range = (10_000_000_000, 46_000_000_000);
        let dataset = DatasetSelection::new(vec!["owner's data".into()]).unwrap();
        let sql = LengthDistribution::sql(&dataset, range);
        assert!(sql.contains("floor((duration_ns - 10000000000) / 1000000000)"));
        assert!(sql.contains("'owner''s data'"));
        let distribution =
            LengthDistribution::new(range, &[(0.0, 2), (17.0, 5), (36.0, 1)], Some(28.0));
        assert_eq!(distribution.counts.len(), 36);
        assert_eq!(distribution.counts.iter().sum::<u64>(), 8);
        assert_eq!(distribution.counts[35], 1);
        assert_eq!(distribution.tallest(), 5);
        assert_eq!(distribution.edge_seconds(36), 46.0);
        assert_eq!(distribution.percent(28.0), 50.0);
        let single = LengthDistribution::new((7, 7), &[(0.0, 1)], None);
        assert_eq!(single.width_ns, 1);
    }
}
