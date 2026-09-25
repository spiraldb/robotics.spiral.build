//! How every measurement's output is read: an index that scrolls beside
//! a recording that holds still.
//!
//! All three output views are the same thing — a list of subtask intervals,
//! and one of them shown as video. Written separately they each pushed the
//! recording off the bottom of a growing list, so the click that plays a
//! subtask had nothing visible to show for itself. One frame owns that
//! proportion now: the index owns the scroll, the evidence column never
//! moves, and the row the operator chose is always beside its own footage.
use super::annotate::SubtaskRow;
use super::document::{CurationNodeKind, PopulationKind};
use super::poster::{PosterFrame, PosterFrames, PosterStill};
use super::state::CurationController;
use crate::activities::episode::{Episode, EpisodeInspection, FeaturedCamera, PlayRequest};
use crate::components::icons::FindSimilarIcon;
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Notice, NoticeTone, Place,
};
use std::fmt::{Display, Formatter, Result as FormatResult};

/// A count with the noun a person would say beside it: "1 episode",
/// "12 episodes".
///
/// It lives here rather than beside any one of its callers because the graph's
/// node cards, the index's episode headings, the run's status line and both
/// similarity nodes' coverage strips all say the same three nouns, and said them
/// three different ways.
pub(super) struct Counted(usize, Noun);

impl Counted {
    pub(super) fn episodes(count: usize) -> Self {
        Self(count, Noun::EPISODE)
    }
    pub(super) fn subtasks(count: usize) -> Self {
        Self(count, Noun::SUBTASK)
    }
    pub(super) fn categories(count: usize) -> Self {
        Self(count, Noun::CATEGORY)
    }
    pub(super) fn moments(count: usize) -> Self {
        Self(count, Noun::MOMENT)
    }
    /// A count of whatever a population of this kind holds.
    pub(super) fn of_kind(kind: Option<PopulationKind>, count: usize) -> Self {
        match kind {
            Some(PopulationKind::Subtasks) => Self::subtasks(count),
            Some(PopulationKind::Moments) => Self::moments(count),
            Some(PopulationKind::Episodes) | None => Self::episodes(count),
        }
    }
}

impl Display for Counted {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FormatResult {
        let Noun(singular, plural) = self.1;
        let word = if self.0 == 1 { singular } else { plural };
        write!(formatter, "{} {word}", self.0)
    }
}

/// A word in both its numbers, because English does not make the plural by
/// adding an `s`: "categorys" told an operator the numbers beside it were not
/// being read by anyone.
#[derive(Clone, Copy, Debug)]
struct Noun(&'static str, &'static str);

impl Noun {
    const EPISODE: Self = Self("episode", "episodes");
    const SUBTASK: Self = Self("subtask", "subtasks");
    const CATEGORY: Self = Self("category", "categories");
    const MOMENT: Self = Self("moment", "moments");
}

/// The order an index lists episodes in: the order they were measured, each
/// at its first sight so the rows of one episode stay together.
///
/// What an episode is called never depends on this order. Its name is its id,
/// so a recording reads the same here as in Data and Curation however the
/// measurement ran — retried, stopped, or partly failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EpisodeOrder(Vec<EpisodeId>);

impl EpisodeOrder {
    pub(super) fn of<'a>(episodes: impl Iterator<Item = &'a EpisodeId>) -> Self {
        let mut ordered = Vec::<EpisodeId>::new();
        for episode in episodes {
            if !ordered.contains(episode) {
                ordered.push(episode.clone());
            }
        }
        Self(ordered)
    }
    /// Where an episode falls. One this index never saw sorts after the rest.
    pub(super) fn place(&self, episode: &EpisodeId) -> usize {
        self.0
            .iter()
            .position(|seen| seen == episode)
            .unwrap_or(self.0.len())
    }
}

/// What a view needs to show any row of a measurement: where the recordings
/// live, which camera the measurement read, and the order episodes list in.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SubtaskContext {
    dataset: Option<DatasetSelection>,
    camera: Option<String>,
    order: EpisodeOrder,
}

