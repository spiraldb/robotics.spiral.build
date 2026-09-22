//! How every measurement's output is read: an index that scrolls beside
//! a recording that holds still.
//!
//! All three output views are the same thing — a list of subtask intervals,
//! and one of them shown as video. Written separately they each pushed the
//! recording off the bottom of a growing list, so the click that plays a
//! subtask had nothing visible to show for itself. One frame owns that
//! proportion now: the index owns the scroll, the evidence column never
//! moves, and the row the operator chose is always beside its own footage.
use super::document::SubtaskRow;
use super::poster::{PosterFrame, PosterFrames};
use crate::activities::data::{
    DatasetClient, EpisodeInspection, EpisodeTransport, FeaturedCamera, OpenedEpisode,
};
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, EmptyState, Notice, NoticeTone,
};
use std::fmt::{Display, Formatter, Result as FormatResult};

/// A subtask plays to its own end and a moment past it, so the operator sees
/// the motion leave the interval rather than being cut off inside it.
///
/// There is deliberately no lead-in. The playhead landing half a second before
/// the row it was asked for put the cursor outside the highlight that marks
/// the same interval, and every reading of the clock was then half a second
/// wrong in a view whose whole product is where a subtask starts.
pub(super) const RUN_OUT_SECONDS: f64 = 0.5;

/// A count with the noun a person would say beside it: "1 episode",
/// "12 episodes".
///
/// It lives here rather than beside any one of its callers because the graph's
/// node cards, the index's episode headings, the run's status line and both
/// explorations' coverage strips all say the same three nouns, and said them
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
}

/// Episode numbers as the operator counts them.
///
/// A UUID is not a name. Explore lists recordings in recording order everywhere,
/// so a row reads "Episode 7" and keeps its identifier in the tooltip for the
/// one moment somebody needs to paste it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EpisodeNames(Vec<String>);

impl EpisodeNames {
    /// The episodes in the order they were measured, keeping the first sight
    /// of each so repeated rows of one episode all read the same number.
    pub(super) fn of<'a>(episodes: impl Iterator<Item = &'a str>) -> Self {
        let mut ordered = Vec::<String>::new();
        for episode in episodes {
            if !ordered.iter().any(|seen| seen == episode) {
                ordered.push(episode.to_owned());
            }
        }
        Self(ordered)
    }
    /// Where an episode falls in numbering order. One this index never saw
    /// sorts after every numbered episode, where its name already puts it.
    pub(super) fn place(&self, episode: &str) -> usize {
        self.0
            .iter()
            .position(|seen| seen == episode)
            .unwrap_or(self.0.len())
    }
    /// What to call one episode. An episode this index never saw keeps the
    /// *tail* of its identifier rather than a number that would mean nothing:
    /// these are time-ordered ULIDs, so every recording of one dataset shares
    /// its head and only the tail tells them apart.
    pub(super) fn name(&self, episode: &str) -> String {
        match self.0.iter().position(|seen| seen == episode) {
            Some(index) => format!("Episode {}", index + 1),
            None => format!(
                "…{}",
                episode
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
        }
    }
}

/// What a view needs to show any row of a measurement: where the recordings
/// live, which camera the measurement read, and what the episodes are called.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SubtaskContext {
    dataset: Option<DatasetSelection>,
    camera: Option<String>,
    names: EpisodeNames,
}

