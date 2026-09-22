//! What one annotation node produced: the index of intervals it wrote, and
//! the episode one of them belongs to, playing beside it.
use super::episodes::AnnotationEpisode;
use crate::activities::data::{EpisodeInspection, EpisodeTransport, FeaturedCamera};
use crate::activities::workflow::explore::document::{EpisodeFailure, SubtaskRow};
use crate::activities::workflow::explore::poster::PosterFrames;
use crate::activities::workflow::explore::review::{
    Counted, RUN_OUT_SECONDS, SubtaskContext, SubtaskEntry, SubtaskStanding,
};
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, Select, TextInput,
    TextInputKind,
};

/// One episode's rows, in time order.
#[derive(Clone, PartialEq)]
pub(super) struct EpisodeSubtasks {
    episode: String,
    rows: Vec<SubtaskRow>,
}

impl EpisodeSubtasks {
    /// Group an annotation node's rows by episode, keeping the order the
    /// document stores them in.
    pub(super) fn group(rows: impl Iterator<Item = SubtaskRow>) -> Vec<Self> {
        let mut grouped = Vec::<Self>::new();
        for row in rows {
            match grouped
                .iter_mut()
                .find(|group| group.episode == row.episode())
            {
                Some(group) => group.rows.push(row),
                None => grouped.push(Self {
                    episode: row.episode().to_owned(),
                    rows: vec![row],
                }),
            }
        }
        grouped
    }
    /// An episode the node has no rows for, so the player still opens.
    pub(super) fn none(episode: &str) -> Self {
        Self {
            episode: episode.to_owned(),
            rows: Vec::new(),
        }
    }
    pub(super) fn episode(&self) -> &str {
        &self.episode
    }
    pub(super) fn rows(&self) -> &[SubtaskRow] {
        &self.rows
    }
    /// This episode carrying only the rows whose description says `wanted`,
    /// which must already be lowercase. An episode none of whose rows say it
    /// leaves the index entirely.
    fn with_rows_saying(&self, wanted: &str) -> Option<Self> {
        let rows = self
            .rows
            .iter()
            .filter(|row| row.description().to_lowercase().contains(wanted))
            .cloned()
            .collect::<Vec<_>>();
        (!rows.is_empty()).then(|| Self {
            episode: self.episode.clone(),
            rows,
        })
    }
    /// The row the operator chose, out of this episode's own.
    fn chosen_row(&self, chosen: &ChosenSubtask) -> Option<&SubtaskRow> {
        self.rows.iter().find(|row| chosen.holds(row))
    }
    /// The subtasks as one labeled timeline row.
    fn track(&self) -> IntervalTrack {
        IntervalTrack::new(
            "explore-annotate-subtasks",
            "Subtasks",
            self.rows
                .iter()
                .map(|row| {
                    TimelineInterval::labeled(row.start_time(), row.end_time(), row.description())
                })
                .collect(),
        )
    }
}

/// One ask to play an interval, and which ask it is.
///
/// The count is what makes two asks different values. An interval alone made
/// pressing the same row twice indistinguishable from pressing it once, and
/// the player — which only ever reacts to a change — sat still the second
/// time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayRequest {
    interval: (f64, f64),
    asked: usize,
}

/// Which interval the operator is looking at: the episode its player shows,
/// and the row inside it, if they picked one.
///
/// Both halves of the review read this, so the index and the recording can
/// never disagree about what is on screen.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ChosenSubtask {
    episode: Signal<Option<String>>,
    /// The chosen row's start time, which is what identifies a row within its
    /// episode everywhere else in Explore.
    start: Signal<Option<f64>>,
    /// The interval the player has been asked to play.
    play: Signal<Option<PlayRequest>>,
}

