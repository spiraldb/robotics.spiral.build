//! A curated view in Data: the rows a curation published, read once when its
//! tab opens and listed by episode, each interval opening its episode with
//! the view's intervals drawn on its timeline.
//!
//! Nothing caches a model's answers, so every read of a view asks its models
//! again about each episode it reads. The tab reads it once and holds the
//! rows for as long as it stays open.
use super::DatasetClient;
use super::catalog_rail::OpenedEpisode;
use crate::activities::curation::CuratedView;
use crate::activities::episode::{
    Episode, EpisodeRows, EpisodeStill, EpisodeStills, RowFace, RowFaceView, StillMoment,
};
use crate::components::icons::SpinnerIcon;
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::{ClockPresentation as _, LongDurationPresentation as _};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, Fact, FactLayout, FactList, InlineConfirmation, KindMark, Notice,
    NoticeTone,
};
use std::fmt::{self, Display, Formatter};

/// The most rows one read lists, which is all the backend answers.
const VIEW_ROWS: usize = 1_000;

/// The interval every curated view's rows lead with.
const INTERVAL_COLUMNS: [&str; 3] = ["episode_id", "start_ns", "end_ns"];

/// What each episode fact read beside a row of the view is named after.
const LISTED: &str = "listed_";

/// One value a view publishes beside an interval, as a list shows it.
#[derive(Clone, Debug, PartialEq)]
enum ViewCell {
    Missing,
    Text(String),
    Number(f64),
    Flag(bool),
    /// A structured answer, such as a category with its probability, shown
    /// as its parts.
    Parts(Vec<ViewCell>),
}

impl From<&Value> for ViewCell {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Self::Missing,
            Value::Bool(flag) => Self::Flag(*flag),
            Value::Number(number) => number.as_f64().map_or(Self::Missing, Self::Number),
            Value::String(text) => Self::Text(text.clone()),
            Value::Array(items) => Self::Parts(items.iter().map(Self::from).collect()),
            Value::Object(fields) => Self::Parts(fields.values().map(Self::from).collect()),
        }
    }
}

impl ViewCell {
    /// The cell as a row shows it under `column`: a number the way the
    /// curation that published the column prints it, else as it reads.
    fn reading(&self, column: &str) -> String {
        match self {
            Self::Number(value) => CuratedView::reading(column, *value),
            _ => None,
        }
        .unwrap_or_else(|| self.to_string())
    }
}

impl Display for ViewCell {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("—"),
            Self::Text(text) => formatter.write_str(text),
            Self::Number(number) if number.fract() == 0.0 => write!(formatter, "{number:.0}"),
            Self::Number(number) => write!(formatter, "{number:.2}"),
            Self::Flag(flag) => formatter.write_str(if *flag { "yes" } else { "no" }),
            Self::Parts(parts) => {
                let shown = parts
                    .iter()
                    .filter(|part| **part != Self::Missing)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if shown.is_empty() {
                    formatter.write_str("—")
                } else {
                    formatter.write_str(&shown.join(" "))
                }
            }
        }
    }
}

/// One row of a view: an interval of one episode, in seconds from its first
/// message, and the values published beside it, one per column after the
/// interval.
#[derive(Clone, Debug, PartialEq)]
struct ViewRow {
    episode: EpisodeId,
    span: (f64, f64),
    cells: Vec<ViewCell>,
}