impl SubtaskContext {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        camera: Option<String>,
        order: EpisodeOrder,
    ) -> Self {
        Self {
            dataset,
            camera,
            order,
        }
    }
    pub(super) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(super) fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub(super) fn place(&self, episode: &EpisodeId) -> usize {
        self.order.place(episode)
    }
}

/// What an index says about one row beyond its own words: what jev made of
/// a subtask, whether it is the row the evidence column is showing, and
/// whether it answers Tab for its list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RowStanding {
    /// `None` before anything has been asked about this row.
    probability: Option<f64>,
    /// A batch that came back without an answer for this row leaves it in the
    /// list, last and unscored, rather than dropping it silently.
    answered: bool,
    chosen: bool,
    /// Whether this row carries its list's tab stop. Three hundred rows meant
    /// three hundred presses of Tab to get past the index, so an index that
    /// moves focus itself marks one row and reaches the others with the
    /// arrows. A caller that does not leaves every row reachable, which is
    /// what both constructors give.
    tab_stop: bool,
}

impl RowStanding {
    /// A row nobody has scored: the annotation's own output.
    pub(super) fn plain(chosen: bool) -> Self {
        Self {
            probability: None,
            answered: true,
            chosen,
            tab_stop: true,
        }
    }
    pub(super) fn scored(probability: f64, answered: bool, chosen: bool) -> Self {
        Self {
            probability: Some(probability),
            answered,
            chosen,
            tab_stop: true,
        }
    }
    /// One row of an index answers Tab; the arrows reach the rest.
    pub(super) fn roving(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
    /// What the score column reads: a whole percentage, as the Select's
    /// threshold and every tooltip print one.
    fn label(self) -> Option<String> {
        let probability = self.probability?;
        Some(if self.answered {
            format!("{:.0}%", probability * 100.0)
        } else {
            "—".to_owned()
        })
    }
}

/// Focus movement inside one index.
///
/// An index row is the words it holds, so it cannot be a `<button>`: Chromium
/// refuses caret selection inside one, and the whole product of this view is
/// three hundred generated sentences nobody could copy. The row is therefore a
/// pressable element that carries its own arrow keys, and this is where they
/// reach the next row.
pub(super) struct IndexFocus;

impl IndexFocus {
    /// Every row of the index the focused row belongs to, whether the index is
    /// one flat list or one list per episode.
    const ROWS: &'static str = ".curation-entry";
    const INDEX: &'static str = ".curation-review-index";

    /// Move focus one row on from the focused one, wrapping at either end.
    fn step(direction: isize) {
        use wasm_bindgen::JsCast as _;

        let Some(active) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
        else {
            return;
        };
        let Ok(Some(index)) = active.closest(Self::INDEX) else {
            return;
        };
        let Ok(found) = index.query_selector_all(Self::ROWS) else {
            return;
        };
        let rows = (0..found.length())
            .filter_map(|position| found.item(position))
            .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return;
        }
        let here = rows
            .iter()
            .position(|row| active.is_same_node(Some(row.as_ref())))
            .unwrap_or_default() as isize;
        if let Some(row) = rows.get((here + direction).rem_euclid(rows.len() as isize) as usize) {
            let _ = row.focus();
        }
    }

    /// What a row does with a key press: press itself, or hand focus on.
    fn handle(event: &KeyboardEvent, on_select: EventHandler<()>) {
        match event.key() {
            Key::Enter => on_select.call(()),
            Key::Character(pressed) if pressed == " " => on_select.call(()),
            Key::ArrowDown => Self::step(1),
            Key::ArrowUp => Self::step(-1),
            _ => return,
        }
        event.prevent_default();
        event.stop_propagation();
    }
}