impl SubtaskContext {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        camera: Option<String>,
        names: EpisodeNames,
    ) -> Self {
        Self {
            dataset,
            camera,
            names,
        }
    }
    pub(super) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(super) fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub(super) fn name(&self, episode: &str) -> String {
        self.names.name(episode)
    }
    pub(super) fn place(&self, episode: &str) -> usize {
        self.names.place(episode)
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
    const ROWS: &'static str = ".explore-entry";
    const INDEX: &'static str = ".explore-review-index";

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
/// in the dataset it happened.
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
) -> Element {
    let span = format!(
        "{} – {}",
        row.start_time().clock_label(),
        row.end_time().clock_label()
    );
    rsx! {
        li { class: "explore-entry-item",
            div {
                class: "explore-entry",
                role: "button",
                tabindex: if standing.tab_stop { "0" } else { "-1" },
                "aria-current": standing.chosen.then_some("true"),
                onclick: move |_| on_select.call(()),
                onkeydown: move |event: KeyboardEvent| IndexFocus::handle(&event, on_select),
                PosterFrame { row: row.clone(), frames }
                span { class: "explore-entry-words",
                    span { class: "explore-entry-description", "{row.description()}" }
                    span { class: "explore-entry-facts",
                        if let Some(episode) = episode.clone() {
                            span { "{episode}" }
                        }
                        span { "{span}" }
                    }
                }
                if let Some(score) = standing.label() {
                    span {
                        class: "explore-entry-score",
                        "data-answered": "{standing.answered}",
                        title: if standing.answered { "How likely jev thinks this is the same operation" }
                               else { "This row came back without an answer" },
                        "{score}"
                    }
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
        div { class: "explore-review",
            div { class: "explore-review-index", {index} }
            aside { class: "explore-review-evidence", "aria-label": "Selected subtask", {evidence} }
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
                "This exploration has no dataset, so the recording cannot be opened."
            }
        };
    };
    let episode = row.episode().to_owned();
    let opened = use_resource(use_reactive(
        (&dataset, &episode),
        move |(dataset, episode)| async move { open_episode(dataset, episode).await },
    ));
    let span = (row.start_time(), row.end_time());
    rsx! {
        header { class: "explore-review-head",
            p { class: "explore-review-subject", "{row.description()}" }
            p { class: "explore-review-facts",
                span { "{context.name(row.episode())}" }
                span { "{span.0.clock_label()} – {span.1.clock_label()}" }
                code { class: "explore-review-id", "{row.episode()}" }
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    title: "Show this interval in the annotation it came from",
                    onclick: move |_| on_open.call(()),
                    "Open in context"
                }
            }
        }
        match opened.read().as_ref() {
            Some(Ok(episode)) => rsx! {
                div { class: "explore-review-player",
                    PlayedEpisode {
                        key: "{episode.title()}",
                        episode: episode.clone(),
                        slice: Some(MeasuredSlice {
                            span,
                            camera: context.camera().map(str::to_owned),
                            subtasks: SubtaskIntervals::of(&row, &episode_rows),
                        }),
                    }
                }
            },
            Some(Err(error)) => rsx! {
                Notice { tone: NoticeTone::Danger, "Could not open this episode: {error}" }
            },
            None => rsx! {
                p { class: "explore-review-note", role: "status", "Opening the recording…" }
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
    /// The episode's rows when the caller has them, and the chosen row alone
    /// when it does not — never an empty strip under a row that plainly has
    /// an interval.
    fn of(chosen: &SubtaskRow, episode_rows: &[SubtaskRow]) -> Self {
        let rows = if episode_rows.is_empty() {
            std::slice::from_ref(chosen)
        } else {
            episode_rows
        };
        Self(
            rows.iter()
                .map(|row| {
                    TimelineInterval::labeled(row.start_time(), row.end_time(), row.description())
                })
                .collect(),
        )
    }
    /// One labelled row of the shared timeline.
    pub(super) fn track(&self) -> IntervalTrack {
        IntervalTrack::new("explore-subtasks", "Subtasks", self.0.clone())
    }
}

/// The slice of a recording the evidence column plays: where it starts and
/// ends, the camera the measurement read it from, and the measurement's own
/// intervals for that episode.
#[derive(Clone, PartialEq)]
pub(super) struct MeasuredSlice {
    span: (f64, f64),
    camera: Option<String>,
    subtasks: SubtaskIntervals,
}

/// One recording on the shared player, which is how every surface in Explore
/// shows an episode: the dataset node watching one, and every measurement's
/// evidence column showing the row it was asked about.
///
/// What a measurement adds is a slice — where the playhead lands, which camera
/// leads, and the measurement's own intervals as a track under the recording.
/// Seeking follows the slice, so choosing another row moves the playhead
/// without remaking the player.
#[component]
pub(super) fn PlayedEpisode(
    episode: OpenedEpisode,
    /// What a measurement is showing in this recording. A caller simply
    /// watching the episode leaves it out: the whole recording, every camera
    /// it holds, no track.
    #[props(default)]
    slice: Option<MeasuredSlice>,
    /// Which camera leads when no measurement names one, such as the camera
    /// the exploration is configured on.
    #[props(default)]
    featured_camera: FeaturedCamera,
) -> Element {
    let transport = EpisodeTransport::use_opened_episode(&episode);
    let mut playing = transport.clone();
    let span = slice.as_ref().map(|slice| slice.span);
    use_effect(use_reactive(&span, move |span| {
        if let Some((start, end)) = span {
            playing.play_interval(start, end + RUN_OUT_SECONDS);
        }
    }));
    let activated = transport.clone();
    let track = slice
        .as_ref()
        .map(|slice| slice.subtasks.track())
        .filter(|track| !track.is_empty());
    rsx! {
        EpisodeInspection {
            episode,
            transport,
            featured_camera: slice
                .and_then(|slice| slice.camera)
                .map_or(featured_camera, FeaturedCamera::Named),
            span_tracks: track.map(|track| vec![track]).unwrap_or_default(),
            on_interval: move |interval: TimelineInterval| {
                let (start, end) = interval.span();
                activated.clone().play_interval(start, end + RUN_OUT_SECONDS);
            },
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

/// The episode identity the shared player needs, read for one episode.
async fn open_episode(dataset: DatasetSelection, episode: String) -> Result<OpenedEpisode, String> {
    let sql = format!(
        "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
         message_start_time, message_end_time, duration_ns, channel_count \
         FROM spql.mcap.episodes('{}') WHERE id = $1",
        dataset.sql_name().replace('\'', "''")
    );
    let request = EpisodeQueryRequest::new(EpisodeSelection::new(dataset.clone(), episode), sql, 1);
    let row = DatasetClient::current()
        .run_episode_query(&request)
        .await
        .map_err(|error| error.to_string())?
        .into_rows()
        .into_iter()
        .next()
        .ok_or("This subtask's episode is no longer in the dataset.")?;
    OpenedEpisode::from_row(dataset, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_are_numbered_in_the_order_they_were_measured() {
        let names = EpisodeNames::of(["ep-a", "ep-a", "ep-b", "ep-a"].into_iter());
        assert_eq!(names.name("ep-a"), "Episode 1");
        assert_eq!(names.name("ep-b"), "Episode 2");
    }

    #[test]
    fn an_episode_outside_the_index_keeps_the_tail_of_its_identifier() {
        // Every recording of one dataset shares the head of its ULID, so the
        // head names nothing; the tail is what differs.
        let names = EpisodeNames::of(["ep-a"].into_iter());
        assert_eq!(names.name("01a0989fb65870a7"), "…5870a7");
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