impl ChosenSubtask {
    pub(super) fn use_chosen() -> Self {
        Self {
            episode: use_signal(|| None),
            start: use_signal(|| None),
            play: use_signal(|| None),
        }
    }
    pub(super) fn episode(&self) -> Option<String> {
        self.episode.read().clone()
    }
    /// Show one whole episode without playing any particular interval, as the
    /// index does when its heading is pressed. The pending ask goes with it,
    /// so opening an episode never replays the interval of the last one.
    pub(super) fn show(mut self, episode: &str) {
        self.episode.set(Some(episode.to_owned()));
        self.start.set(None);
        self.play.set(None);
    }
    /// Show one row and play it.
    pub(super) fn play_row(mut self, row: &SubtaskRow) {
        self.episode.set(Some(row.episode().to_owned()));
        self.start.set(Some(row.start_time()));
        self.ask((row.start_time(), row.end_time()));
    }
    /// Show an episode and, when an exploration named one, its interval.
    pub(super) fn focus(mut self, episode: &str, interval: Option<(f64, f64)>) {
        self.episode.set(Some(episode.to_owned()));
        self.start.set(interval.map(|(start, _)| start));
        match interval {
            Some(interval) => self.ask(interval),
            None => self.play.set(None),
        }
    }
    fn ask(mut self, interval: (f64, f64)) {
        let asked = self.play.peek().as_ref().map_or(0, |last| last.asked + 1);
        self.play.set(Some(PlayRequest { interval, asked }));
    }
    fn holds(&self, row: &SubtaskRow) -> bool {
        self.episode.read().as_deref() == Some(row.episode())
            && *self.start.read() == Some(row.start_time())
    }
    /// Whether no row at all is chosen, in which case the index's first row
    /// carries its tab stop: a roving list always answers Tab once.
    fn holds_no_row(&self) -> bool {
        self.start.read().is_none()
    }
    /// The interval the player has been asked for, read rather than taken.
    ///
    /// Taking it was the whole bug: `Signal::write` registers no subscription,
    /// so the effect that took the ask never learned that a new one had
    /// arrived, and only ran again when the episode changed underneath it.
    fn requested(&self) -> Option<PlayRequest> {
        *self.play.read()
    }
}

/// What one annotation node produced, as its index shows it: the rows it
/// wrote, the episodes it could not read, and what to call them.
///
/// Failure travels with success because it belongs in the same list: an
/// episode named only in a band above the index had no heading, no row, and no
/// way to look at the second it went wrong.
#[derive(Clone, PartialEq)]
pub(super) struct AnnotationOutput {
    groups: Vec<EpisodeSubtasks>,
    failures: Vec<EpisodeFailure>,
    context: SubtaskContext,
}

/// One place in the index: an episode with rows, or one nobody could read.
/// Failure sits among the episodes in numeric order, because an operator
/// scanning by number looks for it where it belongs.
#[derive(Clone, PartialEq)]
pub(super) enum IndexEntry {
    Measured(EpisodeSubtasks),
    Failed(EpisodeFailure),
}

impl IndexEntry {
    fn episode(&self) -> &str {
        match self {
            Self::Measured(group) => group.episode(),
            Self::Failed(failure) => failure.episode(),
        }
    }
    /// The first row this place offers, which is nothing at all for a failure.
    fn first_row(&self) -> Option<&SubtaskRow> {
        match self {
            Self::Measured(group) => group.rows().first(),
            Self::Failed(_) => None,
        }
    }
}

impl AnnotationOutput {
    pub(super) fn new(
        groups: Vec<EpisodeSubtasks>,
        failures: Vec<EpisodeFailure>,
        context: SubtaskContext,
    ) -> Self {
        Self {
            groups,
            failures,
            context,
        }
    }
    /// The episodes the index lists, in the order it lists them.
    fn jumps(&self) -> Vec<(String, String)> {
        self.matching("")
            .iter()
            .map(|entry| {
                (
                    entry.episode().to_owned(),
                    self.context.name(entry.episode()),
                )
            })
            .collect()
    }
    /// The index as one list in episode-number order: the groups whose rows
    /// say `wanted` carrying only those rows, and the failures whose episode
    /// or reason says it. An empty filter keeps everything.
    fn matching(&self, wanted: &str) -> Vec<IndexEntry> {
        let wanted = wanted.trim().to_lowercase();
        let measured = self
            .groups
            .iter()
            .filter_map(|group| {
                if wanted.is_empty() {
                    Some(group.clone())
                } else {
                    group.with_rows_saying(&wanted)
                }
            })
            .map(IndexEntry::Measured);
        let failed = self
            .failures
            .iter()
            .filter(|failure| {
                wanted.is_empty()
                    || failure.reason().to_lowercase().contains(&wanted)
                    || self
                        .context
                        .name(failure.episode())
                        .to_lowercase()
                        .contains(&wanted)
            })
            .cloned()
            .map(IndexEntry::Failed);
        let mut entries = measured.chain(failed).collect::<Vec<_>>();
        entries.sort_by_key(|entry| self.context.place(entry.episode()));
        entries
    }
    fn rows(&self) -> usize {
        self.groups.iter().map(|group| group.rows.len()).sum()
    }
}

