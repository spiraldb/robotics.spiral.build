//! A curated view in Data: the episodes a curation published, read once when
//! its tab opens, each with the ranges the view keeps of it; each range
//! opens its episode with the view's ranges drawn on its timeline.
//!
//! A view reads the answers its models stored, which the run publishing it
//! writes first; while that run goes on, the tab says what it is doing and
//! reads nothing. The tab reads the view once and holds the rows for as long
//! as it stays open. The curation that published the view says what its
//! columns hold, so a range is named by the subtask it lies in and measured
//! by that subtask's similarity and category, as Curation shows them.
use super::DatasetClient;
use super::catalog_rail::OpenedEpisode;
use crate::activities::curation::{CuratedView, SubtaskColumn, ViewFill};
use crate::activities::episode::{
    Episode, EpisodeListing, EpisodeRows, EpisodeStill, EpisodeStills, RowFace, RowFaceView,
    StillMoment,
};
use crate::components::icons::SpinnerIcon;
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::{ClockPresentation as _, LongDurationPresentation as _};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, Fact, FactLayout, FactList, InlineConfirmation, KindMark, Notice,
    NoticeTone,
};

/// The most episodes one read lists, which is all the backend answers.
const VIEW_ROWS: usize = 1_000;

/// What each episode fact read beside a row of the view is named after.
const LISTED: &str = "listed_";

/// One entry of a list a view holds per episode: a range of log time and
/// what it carries.
struct ViewEntry<'a> {
    span: (i64, i64),
    fields: &'a Map<String, Value>,
}

impl<'a> ViewEntry<'a> {
    /// Every entry of the list in `column` of `row`; none when it holds none.
    fn all(row: &'a Map<String, Value>, column: &str) -> Vec<Self> {
        row.get(column)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| {
                let fields = entry.as_object()?;
                let time = |field: &str| fields.get(field).and_then(Value::as_i64);
                Some(Self {
                    span: (time("start_ns")?, time("end_ns")?),
                    fields,
                })
            })
            .collect()
    }
    fn holds(&self, instant: i64) -> bool {
        self.span.0 <= instant && instant < self.span.1
    }
}

/// One kept range of a view, in seconds from its episode's first message,
/// with the words of the subtask it lies in and what measures it.
#[derive(Clone, Debug, PartialEq)]
struct ViewRow {
    episode: EpisodeId,
    span: (f64, f64),
    name: Option<String>,
    measures: Vec<(String, &'static str)>,
}

impl ViewRow {
    /// Whether this is all of `episode` with nothing of its own to say, so
    /// the episode's row already describes it.
    fn is_whole_of(&self, episode: &Episode) -> bool {
        let duration = episode.facts().duration_seconds().unwrap_or_default();
        self.name.is_none()
            && self.measures.is_empty()
            && self.span.0 <= 0.05
            && self.span.1 >= duration - 0.05
    }
    /// The kept ranges of one row of the view, each named and measured by
    /// the subtask columns' entries its middle lies in.
    fn of_row(
        row: &Map<String, Value>,
        (episode, origin): (&Episode, i64),
        subtasks: &[SubtaskColumn],
    ) -> Vec<Self> {
        let seconds = |nanos: i64| nanos.saturating_sub(origin) as f64 / 1e9;
        ViewEntry::all(row, "kept")
            .into_iter()
            .map(|kept| {
                let middle = kept.span.0 + (kept.span.1 - kept.span.0) / 2;
                let mut name = None;
                let mut measures = Vec::new();
                for column in subtasks {
                    let entries = ViewEntry::all(row, column.column());
                    let Some(entry) = entries.iter().find(|entry| entry.holds(middle)) else {
                        continue;
                    };
                    match column {
                        SubtaskColumn::Described(_) if name.is_none() => {
                            name = entry
                                .fields
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                        SubtaskColumn::Described(_) => {}
                        SubtaskColumn::Similar(_) => {
                            if let Some(similarity) =
                                entry.fields.get("similarity").and_then(Value::as_f64)
                            {
                                measures.push((
                                    format!("{:.0}%", similarity * 100.0),
                                    "How likely this is the operation searched for",
                                ));
                            }
                        }
                        SubtaskColumn::Category { name: category, .. } => {
                            measures.push((category.clone(), "Category"));
                        }
                    }
                }
                Self {
                    episode: episode.id().clone(),
                    span: (seconds(kept.span.0), seconds(kept.span.1)),
                    name,
                    measures,
                }
            })
            .collect()
    }
    fn seconds(&self) -> f64 {
        (self.span.1 - self.span.0).max(0.0)
    }
}

/// What one read of a view answered: the episodes it keeps part of, in
/// recording order, and their kept ranges, each episode's by start.
#[derive(Clone, Debug, PartialEq)]
struct ViewReading {
    episodes: Vec<Episode>,
    rows: Vec<ViewRow>,
    truncated: bool,
}

impl ViewReading {
    /// Read `view` once — its `kept` ranges and the columns that name and
    /// measure them, never its bigger lists — with the listed facts of each
    /// episode beside it.
    async fn read(view: CuratedView) -> Result<Self, String> {
        let statement = view
            .statement()
            .ok_or_else(|| format!("“{}” is not published yet.", view.view()))?;
        let columns = view
            .subtasks()
            .iter()
            .map(|column| format!(", v.{}", column.column()))
            .collect::<String>();
        let rows = format!("SELECT v.episode_id, v.kept{columns} FROM ({statement}) v");
        let answer = DatasetClient::current()
            .run_spiral_query(view.listing().beside_sql(&rows, LISTED), VIEW_ROWS)
            .await
            .map_err(|error| error.to_string())?;
        Self::of(view.listing(), view.subtasks(), &answer)
    }