impl ViewRow {
    fn read(
        row: &Map<String, Value>,
        columns: &[String],
        episodes: &[Episode],
    ) -> Result<Self, String> {
        let episode = row
            .get("episode_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "A row of the view names no episode.".to_owned())
            .and_then(EpisodeId::try_from)?;
        let origin = episodes
            .iter()
            .find(|read| read.id() == &episode)
            .and_then(|read| read.facts().message_start_time())
            .and_then(|start| i64::try_from(start).ok())
            .ok_or("An episode of the view lists no first message.")?;
        let seconds = |column: &str| {
            row.get(column)
                .and_then(Value::as_i64)
                .map(|nanos| nanos.saturating_sub(origin) as f64 / 1e9)
                .ok_or_else(|| format!("A row of the view has no {column}."))
        };
        Ok(Self {
            span: (seconds("start_ns")?, seconds("end_ns")?),
            episode,
            cells: columns
                .iter()
                .map(|column| ViewCell::from(row.get(column).unwrap_or(&Value::Null)))
                .collect(),
        })
    }
    fn seconds(&self) -> f64 {
        (self.span.1 - self.span.0).max(0.0)
    }
}

/// What one read of a view answered: the episodes it keeps part of, in
/// recording order, the columns after the interval, and its rows, each
/// episode's by start.
#[derive(Clone, Debug, PartialEq)]
struct ViewReading {
    episodes: Vec<Episode>,
    columns: Vec<String>,
    rows: Vec<ViewRow>,
    truncated: bool,
}

impl ViewReading {
    /// Read `view` once, with the listed facts of the episode each row
    /// belongs to beside it: the view keeps the first episodes its Selects
    /// keep, wherever they fall in the dataset.
    async fn read(view: CuratedView) -> Result<Self, String> {
        let rows = format!("SELECT * FROM {}", view.view());
        let answer = DatasetClient::current()
            .run_spiral_query(
                Episode::beside_sql(view.dataset(), &rows, LISTED),
                VIEW_ROWS,
            )
            .await
            .map_err(|error| error.to_string())?;
        Self::of(view.dataset(), &answer)
    }

    fn of(dataset: &DatasetSelection, answer: &SpiralQuerySnapshot) -> Result<Self, String> {
        let columns = answer
            .columns()
            .iter()
            .map(|column| column.name().to_owned())
            .filter(|column| {
                !INTERVAL_COLUMNS.contains(&column.as_str()) && !column.starts_with(LISTED)
            })
            .collect::<Vec<_>>();
        let mut episodes = Vec::<Episode>::new();
        for row in answer.rows() {
            let episode = Episode::beside(dataset, row, LISTED)?;
            if !episodes.iter().any(|read| read.id() == episode.id()) {
                episodes.push(episode);
            }
        }
        episodes.sort_by(|left, right| left.recording_place().cmp(&right.recording_place()));
        let mut rows = answer
            .rows()
            .iter()
            .map(|row| ViewRow::read(row, &columns, &episodes))
            .collect::<Result<Vec<_>, _>>()?;
        let place = |episode: &EpisodeId| episodes.iter().position(|read| read.id() == episode);
        rows.sort_by(|left, right| {
            place(&left.episode)
                .cmp(&place(&right.episode))
                .then(left.span.0.total_cmp(&right.span.0))
        });
        Ok(Self {
            episodes,
            columns,
            rows,
            truncated: answer.truncated(),
        })
    }