/// Every annotated interval, grouped under the episode it came from, with the
/// episodes that failed listed among them.
#[component]
pub(super) fn SubtaskIndex(
    output: AnnotationOutput,
    frames: PosterFrames,
    chosen: ChosenSubtask,
    /// Annotate one failed episode again.
    on_retry: EventHandler<String>,
) -> Element {
    let mut wanted = use_signal(String::new);
    let filter = wanted();
    let entries = output.matching(&filter);
    // One tab stop at rest, so a keyboard reaches the index at all; the arrows
    // carry on from whichever row it lands in.
    let resting_stop = chosen
        .holds_no_row()
        .then(|| entries.iter().find_map(IndexEntry::first_row).cloned())
        .flatten();
    if output.rows() == 0 && output.failures.is_empty() {
        return rsx! {
            p { class: "explore-annotate-empty", role: "status",
                "No subtasks yet. Run the annotation to fill this list." }
        };
    }
    let context = output.context.clone();
    rsx! {
        IndexSearch {
            jumps: output.jumps(),
            saying: filter.clone(),
            chosen,
            on_filter: move |text| wanted.set(text),
        }
        if entries.is_empty() {
            p { class: "explore-annotate-empty", role: "status",
                "No subtask says “{filter}”." }
        }
        ol { class: "explore-entries explore-entries-grouped", "aria-label": "Annotated subtasks by episode",
            for entry in entries {
                match entry {
                    IndexEntry::Measured(group) => rsx! {
                        li { key: "{group.episode()}", class: "explore-entries-group",
                            "aria-current": (chosen.episode().as_deref() == Some(group.episode())).then_some("true"),
                            // The heading is a heading as well as a button, so a reader
                            // moving by heading walks thirty-nine episodes instead of
                            // finding one heading in the whole workspace.
                            h3 { class: "explore-entries-heading-slot",
                                button {
                                    r#type: "button",
                                    id: "{IndexHeading::id(group.episode())}",
                                    class: "explore-entries-heading",
                                    onclick: {
                                        let episode = group.episode().to_owned();
                                        move |_| chosen.show(&episode)
                                    },
                                    span { class: "explore-entries-heading-name", "{context.name(group.episode())}" }
                                    span { class: "explore-entries-heading-count",
                                        "{Counted::subtasks(group.rows().len())}" }
                                }
                            }
                            ol { class: "explore-entries",
                                for row in group.rows() {
                                    SubtaskEntry {
                                        key: "{row.start_time()}",
                                        row: row.clone(),
                                        standing: SubtaskStanding::plain(chosen.holds(row))
                                            .roving(chosen.holds(row) || resting_stop.as_ref() == Some(row)),
                                        frames,
                                        on_select: {
                                            let row = row.clone();
                                            move |()| chosen.play_row(&row)
                                        },
                                    }
                                }
                            }
                        }
                    },
                    IndexEntry::Failed(failure) => rsx! {
                        FailedEpisode {
                            key: "{failure.episode()}",
                            name: context.name(failure.episode()),
                            failure,
                            on_retry,
                        }
                    },
                }
            }
        }
    }
}

/// The one row an episode nobody could annotate gets: what went wrong, and the
/// one action that could change it.
#[component]
fn FailedEpisode(failure: EpisodeFailure, name: String, on_retry: EventHandler<String>) -> Element {
    let episode = failure.episode().to_owned();
    rsx! {
        li { class: "explore-entries-group explore-entries-failed",
            h3 { class: "explore-entries-heading-slot",
                div { class: "explore-entries-heading explore-entries-heading-failed",
                    span { class: "explore-entries-heading-name", "{name}" }
                    span { class: "explore-entries-heading-count", "not annotated" }
                }
            }
            p { class: "explore-entry-failure",
                span { "{failure.reason()}" }
                Button {
                    purpose: ActionPurpose::Retry,
                    variant: ButtonVariant::Outline,
                    size: ButtonSize::Compact,
                    onclick: move |_| on_retry.call(episode.clone()),
                    "Retry"
                }
            }
        }
    }
}

