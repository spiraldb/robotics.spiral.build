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
use super::episode_tabs::OpenEpisodes;
use super::state::CurationController;
use super::thumbnail::{Thumbnail, ThumbnailMoment, Thumbnails};
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, EpisodeRows, FeaturedCamera, HeadingSubject,
    PlayRequest, RowFace, RowFaceView,
};
use crate::components::icons::FindSimilarIcon;
use crate::components::page_keys::PageKeys;
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

/// The keys a list of subtask rows is keyed by: episode and start, which tell
/// one node's rows apart, since Break down's subtasks never overlap and stay
/// apart when cut to the stretches that reached them. A repeat still gets a
/// count after it, because two siblings sharing a key stop the page.
pub(super) struct RowKeys;

impl RowKeys {
    pub(super) fn of<'a>(rows: impl IntoIterator<Item = &'a SubtaskRow>) -> Vec<String> {
        let mut seen = std::collections::HashMap::<String, usize>::new();
        rows.into_iter()
            .map(|row| {
                let key = format!("{}-{}", row.episode(), row.start_time());
                let repeats = seen.entry(key.clone()).or_default();
                *repeats += 1;
                match *repeats {
                    1 => key,
                    repeat => format!("{key}~{repeat}"),
                }
            })
            .collect()
    }
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

/// What an index says about one row beyond its own words: what the model made
/// of a subtask, whether it is the row the evidence column is showing, and
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

    /// J and K open the next and previous row of the index on screen, as they
    /// step Data's episodes, and `/` goes to its filter: the keys answer
    /// wherever focus was left, while Curation is in front.
    fn use_page_keys() {
        let activity = crate::shell::activity::WorkspaceActivity::current();
        let tabs = OpenEpisodes::use_around();
        PageKeys::use_listener(move |event| {
            if !activity.is_active() || !PageKeys::is_shortcut(event) {
                return false;
            }
            let step = match event.key().as_str() {
                "j" | "J" => 1,
                "k" | "K" => -1,
                "/" => return Self::focus_filter(),
                _ => return false,
            };
            // The row steps the tab in front on rather than opening another.
            if let Some(tabs) = tabs {
                tabs.step_next();
            }
            Self::open_adjacent(step)
        });
    }

    /// Open the row `step` rows on from the chosen one — the first row when
    /// none is chosen — and bring it into view. At either end the key is
    /// still spent, so it does not fall through to the page.
    fn open_adjacent(step: isize) -> bool {
        use wasm_bindgen::JsCast as _;

        let Some(index) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.query_selector(Self::INDEX).ok().flatten())
        else {
            return false;
        };
        let Ok(found) = index.query_selector_all(Self::ROWS) else {
            return false;
        };
        let rows = (0..found.length())
            .filter_map(|position| found.item(position))
            .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
            .collect::<Vec<_>>();
        let chosen = rows
            .iter()
            .position(|row| row.get_attribute("aria-current").as_deref() == Some("true"));
        let next = match chosen {
            Some(here) => here
                .checked_add_signed(step)
                .filter(|next| *next < rows.len()),
            None => (!rows.is_empty()).then_some(0),
        };
        if let Some(row) = next.and_then(|next| rows.get(next)) {
            row.click();
            Self::reveal(&index, row);
        }
        true
    }

    /// Scroll the index, and only the index, until `row` is inside it. The
    /// browser's own `scrollIntoView` also scrolls every ancestor that hides
    /// its overflow, which slid the whole workspace out from under its bars.
    fn reveal(index: &web_sys::Element, row: &web_sys::HtmlElement) {
        let (frame, place) = (
            index.get_bounding_client_rect(),
            row.get_bounding_client_rect(),
        );
        // The bar over the index is sticky, so a row under it is hidden.
        let covered = index
            .query_selector(".curation-index-bar")
            .ok()
            .flatten()
            .map_or(0.0, |bar| bar.get_bounding_client_rect().height());
        let above = place.top() - (frame.top() + covered);
        let below = place.bottom() - frame.bottom();
        let scroll = f64::from(index.scroll_top());
        if above < 0.0 {
            index.set_scroll_top((scroll + above).round() as i32);
        } else if below > 0.0 {
            index.set_scroll_top((scroll + below).round() as i32);
        }
    }

    /// Put the caret in the index's filter, with what it says selected.
    fn focus_filter() -> bool {
        use wasm_bindgen::JsCast as _;

        let filter = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| {
                document
                    .query_selector(".curation-review-index .curation-index-bar input")
                    .ok()
                    .flatten()
            })
            .and_then(|input| input.dyn_into::<web_sys::HtmlElement>().ok());
        let Some(filter) = filter else {
            return false;
        };
        let _ = filter.focus();
        // `select` is the input's own; reached by name, it needs no binding.
        if let Ok(select) = js_sys::Reflect::get(&filter, &"select".into())
            .and_then(|select| select.dyn_into::<js_sys::Function>())
        {
            let _ = select.call0(&filter);
        }
        true
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