    fn of(
        listing: &EpisodeListing,
        subtasks: &[SubtaskColumn],
        answer: &SpiralQuerySnapshot,
    ) -> Result<Self, String> {
        let mut episodes = Vec::<Episode>::new();
        let mut rows = Vec::new();
        for row in answer.rows() {
            let episode = listing.beside(row, LISTED)?;
            let origin = episode
                .facts()
                .message_start_time()
                .and_then(|start| i64::try_from(start).ok())
                .ok_or("An episode of the view lists no first message.")?;
            rows.extend(ViewRow::of_row(row, (&episode, origin), subtasks));
            episodes.push(episode);
        }
        episodes.sort_by(|left, right| left.recording_place().cmp(&right.recording_place()));
        let place = |episode: &EpisodeId| episodes.iter().position(|read| read.id() == episode);
        rows.sort_by(|left, right| {
            place(&left.episode)
                .cmp(&place(&right.episode))
                .then(left.span.0.total_cmp(&right.span.0))
        });
        Ok(Self {
            episodes,
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
    /// How long what it keeps is, out of its episodes' whole length when it
    /// keeps only part of them.
    fn kept_time_label(&self) -> String {
        let (kept, read) = (self.kept_seconds(), self.read_seconds());
        if read - kept < 0.05 {
            kept.long_duration_label()
        } else {
            format!(
                "{} of {}",
                kept.long_duration_label(),
                read.long_duration_label()
            )
        }
    }
    /// `row` as its episode's list words it: named by the subtask it lies
    /// in when it has one, else by its span, drawn where it sits in the
    /// episode, with each measure.
    fn face(&self, row: &ViewRow, episode: &Episode) -> RowFace {
        let duration = episode.facts().duration_seconds().unwrap_or_default();
        let face = match &row.name {
            Some(words) => RowFace::described(row.span, duration, words),
            None => RowFace::span(row.span, duration),
        };
        row.measures.iter().fold(face, |face, (reading, meaning)| {
            face.measured(reading.clone(), *meaning)
        })
    }
    /// `episode`'s rows as a timeline row named for the view, each span by
    /// its words or else its clock, `chosen` marked when there is one.
    fn spans(&self, view: &str, episode: &Episode, chosen: Option<&ViewRow>) -> ViewSpans {
        let spans = self
            .rows
            .iter()
            .filter(|row| &row.episode == episode.id())
            .map(|row| {
                let label = row.name.clone().unwrap_or_else(|| {
                    format!(
                        "{} – {}",
                        row.span.0.clock_label(),
                        row.span.1.clock_label()
                    )
                });
                TimelineInterval::labeled(row.span.0, row.span.1, label)
            })
            .collect();
        let chosen = chosen.map(|row| row.span);
        ViewSpans {
            track: IntervalTrack::new("curated-view", view, spans)
                .with_chosen(chosen.map(|(start, _)| start)),
            chosen,
        }
    }
}

/// A curated view's intervals of one episode, drawn as a row of the
/// episode's timeline, and the one chosen to play.
#[derive(Clone, PartialEq)]
pub(super) struct ViewSpans {
    track: IntervalTrack,
    chosen: Option<(f64, f64)>,
}

impl ViewSpans {
    pub(super) fn track(&self) -> IntervalTrack {
        self.track.clone()
    }
    /// The chosen interval, if one was chosen rather than the whole episode.
    pub(super) fn chosen(&self) -> Option<(f64, f64)> {
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
                        title: "Drop this view",
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
            match view.fill().cloned() {
                Some(fill @ ViewFill::Running(_)) => rsx! {
                    p { class: "data-curated-status", role: "status",
                        SpinnerIcon {}
                        "Curation in progress. {fill.doing()}"
                    }
                },
                Some(ViewFill::Failed(reason)) => rsx! {
                    Notice { class: "data-curated-notice", tone: NoticeTone::Danger,
                        "Its curation could not finish: {reason} Publish it again from Curation to go on where it stopped."
                    }
                    CuratedRows { view, on_open }
                },
                None => rsx! {
                    if !view.is_current() {
                        Notice { class: "data-curated-notice", tone: NoticeTone::Caution,
                            "Its curation has changed since it was published. Publish it again from Curation to bring the view up to date."
                        }
                    }
                    CuratedRows { view, on_open }
                },
            }
        }
    }
}

/// Whether to unpublish the view, asked under its head: confirmed, the view
/// is dropped and its curation forgets it, keeping the recipe.
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
                    "Unpublish {name}? The view is dropped; the curation “{}” keeps its recipe and can publish it again.",
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
    let camera = EpisodeStills::use_overview_camera(Some(view.dataset().clone()));
    match &*answer {
        None => rsx! {
            p { class: "data-curated-status", role: "status",
                SpinnerIcon {}
                "Reading the view…"
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
            let kept_time = reading.kept_time_label();
            rsx! {
                FactList { class: "data-overview-facts", layout: FactLayout::Strip,
                    Fact { label: "Episodes",
                        if reading.truncated { "first {groups.len()}" } else { "{groups.len()}" }
                    }
                    Fact { label: "Intervals", "{intervals}" }
                    Fact { label: "Kept time", "{kept_time}" }
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
                                    if matches!(rows.as_slice(), [row] if row.is_whole_of(episode)) { "Whole episode" }
                                    else if rows.len() == 1 { "1 interval" } else { "{rows.len()} intervals" }
                                }
                            }
                            if !matches!(rows.as_slice(), [row] if row.is_whole_of(episode)) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::relations::QueryColumn;

    fn dataset() -> EpisodeListing {
        EpisodeListing::mcap_fixture("dataset_bottles")
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
            "kept",
            "break_down_subtasks_1",
            "find_similar_subtasks_2",
        ]
        .map(QueryColumn::new)
        .to_vec();
        let rows = rows
            .into_iter()
            .map(|row| row.as_object().unwrap().clone())
            .collect();
        SpiralQuerySnapshot::new(columns, rows, false)
    }

    fn columns() -> Vec<SubtaskColumn> {
        vec![
            SubtaskColumn::Described("break_down_subtasks_1".into()),
            SubtaskColumn::Similar("find_similar_subtasks_2".into()),
            SubtaskColumn::Category {
                column: "group_similar_subtasks_3_category_1".into(),
                name: "Pick-and-Place".into(),
            },
        ]
    }

    #[test]
    fn a_view_reads_as_kept_ranges_by_episode_named_by_the_subtask_they_lie_in() {
        let reading = ViewReading::of(
            &dataset(),
            &columns(),
            &answer(vec![
                row(2, 90_000_000_000, json!({
                    "kept": [{"start_ns": 91_000_000_000_i64, "end_ns": 93_500_000_000_i64}],
                    "break_down_subtasks_1": [{"description": "drop the bottle", "start_ns": 90_000_000_000_i64, "end_ns": 94_000_000_000_i64}],
                    "find_similar_subtasks_2": [{"start_ns": 90_000_000_000_i64, "end_ns": 94_000_000_000_i64, "similarity": 0.8}],
                    "group_similar_subtasks_3_category_1": [{"start_ns": 90_000_000_000_i64, "end_ns": 94_000_000_000_i64}],
                })),
                row(1, 5_000_000_000, json!({
                    "kept": [{"start_ns": 6_000_000_000_i64, "end_ns": 7_000_000_000_i64}, {"start_ns": 9_000_000_000_i64, "end_ns": 10_000_000_000_i64}],
                    "break_down_subtasks_1": [{"description": "reach", "start_ns": 5_000_000_000_i64, "end_ns": 8_000_000_000_i64}],
                    "find_similar_subtasks_2": null,
                    "group_similar_subtasks_3_category_1": [],
                })),
            ]),
        )
        .unwrap();
        let groups = reading.by_episode();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0.id(), &EpisodeId::fixture(1));
        assert_eq!(
            groups[0].1.iter().map(|row| row.span).collect::<Vec<_>>(),
            [(1.0, 2.0), (4.0, 5.0)]
        );
        assert_eq!(groups[0].1[0].name.as_deref(), Some("reach"));
        assert_eq!(groups[0].1[1].name, None, "it lies in no subtask");
        let second = groups[1].0.clone();
        assert_eq!(groups[1].1[0].span, (1.0, 3.5));
        assert_eq!(
            groups[1].1[0].measures,
            [
                (
                    "80%".to_owned(),
                    "How likely this is the operation searched for"
                ),
                ("Pick-and-Place".to_owned(), "Category")
            ]
        );
        assert_eq!(reading.kept_seconds(), 4.5);
        assert_eq!(reading.read_seconds(), 20.0);
        let spans = reading.spans("pick_and_place", &second, Some(groups[1].1[0]));
        assert_eq!(spans.chosen(), Some((1.0, 3.5)));
    }

    #[test]
    fn a_row_whose_episode_lists_no_first_message_is_an_error() {
        let mut unlisted = row(3, 0, json!({"kept": [{"start_ns": 0, "end_ns": 1}]}));
        unlisted
            .as_object_mut()
            .unwrap()
            .remove("listed_message_start_time");
        assert!(ViewReading::of(&dataset(), &[], &answer(vec![unlisted])).is_err());
    }
}
