//! How every measurement's output is read: an index that scrolls beside
//! a recording that holds still.
//!
//! All three output views are the same thing — a list of subtask intervals,
//! and one of them shown as video. Written separately they each pushed the
//! recording off the bottom of a growing list, so the click that plays a
//! subtask had nothing visible to show for itself. One frame owns that
//! proportion now: the index owns the scroll, the evidence column never
//! moves, and the row the operator chose is always beside its own footage.
use super::document::{PopulationKind, SubtaskRow};
use super::poster::{PosterFrame, PosterFrames};
use crate::activities::episode::{Episode, EpisodeInspection, FeaturedCamera, PlayRequest};
use crate::components::icons::FindSimilarIcon;
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, EmptyState, Notice, NoticeTone, Place,
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
/// it, whether it is the row the evidence column is showing, and whether it
/// answers Tab for its list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SubtaskStanding {
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

impl SubtaskStanding {
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
    /// What the score column reads. One decimal is as much as a calibrated
    /// probability earns.
    fn label(self) -> Option<String> {
        let probability = self.probability?;
        Some(if self.answered {
            format!("{:.1}%", probability * 100.0)
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
    standing: SubtaskStanding,
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
                PosterFrame { row: row.clone(), frames }
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
                        title: if standing.answered { "How likely jev thinks this is the same operation" }
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

/// One row of an index that is not a subtask — a whole episode, or a stretch
/// of one — in the same anatomy as a subtask's row: where it sits in its
/// episode in place of a poster, its name and facts, and its measure.
#[component]
pub(super) fn SliceEntry(
    title: String,
    facts: Vec<String>,
    /// Where the slice sits in its episode, as fractions of its length.
    place: (f64, f64),
    /// The measure the row is kept or left out by, as its column reads it.
    #[props(default)]
    score: Option<String>,
    chosen: bool,
    on_select: EventHandler<()>,
) -> Element {
    let (left, right) = (place.0.clamp(0.0, 1.0), place.1.clamp(0.0, 1.0));
    rsx! {
        li { class: "curation-entry-item",
            div {
                class: "curation-entry",
                role: "button",
                tabindex: "0",
                "aria-current": chosen.then_some("true"),
                onclick: move |_| on_select.call(()),
                onkeydown: move |event: KeyboardEvent| IndexFocus::handle(&event, on_select),
                span { class: "curation-entry-span", "aria-hidden": "true",
                    span {
                        class: "curation-entry-span-fill",
                        style: "left: {left * 100.0:.2}%; width: {(right - left).max(0.0) * 100.0:.2}%",
                    }
                }
                span { class: "curation-entry-words",
                    span { class: "curation-entry-description", "{title}" }
                    span { class: "curation-entry-facts",
                        for fact in facts {
                            span { "{fact}" }
                        }
                    }
                }
                if let Some(score) = score {
                    span { class: "curation-entry-score", "{score}" }
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
    context: SubtaskContext,
    /// Send the operator to this interval in the annotation it came from.
    on_open: EventHandler<()>,
    /// Ask for subtasks like this one; a search already about it leaves this
    /// out.
    #[props(default)]
    on_find_similar: Option<EventHandler<()>>,
    /// Every row the measurement wrote for this row's episode, drawn as one
    /// track under the recording so a ranked result is read in the context of
    /// what came before and after it. A caller that has only the one row
    /// leaves this out and the track carries that row alone.
    #[props(default)]
    episode_rows: Vec<SubtaskRow>,
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
        header { class: "curation-review-head",
            p { class: "curation-review-subject", "{row.description()}" }
            p { class: "curation-review-facts",
                span { "{Episode::name_of(row.episode())}" }
                span { "{span.0.clock_label()} – {span.1.clock_label()}" }
                code { class: "curation-review-id", "{row.episode()}" }
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
            }
        }
        match opened.read().as_ref() {
            Some(Ok(episode)) => rsx! {
                div { class: "curation-review-player",
                    EpisodeInspection {
                        episode: episode.clone(),
                        featured_camera: FeaturedCamera::measured(context.camera()),
                        tracks: vec![SubtaskIntervals::around(&row, &episode_rows).track().into()],
                        focus: Some(PlayRequest::new(span)),
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

/// One measurement's intervals for one episode, as the timeline draws them.
///
/// The same track in every view that shows a subtask, so the strip under the
/// recording means one thing wherever it is met.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SubtaskIntervals(Vec<TimelineInterval>);

impl SubtaskIntervals {
    /// Every row one measurement wrote for one episode.
    pub(super) fn of(rows: &[SubtaskRow]) -> Self {
        Self(
            rows.iter()
                .map(|row| {
                    TimelineInterval::labeled(row.start_time(), row.end_time(), row.description())
                })
                .collect(),
        )
    }
    /// The episode's rows when the caller has them, and the chosen row alone
    /// when it does not — never an empty strip under a row that plainly has
    /// an interval.
    fn around(chosen: &SubtaskRow, episode_rows: &[SubtaskRow]) -> Self {
        if episode_rows.is_empty() {
            Self::of(std::slice::from_ref(chosen))
        } else {
            Self::of(episode_rows)
        }
    }
    /// One labelled row of the shared timeline.
    pub(super) fn track(&self) -> IntervalTrack {
        IntervalTrack::new("curation-subtasks", "Subtasks", self.0.clone())
    }
}

/// One line: what a measurement's run has reached, what it runs with, and the
/// actions that change either. Nothing else sits between the workspace tab
/// and the index.
#[component]
pub(super) fn MeasurementStatus(facts: String, note: Option<String>, actions: Element) -> Element {
    rsx! {
        div { class: "curation-review-status",
            p { class: "curation-review-status-facts", role: "status", "{facts}" }
            if let Some(note) = note {
                p { class: "curation-review-status-note", "{note}" }
            }
            div { class: "curation-review-status-actions", {actions} }
        }
    }
}

/// Nothing chosen yet: say what choosing one does rather than leaving the
/// column blank beside a full list.
#[component]
pub(super) fn NoSubtaskChosen(prompt: String) -> Element {
    rsx! {
        EmptyState { title: "Nothing selected", p { "{prompt}" } }
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
        assert!(SubtaskStanding::plain(false).tab_stop);
        assert!(SubtaskStanding::scored(0.9, true, false).tab_stop);
        assert!(!SubtaskStanding::plain(false).roving(false).tab_stop);
    }
}