/// Where an episode's heading is, so a jump can land on it.
struct IndexHeading;

impl IndexHeading {
    /// Focusing the heading also scrolls the index to it, so one call does
    /// both.
    fn id(episode: &str) -> String {
        format!("explore-entries-head-{episode}")
    }
}

/// The two ways into a three-hundred-row index: say what you are looking for,
/// or name the episode you want. Twelve thousand pixels of scroll was the
/// only other one.
#[component]
fn IndexSearch(
    jumps: Vec<(String, String)>,
    saying: String,
    chosen: ChosenSubtask,
    on_filter: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "explore-entries-search",
            label { r#for: "explore-entries-filter", class: "sr-only", "Filter subtasks" }
            TextInput {
                id: "explore-entries-filter".to_owned(),
                kind: TextInputKind::Search,
                value: saying,
                placeholder: "Filter subtasks",
                oninput: on_filter,
            }
            label { r#for: "explore-entries-jump", class: "sr-only", "Go to episode" }
            Select {
                id: "explore-entries-jump",
                class: "explore-entries-jump",
                value: String::new(),
                onchange: move |episode: String| {
                    if episode.is_empty() {
                        return;
                    }
                    chosen.show(&episode);
                    BrowserFocus::after_render(IndexHeading::id(&episode));
                },
                option { value: "", "Go to episode" }
                for (episode , name) in jumps {
                    option { key: "{episode}", value: "{episode}", "{name}" }
                }
            }
        }
    }
}

/// The chosen episode seen whole — every camera it recorded, and the robot
/// when the dataset has a model — with the measurement's subtasks on the
/// shared timeline beneath it.
///
/// This is the one place Explore mounts the whole-episode view: the track and
/// the chosen interval arrive as props, so swapping the composition underneath
/// is one component name.
#[component]
pub(super) fn AnnotatedEpisode(
    episode: AnnotationEpisode,
    subtasks: EpisodeSubtasks,
    context: SubtaskContext,
    chosen: ChosenSubtask,
) -> Element {
    let transport = EpisodeTransport::use_opened_episode(episode.opened());
    let track = subtasks.track();
    let requested = transport.clone();
    // Read the ask rather than take it: taking registers no subscription, and
    // an effect with no subscription runs once and never again.
    use_effect(move || {
        if let Some(request) = chosen.requested() {
            let (start, end) = request.interval;
            requested
                .clone()
                .play_interval(start, end + RUN_OUT_SECONDS);
        }
    });
    let activated = transport.clone();
    let playing = subtasks.chosen_row(&chosen);
    rsx! {
        header { class: "explore-review-head",
            match playing {
                Some(row) => rsx! {
                    p { class: "explore-review-subject", "{row.description()}" }
                    p { class: "explore-review-facts",
                        span { "{context.name(subtasks.episode())}" }
                        span { "{row.start_time().clock_label()} – {row.end_time().clock_label()}" }
                        code { class: "explore-review-id", "{subtasks.episode()}" }
                    }
                },
                None => rsx! {
                    p { class: "explore-review-subject",
                        "{context.name(subtasks.episode())}" }
                    p { class: "explore-review-facts",
                        span { "{Counted::subtasks(subtasks.rows().len())}" }
                        code { class: "explore-review-id", "{subtasks.episode()}" }
                    }
                },
            }
        }
        section { class: "explore-review-player", "aria-label": "Annotated episode",
            EpisodeInspection {
                episode: episode.opened().clone(),
                transport,
                featured_camera: context
                    .camera()
                    .map_or(FeaturedCamera::Overview, |camera| FeaturedCamera::Named(camera.to_owned())),
                span_tracks: if track.is_empty() { Vec::new() } else { vec![track] },
                on_interval: move |interval: TimelineInterval| {
                    let (start, end) = interval.span();
                    activated.clone().play_interval(start, end + RUN_OUT_SECONDS);
                },
            }
        }
    }
}