    /// Each episode, in recording order, beside its rows.
    fn by_episode(&self) -> Vec<(&Episode, Vec<&ViewRow>)> {
        self.episodes
            .iter()
            .map(|episode| {
                let rows = self
                    .rows
                    .iter()
                    .filter(|row| &row.episode == episode.id())
                    .collect::<Vec<_>>();
                (episode, rows)
            })
            .collect()
    }
    fn kept_seconds(&self) -> f64 {
        self.rows.iter().map(ViewRow::seconds).sum()
    }
    /// How long the episodes it keeps part of are, whole.
    fn read_seconds(&self) -> f64 {
        self.episodes
            .iter()
            .filter_map(|episode| episode.facts().duration_seconds())
            .sum()
    }
    /// The first column any row holds words in, which names each row — the
    /// subtask an interval is, say. The rest are its measures.
    fn naming_column(&self) -> Option<usize> {
        (0..self.columns.len()).find(|&column| {
            self.rows
                .iter()
                .any(|row| matches!(row.cells.get(column), Some(ViewCell::Text(_))))
        })
    }
    /// `row` as its episode's list words it: named by its words when it has
    /// some, else by its span, drawn where it sits in the episode, with each
    /// other column as a measure.
    fn face(&self, row: &ViewRow, episode: &Episode) -> RowFace {
        let duration = episode.facts().duration_seconds().unwrap_or_default();
        let naming = self.naming_column();
        let mut face = match naming.and_then(|column| row.cells.get(column)) {
            Some(ViewCell::Text(words)) => RowFace::described(row.span, duration, words),
            _ => RowFace::span(row.span, duration),
        };
        for (column, cell) in row.cells.iter().enumerate() {
            if Some(column) != naming {
                let name = &self.columns[column];
                face = face.measured(cell.reading(name), Self::column_label(name));
            }
        }
        face
    }
    /// A column's name as words: `subtask_category` reads "Subtask category".
    fn column_label(column: &str) -> String {
        let words = column.replace('_', " ");
        let mut letters = words.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default()
    }
    /// `episode`'s rows as a timeline row named for the view, each span by
    /// its words or else its clock, `chosen` marked when there is one.
    fn spans(&self, view: &str, episode: &Episode, chosen: Option<&ViewRow>) -> ViewSpans {
        let naming = self.naming_column();
        let spans = self
            .rows
            .iter()
            .filter(|row| &row.episode == episode.id())
            .map(|row| {
                let label = naming.and_then(|column| row.cells.get(column)).map_or_else(
                    || {
                        format!(
                            "{} – {}",
                            row.span.0.clock_label(),
                            row.span.1.clock_label()
                        )
                    },
                    ToString::to_string,
                );
                TimelineInterval::labeled(row.span.0, row.span.1, label)
            })
            .collect();
        let chosen = chosen.map(|row| row.span.0);
        ViewSpans {
            track: IntervalTrack::new("curated-view", view, spans).with_chosen(chosen),
            chosen,
        }
    }
}

/// A curated view's intervals of one episode, drawn as a row of the
/// episode's timeline, and the one chosen to play.
#[derive(Clone, PartialEq)]
pub(super) struct ViewSpans {
    track: IntervalTrack,
    chosen: Option<f64>,
}

impl ViewSpans {
    pub(super) fn track(&self) -> IntervalTrack {
        self.track.clone()
    }
    /// Where the chosen interval starts, if one was chosen rather than the
    /// whole episode.
    pub(super) fn chosen(&self) -> Option<f64> {
        self.chosen
    }
}

/// A curated view's tab: where it comes from, what one read of it kept, and
/// its intervals by episode; and Unpublish, which ends it.
#[component]
pub(super) fn CuratedViewSubject(
    view: CuratedView,
    on_dataset: EventHandler<()>,
    on_open: EventHandler<OpenedEpisode>,
    on_unpublished: EventHandler<()>,
) -> Element {
    let mut confirming = use_signal(|| false);
    let dataset = view.dataset().display_name();
    let name = view.view().to_owned();
    let unpublish_id = format!("data-curated-unpublish-{name}");
    rsx! {
        section { class: "data-curated", "aria-label": "Curated view {name}",
            header { class: "data-overview-head",
                KindMark { class: "data-overview-mark", letter: 'C', hue: "var(--tone-kind-curated)", label: "Curated view" }
                div { class: "data-curated-title",
                    h2 { "{name}" }
                    p { class: "data-curated-origin",
                        "{view.output()} of the curation “{view.curation()}”, over "
                        Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                            class: "data-curated-dataset", title: "Open {dataset}", onclick: move |_| on_dataset.call(()),
                            KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                            "{dataset}"
                        }
                    }
                }
                if !confirming() {
                    Button { id: unpublish_id.clone(), purpose: ActionPurpose::Remove, variant: ButtonVariant::Link,
                        size: ButtonSize::Compact, class: "data-curated-unpublish",
                        title: "Drop this view from the catalog",
                        onclick: move |_| confirming.set(true), "Unpublish" }
                }
            }
            if confirming() {
                UnpublishConfirmation { view: view.clone(), return_focus: unpublish_id,
                    on_done: move |unpublished: bool| {
                        confirming.set(false);
                        if unpublished {
                            on_unpublished.call(());
                        }
                    },
                }
            }
            if !view.is_current() {
                Notice { class: "data-curated-notice", tone: NoticeTone::Caution,
                    "Its curation has changed since it was published. Publish it again from Curation to bring the view up to date."
                }
            }
            if view.is_bounded() {
                CuratedRows { view, on_open }
            } else {
                Notice { class: "data-curated-notice", tone: NoticeTone::Caution,
                    "Published before views were bounded, so reading it would ask its models about every episode of the dataset. Publish it again from Curation to inspect it here."
                }
            }
        }
    }
}