/// One row of an index: its poster, what jev made of it, its words, and where
/// in the dataset it happened, with the action that asks for more like it.
///
/// The same row in all three views, because it is the same thing in all three.
#[component]
pub(super) fn SubtaskEntry(
    row: SubtaskRow,
    standing: RowStanding,
    /// What to call the episode, for an index that is not already grouped by
    /// one. Grouped indexes leave it out rather than repeat their heading.
    episode: Option<String>,
    frames: PosterFrames,
    on_select: EventHandler<()>,
    /// Ask for subtasks like this one, handing over where its poster sits on
    /// screen so the question can be seen leaving it.
    #[props(default)]
    on_find_similar: Option<EventHandler<Option<Place>>>,
) -> Element {
    let mut item = use_signal(|| None::<web_sys::Element>);
    let span = format!(
        "{} – {}",
        row.start_time().clock_label(),
        row.end_time().clock_label()
    );
    let motion_key = format!("{}-{}", row.episode(), row.start_time());
    rsx! {
        li {
            class: "curation-entry-item",
            "data-motion-key": "{motion_key}",
            "data-findable": on_find_similar.is_some().then_some("true"),
            onmounted: move |event: Event<MountedData>| {
                item.set(event.data().downcast::<web_sys::Element>().cloned());
            },
            div {
                class: "curation-entry",
                role: "button",
                tabindex: if standing.tab_stop { "0" } else { "-1" },
                "aria-current": standing.chosen.then_some("true"),
                onclick: move |_| on_select.call(()),
                onkeydown: move |event: KeyboardEvent| IndexFocus::handle(&event, on_select),
                PosterFrame { still: PosterStill::of_subtask(&row), frames }
                span { class: "curation-entry-words",
                    span { class: "curation-entry-description", "{row.description()}" }
                    span { class: "curation-entry-facts",
                        if let Some(episode) = episode.clone() {
                            span { "{episode}" }
                        }
                        span { "{span}" }
                    }
                }
                if let Some(score) = standing.label() {
                    span {
                        class: "curation-entry-score",
                        "data-answered": "{standing.answered}",
                        title: if standing.answered { "How likely this is the same operation" }
                               else { "This row came back without an answer" },
                        "{score}"
                    }
                }
            }
            // Beside the row rather than in it: the row is itself a button,
            // and a button inside a button is one nobody can reach.
            if let Some(find) = on_find_similar {
                button {
                    r#type: "button",
                    class: "curation-entry-find",
                    // A tab stop only beside the row that is one, so the
                    // action does not bring back three hundred presses of Tab.
                    tabindex: if standing.tab_stop { "0" } else { "-1" },
                    title: "Find subtasks like this one",
                    "aria-label": "Find subtasks like “{row.description()}”",
                    onclick: move |_| {
                        let poster = item
                            .peek()
                            .as_ref()
                            .and_then(|item| item.query_selector(".curation-poster").ok().flatten())
                            .map(|poster| Place::of(&poster));
                        find.call(poster);
                    },
                    FindSimilarIcon {}
                }
            }
        }
    }
}

/// What a row of whole episodes or stretches is called: a recording's own
/// label, or its id, which is set as the measurement it is.
#[derive(Clone, Debug, PartialEq)]
struct SliceTitle {
    words: String,
    is_id: bool,
}

/// The small drawing in a row's facts line: an episode's length against the
/// longest listed, where a stretch sits in its episode, or a measurement's
/// values across the episode.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SliceShape {
    /// A share of the longest episode listed, 0–1.
    Length(f64),
    /// Where a stretch starts and ends, as fractions of its episode.
    Within(f64, f64),
    Trace(TraceLine),
}

/// A measurement's values across one episode, drawn small against the level
/// they are read by: where a recording slowed, stalled or went backwards,
/// before it is opened.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TraceLine {
    /// Each value at its place across the episode, 0–1; `None` where nothing
    /// was read, which breaks the line.
    points: Vec<(f64, Option<f64>)>,
    /// The values the drawing's height spans; values beyond it are drawn at
    /// its edge, so every row of an index shares one scale.
    domain: (f64, f64),
    reference: f64,
}

impl TraceLine {
    const WIDTH: f64 = 100.0;
    const HEIGHT: f64 = 20.0;