/// One row of an index: its thumbnail, what the model made of it, its words, and
/// where in the dataset it happened, with the action that asks for more like it.
///
/// The same row in all three views, because it is the same thing in all three.
#[component]
pub(super) fn SubtaskEntry(
    row: SubtaskRow,
    standing: RowStanding,
    /// What to call the episode, for an index that is not already grouped by
    /// one. Grouped indexes leave it out rather than repeat their heading.
    episode: Option<String>,
    thumbnails: Thumbnails,
    on_select: EventHandler<()>,
    /// Ask for subtasks like this one, handing over where its thumbnail sits on
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
    rsx! {
        li {
            class: "curation-entry-item",
            "data-findable": on_find_similar.is_some().then_some("true"),
            // A row that names its episode has two facts, which wrap to a
            // second line in a narrow index, so every row of its index is
            // drawn that tall.
            "data-names-episode": episode.is_some().then_some("true"),
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
                Thumbnail { moment: ThumbnailMoment::of_subtask(&row), thumbnails }
                span { class: "episode-row-words",
                    span { class: "episode-row-title", "{row.description()}" }
                    span { class: "episode-row-facts",
                        if let Some(episode) = episode.clone() {
                            span { class: "ui-sr-only", ", " }
                            span { "{episode}" }
                        }
                        span { class: "ui-sr-only", ", " }
                        span { "{span}" }
                    }
                }
                if let Some(score) = standing.label() {
                    span { class: "episode-row-measures",
                        span {
                            class: "episode-row-measure",
                            "data-answered": "{standing.answered}",
                            title: if standing.answered { "How likely this is the same operation" }
                                   else { "This row came back without an answer" },
                            "{score}"
                        }
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
                        let thumbnail = item
                            .peek()
                            .as_ref()
                            .and_then(|item| item.query_selector(".episode-still").ok().flatten())
                            .map(|thumbnail| Place::of(&thumbnail));
                        find.call(thumbnail);
                    },
                    FindSimilarIcon {}
                }
            }
        }
    }
}

/// One row of an index that is not a subtask — a whole episode, or a stretch
/// of one — as every list of recordings words it, and the frame its thumbnail
/// is.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SliceFace {
    face: RowFace,
    thumbnail: ThumbnailMoment,
}

impl SliceFace {
    /// A whole episode, as `rows` words it — the row Data's rail gives it.
    pub(super) fn episode(rows: &EpisodeRows, episode: &Episode) -> Self {
        Self {
            face: rows.face(episode),
            thumbnail: ThumbnailMoment::opening(episode.id()),
        }
    }
    /// A stretch of one episode, listed under its episode's heading.
    pub(super) fn stretch(episode: &EpisodeId, span: (f64, f64), duration: f64) -> Self {
        Self {
            face: RowFace::span(span, duration),
            thumbnail: ThumbnailMoment::of_stretch(episode, span),
        }
    }
    /// The episode's length under the measures the list is about.
    pub(super) fn length_last(mut self) -> Self {
        self.face = self.face.length_last();
        self
    }
    pub(super) fn measured(mut self, reading: String, meaning: impl Into<String>) -> Self {
        self.face = self.face.measured(reading, meaning);
        self
    }
    /// One more measure, `failing` when it is what leaves the row out.
    pub(super) fn judged(mut self, reading: String, meaning: &str, failing: bool) -> Self {
        self.face = self.face.judged(reading, meaning, failing);
        self
    }
}

/// One row of an index that is not a subtask — a whole episode, or a stretch
/// of one — in the same anatomy as a subtask's row.
#[component]
pub(super) fn SliceEntry(
    face: SliceFace,
    thumbnails: Thumbnails,
    standing: RowStanding,
    on_select: EventHandler<()>,
) -> Element {
    let SliceFace { face, thumbnail } = face;
    rsx! {
        li { class: "curation-entry-item",
            div {
                class: "curation-entry",
                role: "button",
                tabindex: if standing.tab_stop { "0" } else { "-1" },
                "aria-current": standing.chosen.then_some("true"),
                onclick: move |_| on_select.call(()),
                onkeydown: move |event: KeyboardEvent| IndexFocus::handle(&event, on_select),
                RowFaceView { face, still: rsx! { Thumbnail { moment: thumbnail, thumbnails } } }
            }
        }
    }
}

/// The index beside the recording. The caller fills both halves; this owns
/// only which one scrolls.
#[component]
pub(super) fn SubtaskReview(index: Element, evidence: Element) -> Element {
    IndexFocus::use_page_keys();
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
        EpisodeHeading {
            subject: HeadingSubject::part(row.description(), row.episode())
                .with_facts(vec![format!("{} – {}", span.0.clock_label(), span.1.clock_label())]),
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
                CurationNodeKind::ScoreProgress(_) => Some("Score the rest"),
                CurationNodeKind::BreakDownSubtasks(_) => Some("Break down the rest"),
                CurationNodeKind::FindSimilarSubtasks(_) => Some("Search the rest"),
                CurationNodeKind::GroupSimilarSubtasks(_) => Some("Group the rest"),
                CurationNodeKind::Dataset | CurationNodeKind::Select => None,
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
/// With a way the count is its value alone ("0 of 100 episodes · Break down
/// the rest"), so the bar keeps one line beside a query; the sentence it
/// stands for is its title.
#[component]
pub(super) fn ReachNote(controller: CurationController, reach: NodeReach) -> Element {
    let count = format!("{} of {}", reach.reached, Counted::episodes(reach.sampled));
    let sentence = format!("{count} reach this node");
    rsx! {
        p { class: "curation-node-bar-reach",
            match reach.unfinished.clone() {
                Some((node, verb)) => rsx! {
                    span { title: "{sentence}", "{count}" }
                    Button {
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Link,
                        size: ButtonSize::Compact,
                        title: "Open the measurement the other episodes are waiting on",
                        onclick: move |_| controller.select_node(Some(node.clone())),
                        "{verb}"
                    }
                },
                None => rsx! { "{sentence}" },
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
    fn rows_sharing_an_episode_and_a_start_get_keys_of_their_own() {
        let row = |start: f64, end: f64| {
            SubtaskRow::new(EpisodeId::fixture(1), (start, end), "fold").unwrap()
        };
        let keys = RowKeys::of(&[row(0.0, 2.0), row(0.0, 3.0), row(1.0, 2.0), row(0.0, 4.0)]);
        let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 4, "{keys:?}");
        assert_eq!(keys[2], format!("{}-1", EpisodeId::fixture(1)));
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