/// Whether to unpublish the view, asked under its head: confirmed, the view
/// leaves the catalog and its curation forgets it, keeping the recipe.
/// `on_done` says whether it was unpublished.
#[component]
fn UnpublishConfirmation(
    view: CuratedView,
    return_focus: String,
    on_done: EventHandler<bool>,
) -> Element {
    let studio = StudioProjectState::current();
    let mut pending = use_signal(|| false);
    let name = view.view().to_owned();
    rsx! {
        InlineConfirmation {
            id: "data-curated-unpublish-confirmation",
            class: "data-curated-confirmation",
            copy: ConfirmationCopy::new(
                format!(
                    "Unpublish {name}? The catalog view is dropped; the curation “{}” keeps its recipe and can publish it again.",
                    view.curation()
                ),
                if pending() { "Unpublishing…" } else { "Unpublish" },
                "Keep view",
            ),
            response: ConfirmationResponse::new(
                ActionPurpose::Remove,
                move |_| {
                    pending.set(true);
                    let view = view.clone();
                    spawn(async move {
                        let outcome = view.unpublish().await;
                        pending.set(false);
                        let unpublished = outcome.is_ok();
                        match outcome {
                            Ok(()) => studio.report_success_from(
                                MessageSource::Curation,
                                format!("Unpublished {}", view.view()),
                            ),
                            Err(error) => studio.report_from(MessageSource::Curation, error),
                        }
                        on_done.call(unpublished);
                    });
                },
                move |_| on_done.call(false),
            ),
            dismissal: ConfirmationDismissal::new(return_focus).with_pending(pending()),
        }
    }
}