    pub(super) fn new(points: Vec<(f64, Option<f64>)>, domain: (f64, f64), reference: f64) -> Self {
        Self {
            points,
            domain,
            reference,
        }
    }
    fn y(&self, value: f64) -> f64 {
        let (low, high) = self.domain;
        let share = ((value - low) / (high - low).max(f64::EPSILON)).clamp(0.0, 1.0);
        Self::HEIGHT - 1.0 - share * (Self::HEIGHT - 2.0)
    }
    fn path(&self) -> String {
        use std::fmt::Write as _;
        let mut path = String::new();
        let mut connected = false;
        for (place, value) in &self.points {
            let Some(value) = value.filter(|value| value.is_finite()) else {
                connected = false;
                continue;
            };
            let command = if connected { 'L' } else { 'M' };
            let _ = write!(
                path,
                "{command}{:.2},{:.2} ",
                place.clamp(0.0, 1.0) * Self::WIDTH,
                self.y(value)
            );
            connected = true;
        }
        path
    }
}

/// The measure a row is ranked or kept by, and what that number is.
#[derive(Clone, Debug, PartialEq)]
struct RowMeasure {
    reading: String,
    meaning: String,
}

/// One row of an index of whole episodes or stretches, in the anatomy every
/// index row shares: a still, a name over a line of facts, and the measure.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SliceFace {
    title: SliceTitle,
    facts: Vec<String>,
    still: PosterStill,
    shape: Option<SliceShape>,
    measure: Option<RowMeasure>,
}

impl SliceFace {
    /// A whole episode: its own label when its path gives one, with the id
    /// beside its length, else its id alone.
    pub(super) fn episode(episode: &Episode, label: Option<&str>) -> Self {
        let id = Episode::listed_name_of(episode.id());
        let seconds = episode.facts().duration_seconds();
        let (title, mut facts) = match label {
            Some(label) => (
                SliceTitle {
                    words: label.to_owned(),
                    is_id: false,
                },
                vec![id],
            ),
            None => (
                SliceTitle {
                    words: id,
                    is_id: true,
                },
                Vec::new(),
            ),
        };
        facts.extend(seconds.map(|seconds| seconds.clock_label()));
        Self {
            title,
            facts,
            still: PosterStill::within(episode.id(), (0.0, seconds.unwrap_or_default())),
            shape: None,
            measure: None,
        }
    }
    /// A stretch of one episode, listed under its episode's heading: named
    /// by its span, saying how long it runs — a stretch too short to train
    /// on reads as one — and drawn where it sits in the episode's
    /// `duration`.
    pub(super) fn stretch(episode: &EpisodeId, (start, end): (f64, f64), duration: f64) -> Self {
        let duration = duration.max(f64::EPSILON);
        Self {
            title: SliceTitle {
                words: format!("{} – {}", start.clock_label(), end.clock_label()),
                is_id: true,
            },
            facts: vec![format!("{:.1} s", (end - start).max(0.0))],
            still: PosterStill::within(episode, (start, end)),
            shape: Some(SliceShape::Within(start / duration, end / duration)),
            measure: None,
        }
    }
    pub(super) fn shaped(mut self, shape: SliceShape) -> Self {
        self.shape = Some(shape);
        self
    }
    /// Replace the facts line's words, keeping its drawing.
    pub(super) fn saying(mut self, facts: Vec<String>) -> Self {
        self.facts = facts;
        self
    }
    pub(super) fn measured(mut self, reading: String, meaning: impl Into<String>) -> Self {
        self.measure = Some(RowMeasure {
            reading,
            meaning: meaning.into(),
        });
        self
    }
    /// Whether this row names its recording with the words `needle` says.
    pub(super) fn says(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        needle.is_empty()
            || self.title.words.to_lowercase().contains(&needle)
            || self
                .facts
                .iter()
                .any(|fact| fact.to_lowercase().contains(&needle))
    }
    pub(super) fn still(&self) -> &PosterStill {
        &self.still
    }
}