/// What the player was actually told to play, with the transport replaced by a
/// log. The effect under test is the one `AnnotatedEpisode` runs.
#[cfg(all(test, target_arch = "wasm32"))]
#[derive(Clone, Default, PartialEq)]
struct PlayLog {
    chosen: std::rc::Rc<std::cell::RefCell<Option<ChosenSubtask>>>,
    played: std::rc::Rc<std::cell::RefCell<Vec<(f64, f64)>>>,
}

#[cfg(all(test, target_arch = "wasm32"))]
impl PlayLog {
    fn render(self) -> Element {
        let chosen = ChosenSubtask::use_chosen();
        *self.chosen.borrow_mut() = Some(chosen);
        let played = self.played.clone();
        use_effect(move || {
            if let Some(request) = chosen.requested() {
                let (start, end) = request.interval;
                played.borrow_mut().push((start, end + RUN_OUT_SECONDS));
            }
        });
        rsx! {}
    }
    fn chosen(&self) -> ChosenSubtask {
        self.chosen
            .borrow()
            .expect("the harness mounted its choice")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: &str, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new("break_down_subtasks-1", episode, start, end, "fold").expect("a valid row")
    }

    /// The regression for the click that did nothing.
    ///
    /// Every row here belongs to one episode, so the player never remounts and
    /// the only thing that can move it is the ask itself. Taking the ask
    /// registered no subscription, so the effect ran once at mount and the
    /// transport sat at 0:00 through four clicks. The intervals also prove the
    /// playhead is asked for the row's own start, with the run-out after the
    /// end rather than half a second of lead-in before it.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn every_row_of_the_episode_already_open_moves_the_playhead() {
        let harness = PlayLog::default();
        let mut dom = VirtualDom::new_with_props(PlayLog::render, harness.clone());
        dom.rebuild_in_place();
        let chosen = harness.chosen();
        for (start, end) in [
            (1.5, 4.5),
            (8.0, 12.0),
            (17.5, 23.0),
            (8.0, 12.0),
            (8.0, 12.0),
        ] {
            let row = row("ep-1", start, end);
            dom.in_scope(ScopeId::ROOT, || chosen.play_row(&row));
            dom.render_immediate_to_vec();
        }
        assert_eq!(
            *harness.played.borrow(),
            [
                (1.5, 5.0),
                (8.0, 12.5),
                (17.5, 23.5),
                (8.0, 12.5),
                // Pressing one row twice is two asks, so a replay replays.
                (8.0, 12.5),
            ]
        );

        // Opening a whole episode plays nothing, and does not replay the last
        // interval when the player remounts on the new episode.
        dom.in_scope(ScopeId::ROOT, || chosen.show("ep-2"));
        dom.render_immediate_to_vec();
        assert_eq!(harness.played.borrow().len(), 5);
    }

    #[test]
    fn rows_group_by_episode_in_the_order_the_document_stores_them() {
        let groups = EpisodeSubtasks::group(
            [
                row("ep-1", 0.0, 8.0),
                row("ep-1", 8.0, 13.0),
                row("ep-2", 0.0, 4.0),
            ]
            .into_iter(),
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].episode(), "ep-1");
        assert_eq!(groups[0].rows().len(), 2);
        assert_eq!(groups[1].rows()[0].start_time(), 0.0);
        assert!(!groups[0].track().is_empty());
        assert!(EpisodeSubtasks::group(std::iter::empty()).is_empty());
    }

    #[test]
    fn a_filter_keeps_the_rows_that_say_it_and_drops_the_episodes_that_do_not() {
        let group = EpisodeSubtasks {
            episode: "ep-1".to_owned(),
            rows: vec![
                SubtaskRow::new("break_down_subtasks-1", "ep-1", 0.0, 4.0, "Fold the skirt")
                    .unwrap(),
                SubtaskRow::new("break_down_subtasks-1", "ep-1", 4.0, 8.0, "Grasp the edge")
                    .unwrap(),
            ],
        };
        let folded = group.with_rows_saying("fold").expect("one row says it");
        assert_eq!(folded.rows().len(), 1);
        assert_eq!(folded.rows()[0].description(), "Fold the skirt");
        assert_eq!(
            group.with_rows_saying("").map(|kept| kept.rows().len()),
            Some(2)
        );
        assert!(group.with_rows_saying("weld").is_none());
    }
}