/// The view read once: its totals, then every episode it kept part of, each
/// interval under it.
#[component]
fn CuratedRows(view: CuratedView, on_open: EventHandler<OpenedEpisode>) -> Element {
    let read = view.clone();
    let mut view_read = use_resource(move || ViewReading::read(read.clone()));
    let answer = view_read.read();
    let first = match &*answer {
        Some(Ok(reading)) => reading.episodes.first().map(Episode::selection),
        _ => None,
    };
    let camera = EpisodeStills::use_overview_camera(first);
    match &*answer {
        None => rsx! {
            p { class: "data-curated-status", role: "status",
                SpinnerIcon {}
                "Curation in progress. This can take a few minutes."
            }
        },
        Some(Err(error)) => rsx! {
            div { class: "data-curated-status", role: "alert",
                p { "The view could not be read: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    onclick: move |_| view_read.restart(), "Read again" }
            }
        },
        Some(Ok(reading)) => {
            let groups = reading.by_episode();
            let listing = EpisodeRows::of(reading.episodes.iter());
            let intervals = reading.rows.len();
            rsx! {
                FactList { class: "data-overview-facts", layout: FactLayout::Strip,
                    Fact { label: "Intervals",
                        if reading.truncated { "first {intervals}" } else { "{intervals}" }
                    }
                    Fact { label: "Episodes", "{groups.len()}" }
                    Fact { label: "Kept time",
                        "{reading.kept_seconds().long_duration_label()} of {reading.read_seconds().long_duration_label()}"
                    }
                }
                if groups.is_empty() {
                    p { class: "data-rail-note", "It keeps nothing of the episodes it reads." }
                }
                ol { class: "data-curated-index",
                    for (episode, rows) in groups {
                        li { key: "{episode.id()}", class: "data-curated-group",
                            button { class: "data-curated-episode", r#type: "button",
                                title: "Open {episode.name()} with the view's intervals on its timeline",
                                onclick: {
                                    let opened = OpenedEpisode::from_view(episode.clone(), listing.label(episode), reading.spans(view.view(), episode, None));
                                    move |_| on_open.call(opened.clone())
                                },
                                RowFaceView {
                                    face: listing.face(episode),
                                    still: rsx! { EpisodeStill { moment: StillMoment::opening(episode), camera: camera.clone() } },
                                }
                                span { class: "data-curated-count",
                                    if rows.len() == 1 { "1 interval" } else { "{rows.len()} intervals" }
                                }
                            }
                            ol { class: "data-curated-rows",
                                for row in rows.iter() {
                                    li { key: "{row.span.0}",
                                        button { class: "data-curated-row", r#type: "button",
                                            title: "Play this interval in its episode",
                                            onclick: {
                                                let opened = OpenedEpisode::from_view(episode.clone(), listing.label(episode), reading.spans(view.view(), episode, Some(*row)));
                                                move |_| on_open.call(opened.clone())
                                            },
                                            RowFaceView {
                                                face: reading.face(row, episode),
                                                still: rsx! { EpisodeStill { moment: StillMoment::at(episode, f64::midpoint(row.span.0, row.span.1)), camera: camera.clone() } },
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::relations::QueryColumn;

    fn dataset() -> DatasetSelection {
        DatasetSelection::new(vec!["dataset_bottles".into()]).unwrap()
    }

    /// A row of the view on episode `id`, recorded from `start`, with the
    /// episode's listed facts beside it as one read answers them.
    fn row(id: u32, start: u64, published: Value) -> Value {
        let mut row = json!({
            "episode_id": EpisodeId::fixture(id).to_string(),
            "listed_source_uri": format!("s3://recordings/{id}.mcap"),
            "listed_message_start_time": start,
            "listed_duration_ns": 10_000_000_000_u64,
        });
        row.as_object_mut()
            .unwrap()
            .extend(published.as_object().unwrap().clone());
        row
    }

    fn answer(rows: Vec<Value>) -> SpiralQuerySnapshot {
        let columns = [
            "episode_id",
            "start_ns",
            "end_ns",
            "subtask",
            "subtask_category",
            "listed_source_uri",
            "listed_message_start_time",
            "listed_duration_ns",
        ]
        .map(|name| QueryColumn::new(name, "Utf8", true))
        .to_vec();
        let rows = rows
            .into_iter()
            .map(|row| row.as_object().unwrap().clone())
            .collect();
        SpiralQuerySnapshot::new(columns, rows, false)
    }

    #[test]
    fn a_view_reads_as_intervals_by_episode_in_recording_order() {
        let reading = ViewReading::of(
            &dataset(),
            &answer(vec![
                row(2, 90_000_000_000, json!({"start_ns": 91_000_000_000_i64, "end_ns": 93_500_000_000_i64,
                       "subtask": "drop the bottle", "subtask_category": {"category": "Pick-and-Place", "probability": 0.8}})),
                row(1, 5_000_000_000, json!({"start_ns": 9_000_000_000_i64, "end_ns": 10_000_000_000_i64,
                       "subtask": "grasp", "subtask_category": null})),
                row(1, 5_000_000_000, json!({"start_ns": 6_000_000_000_i64, "end_ns": 7_000_000_000_i64,
                       "subtask": "reach", "subtask_category": {"category": "Reaching", "probability": 0.5}})),
            ]),
        )
        .unwrap();
        assert_eq!(reading.columns, ["subtask", "subtask_category"]);
        let groups = reading.by_episode();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0.id(), &EpisodeId::fixture(1));
        let second = groups[1].0.clone();
        assert_eq!(
            groups[0].1.iter().map(|row| row.span).collect::<Vec<_>>(),
            [(1.0, 2.0), (4.0, 5.0)]
        );
        assert_eq!(groups[1].1[0].span, (1.0, 3.5));
        assert_eq!(reading.kept_seconds(), 4.5);
        assert_eq!(reading.read_seconds(), 20.0);
        assert_eq!(reading.naming_column(), Some(0));
        assert_eq!(groups[1].1[0].cells[1].to_string(), "Pick-and-Place 0.80");
        assert_eq!(groups[0].1[1].cells[1].to_string(), "—");
        assert_eq!(
            ViewReading::column_label("subtask_category"),
            "Subtask category"
        );
        let spans = reading.spans("pick_and_place", &second, Some(groups[1].1[0]));
        assert_eq!(spans.chosen(), Some(1.0));
    }

    #[test]
    fn a_published_measure_reads_as_curation_prints_it() {
        let rate = ViewCell::Number(0.6912);
        assert_eq!(rate.reading("progress_rate"), "0.69×");
        assert_eq!(rate.reading("similarity"), "69%");
        assert_eq!(rate.reading("similarity_2"), "69%");
        assert_eq!(rate.reading("similarity_of"), "0.69");
        assert_eq!(rate.reading("duration_seconds"), "0.69");
        assert_eq!(ViewCell::Missing.reading("progress_rate"), "—");
    }

    #[test]
    fn a_row_whose_episode_lists_no_first_message_is_an_error() {
        let mut unlisted = row(3, 0, json!({"start_ns": 0, "end_ns": 1, "subtask": "x"}));
        unlisted
            .as_object_mut()
            .unwrap()
            .remove("listed_message_start_time");
        assert!(ViewReading::of(&dataset(), &answer(vec![unlisted])).is_err());
    }
}