/// A row's facts-line drawing.
#[component]
fn SliceDrawing(shape: SliceShape) -> Element {
    match shape {
        SliceShape::Length(share) => rsx! {
            span { class: "curation-entry-extent", "aria-hidden": "true",
                i { style: "width: {share.clamp(0.0, 1.0) * 100.0:.1}%" }
            }
        },
        SliceShape::Within(left, right) => {
            let (left, right) = (left.clamp(0.0, 1.0), right.clamp(0.0, 1.0));
            rsx! {
                span { class: "curation-entry-extent", "aria-hidden": "true",
                    i { style: "left: {left * 100.0:.2}%; width: {(right - left).max(0.0) * 100.0:.2}%" }
                }
            }
        }
        SliceShape::Trace(trace) => rsx! {
            svg {
                class: "curation-entry-trace",
                "aria-hidden": "true",
                view_box: "0 0 {TraceLine::WIDTH} {TraceLine::HEIGHT}",
                preserve_aspect_ratio: "none",
                line {
                    class: "curation-entry-trace-reference",
                    x1: "0", x2: "{TraceLine::WIDTH}",
                    y1: "{trace.y(trace.reference):.2}", y2: "{trace.y(trace.reference):.2}",
                    vector_effect: "non-scaling-stroke",
                }
                path { d: "{trace.path()}", vector_effect: "non-scaling-stroke" }
            }
        },
    }
}

/// One row of an index that is not a subtask — a whole episode, or a stretch
/// of one — in the same anatomy as a subtask's row.
#[component]
pub(super) fn SliceEntry(
    face: SliceFace,
    frames: PosterFrames,
    standing: RowStanding,
    on_select: EventHandler<()>,
) -> Element {
    rsx! {
        li { class: "curation-entry-item",
            div {
                class: "curation-entry",
                role: "button",
                tabindex: if standing.tab_stop { "0" } else { "-1" },
                "aria-current": standing.chosen.then_some("true"),
                onclick: move |_| on_select.call(()),
                onkeydown: move |event: KeyboardEvent| IndexFocus::handle(&event, on_select),
                PosterFrame { still: face.still.clone(), frames }
                span { class: "curation-entry-words",
                    span {
                        class: "curation-entry-description",
                        "data-id": face.title.is_id.then_some("true"),
                        "{face.title.words}"
                    }
                    span { class: "curation-entry-facts",
                        if let Some(shape) = face.shape.clone() {
                            SliceDrawing { shape }
                        }
                        for fact in face.facts.iter() {
                            span { "{fact}" }
                        }
                    }
                }
                if let Some(measure) = face.measure.clone() {
                    span { class: "curation-entry-score", title: "{measure.meaning}", "{measure.reading}" }
                }
            }
        }
    }
}

/// The index beside the recording. The caller fills both halves; this owns
/// only which one scrolls.
#[component]
pub(super) fn SubtaskReview(index: Element, evidence: Element) -> Element {
    rsx! {
        div { class: "curation-review",
            div { class: "curation-review-index", {index} }
            aside { class: "curation-review-evidence", "aria-label": "Selected subtask", {evidence} }
        }
    }
}

/// One subtask row shown as what it is: the camera the measurement read,
/// played over that row's interval alone.
///
/// The whole recording, every camera it holds, seeking to this row's interval:
/// the same episode composition the rest of Studio shows, so an operator
/// recognizes the recording wherever they meet it.
#[component]
pub(super) fn SubtaskEvidence(
    row: SubtaskRow,
    /// The ask to play the row, counted, so choosing it again replays it.
    play: Option<PlayRequest>,
    context: SubtaskContext,
    /// Send the operator to this interval in the annotation it came from.
    on_open: EventHandler<()>,
    /// Ask for subtasks like this one; a search already about it leaves this
    /// out.
    #[props(default)]
    on_find_similar: Option<EventHandler<()>>,
    /// The episode's subtasks as the track under the recording draws them,
    /// with this row chosen.
    intervals: SubtaskIntervals,
) -> Element {
    let Some(dataset) = context.dataset().cloned() else {
        return rsx! {
            Notice { tone: NoticeTone::Caution,
                "This curation has no dataset, so the recording cannot be opened."
            }
        };
    };
    let selection = EpisodeSelection::new(dataset, row.episode().clone());
    let opened = use_resource(use_reactive(&selection, Episode::open));
    let span = (row.start_time(), row.end_time());
    rsx! {
        EvidenceHeading {
            subject: row.description().to_owned(),
            episode: Some(row.episode().clone()),
            facts: vec![format!("{} – {}", span.0.clock_label(), span.1.clock_label())],
            actions: rsx! {
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    title: "Show this interval in the annotation it came from",
                    onclick: move |_| on_open.call(()),
                    "Open in context"
                }
                if let Some(find) = on_find_similar {
                    Button {
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Link,
                        size: ButtonSize::Compact,
                        title: "Rank every other subtask by how likely it is the same operation",
                        onclick: move |_| find.call(()),
                        "Find similar"
                    }
                }
            },
        }
        match opened.read().as_ref() {
            Some(Ok(episode)) => rsx! {
                div { class: "curation-review-player",
                    EpisodeInspection {
                        episode: episode.clone(),
                        featured_camera: FeaturedCamera::measured(context.camera()),
                        tracks: vec![intervals.track().into()],
                        focus: play,
                    }
                }
            },
            Some(Err(error)) => rsx! {
                Notice { tone: NoticeTone::Danger, "Could not open this episode: {error}" }
            },
            None => rsx! {
                p { class: "curation-review-note", role: "status", "Opening the recording…" }
            },
        }
    }
}

/// What the evidence column is showing, named once, on one line: the
/// subject, the facts that place it, and at the end the actions that take it
/// elsewhere.
///
/// A subtask's episode is named by its id, cut from the front so the tail
/// every index heading shows ("…f9e3e090") is the part that stays; it is text,
/// so it can be selected and pasted. When the subject is the episode itself
/// its name already says that tail, so the id is the name's tooltip instead.
#[component]
pub(super) fn EvidenceHeading(
    subject: String,
    /// The episode the subject belongs to, when the subject is not the
    /// episode itself.
    #[props(default)]
    episode: Option<EpisodeId>,
    #[props(default)] facts: Vec<String>,
    #[props(default = VNode::empty())] actions: Element,
) -> Element {
    let names_itself = episode
        .as_ref()
        .is_some_and(|episode| Episode::name_of(episode) == subject);
    let (subject_title, id) = match episode {
        Some(episode) if names_itself => (episode.to_string(), None),
        Some(episode) => (subject.clone(), Some(episode)),
        None => (subject.clone(), None),
    };
    rsx! {
        header { class: "curation-review-head",
            p { class: "curation-review-subject", title: "{subject_title}", "{subject}" }
            p { class: "curation-review-facts",
                if let Some(episode) = id {
                    code { class: "curation-review-id", title: "{episode}", "{episode}" }
                }
                for fact in facts {
                    span { "{fact}" }
                }
            }
            div { class: "curation-review-actions", {actions} }
        }
    }
}

/// One measurement's intervals for one episode, as the timeline draws them,
/// and which of them the index has chosen.
///
/// The same track in every view that shows a subtask, so the strip under the
/// recording means one thing wherever it is met.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SubtaskIntervals {
    spans: Vec<TimelineInterval>,
    chosen: Option<f64>,
}

impl SubtaskIntervals {
    /// Every row one measurement wrote for one episode.
    pub(super) fn of(rows: &[SubtaskRow]) -> Self {
        Self::painted(rows, |_| String::new())
    }
    /// Every row, each wearing the hue `tint` lends it — the category a
    /// grouping put it in — as `--hue: <colour>`, or nothing.
    pub(super) fn painted(rows: &[SubtaskRow], tint: impl Fn(&SubtaskRow) -> String) -> Self {
        Self {
            spans: rows
                .iter()
                .map(|row| {
                    TimelineInterval::labeled(row.start_time(), row.end_time(), row.description())
                        .tinted(tint(row))
                })
                .collect(),
            chosen: None,
        }
    }
    /// Mark the row the index has chosen, so the track shows it selected too.
    pub(super) fn choosing(mut self, row: Option<&SubtaskRow>) -> Self {
        self.chosen = row.map(SubtaskRow::start_time);
        self
    }
    /// Every row of `rows` from the chosen row's episode, with the chosen one
    /// marked, so a ranked result is read in the context of what came before
    /// and after it.
    pub(super) fn around(chosen: &SubtaskRow, rows: &[SubtaskRow]) -> Self {
        Self::of(&Self::episode_of(chosen, rows)).choosing(Some(chosen))
    }
    /// `rows` of the chosen row's episode, or the chosen row alone when none
    /// are — never an empty strip under a row that plainly has an interval.
    pub(super) fn episode_of(chosen: &SubtaskRow, rows: &[SubtaskRow]) -> Vec<SubtaskRow> {
        let episode = rows
            .iter()
            .filter(|row| row.episode() == chosen.episode())
            .cloned()
            .collect::<Vec<_>>();
        if episode.is_empty() {
            vec![chosen.clone()]
        } else {
            episode
        }
    }
    /// One labelled row of the shared timeline.
    pub(super) fn track(&self) -> IntervalTrack {
        IntervalTrack::new("curation-subtasks", "Subtasks", self.spans.clone())
            .with_chosen(self.chosen)
    }
}

/// The one bar every node's workspace opens with: on the left what the node
/// reads, runs with or asks — its query, its rule, its settings — and any note
/// that needs attention, and at the far end the node's own actions. What the
/// node holds is on its card, so it is not repeated here.
#[component]
pub(super) fn NodeBar(
    #[props(default = VNode::empty())] lead: Element,
    /// What needs attention: a missing setting, failed episodes.
    #[props(default)]
    note: Option<String>,
    /// How many episodes reach the node, when fewer than all do.
    #[props(default)]
    reach: Option<(CurationController, NodeReach)>,
    #[props(default = VNode::empty())] actions: Element,
) -> Element {
    rsx! {
        div { class: "curation-node-bar",
            div { class: "curation-node-bar-lead", {lead} }
            // What the node's state is, then what can be done about it, in
            // one group at the bar's far end.
            div { class: "curation-node-bar-end",
                if let Some(note) = note {
                    p { class: "curation-node-bar-note", role: "status", "{note}" }
                }
                if let Some((controller, reach)) = reach {
                    ReachNote { controller, reach }
                }
                div { class: "curation-node-bar-actions", {actions} }
            }
        }
    }
}

/// The bar over an index, the height of the evidence heading beside it so
/// the two columns' hairlines meet: the index's own filter and sides.
#[component]
pub(super) fn IndexBar(children: Element) -> Element {
    rsx! {
        div { class: "curation-index-bar", {children} }
    }
}

/// Which row of an index answers Tab: the chosen one, else the first, so a
/// keyboard reaches the index once and the arrows carry on from there.
pub(super) struct TabStop;

impl TabStop {
    pub(super) fn at(position: usize, chosen: Option<usize>) -> bool {
        position == chosen.unwrap_or(0)
    }
}

/// How many of the curation's episodes reach a node, said only when fewer
/// than all of them do: after a Select, or after a measurement that has not
/// run on every episode yet, everything below reads only what came through,
/// and a node's own counts look complete when they are not.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NodeReach {
    reached: usize,
    sampled: usize,
    /// The nearest measurement above that has not measured every episode
    /// reaching it, and what finishing it is called: the one place to go
    /// to let the rest through.
    unfinished: Option<(String, &'static str)>,
}

impl NodeReach {
    pub(super) fn of(controller: CurationController, node: &str) -> Option<Self> {
        let sampled = controller.sample()?.episodes().len();
        let (input, _) = controller.populations(node)?;
        let reached = input.episodes().len();
        (reached < sampled).then(|| Self {
            reached,
            sampled,
            unfinished: Self::unfinished_above(controller, node),
        })
    }
    fn unfinished_above(
        controller: CurationController,
        node: &str,
    ) -> Option<(String, &'static str)> {
        let mut above =
            controller.with_payload(|payload| payload.parent_of(node).map(str::to_owned));
        while let Some(id) = above {
            let kind = controller
                .with_payload(|payload| payload.node(&id).map(|node| node.kind().clone()))?;
            let verb = match kind {
                CurationNodeKind::IdentifyUnproductiveMoments(_) => Some("Score the rest"),
                CurationNodeKind::BreakDownSubtasks(_) => Some("Break down the rest"),
                CurationNodeKind::FindSimilarSubtasks(_) => Some("Search the rest"),
                CurationNodeKind::GroupSimilarSubtasks(_) => Some("Group the rest"),
                CurationNodeKind::Dataset | CurationNodeKind::Select(_) => None,
            };
            let finished = controller
                .progress(&id)
                .is_some_and(|progress| progress.pending().is_empty());
            if let Some(verb) = verb.filter(|_| !finished) {
                return Some((id, verb));
            }
            above = controller.with_payload(|payload| payload.parent_of(&id).map(str::to_owned));
        }
        None
    }
}

/// How many episodes reach a node, beside the way to let the rest through.
#[component]
pub(super) fn ReachNote(controller: CurationController, reach: NodeReach) -> Element {
    rsx! {
        p { class: "curation-node-bar-reach",
            "{reach.reached} of {Counted::episodes(reach.sampled)} reach this node"
            if let Some((node, verb)) = reach.unfinished.clone() {
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    title: "Open the measurement the other episodes are waiting on",
                    onclick: move |_| controller.select_node(Some(node.clone())),
                    "{verb}"
                }
            }
        }
    }
}

/// What a node's bar says needs attention, in order, as one line.
pub(super) struct NodeNotes;

impl NodeNotes {
    pub(super) fn join(notes: impl IntoIterator<Item = Option<String>>) -> Option<String> {
        let notes = notes.into_iter().flatten().collect::<Vec<_>>();
        (!notes.is_empty()).then(|| notes.join(" · "))
    }
}

/// How many episodes a node could not measure, said only when some failed.
pub(super) struct FailedEpisodes;

impl FailedEpisodes {
    pub(super) fn note(failed: usize) -> Option<String> {
        match failed {
            0 => None,
            1 => Some("1 episode failed".to_owned()),
            failed => Some(format!("{failed} episodes failed")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_list_in_the_order_they_were_measured_and_unseen_ones_last() {
        let [first, second, unseen] = [3, 1, 2].map(EpisodeId::fixture);
        let order = EpisodeOrder::of([&first, &first, &second, &first].into_iter());
        assert_eq!(order.place(&first), 0);
        assert_eq!(order.place(&second), 1);
        assert_eq!(order.place(&unseen), 2);
    }

    #[test]
    fn a_count_names_what_it_counts_in_the_singular_too() {
        assert_eq!(Counted::episodes(1).to_string(), "1 episode");
        assert_eq!(Counted::episodes(39).to_string(), "39 episodes");
        assert_eq!(Counted::subtasks(0).to_string(), "0 subtasks");
        assert_eq!(Counted::subtasks(1).to_string(), "1 subtask");
        assert_eq!(Counted::categories(1).to_string(), "1 category");
        assert_eq!(Counted::categories(6).to_string(), "6 categories");
    }

    #[test]
    fn only_an_index_that_moves_focus_itself_takes_rows_out_of_the_tab_order() {
        assert!(RowStanding::plain(false).tab_stop);
        assert!(RowStanding::scored(0.9, true, false).tab_stop);
        assert!(!RowStanding::plain(false).roving(false).tab_stop);
    }
}
