//! What one annotation node produced: the index of intervals it wrote, and
//! the episode one of them belongs to, playing beside it.
use super::episodes::AnnotationEpisode;
use super::subtask::SubtaskRow;
use crate::activities::curation::episode_tabs::{OpenEpisode, OpenEpisodes};
use crate::activities::curation::review::{
    Counted, IndexBar, RowKeys, RowStanding, SubtaskContext, SubtaskEntry, SubtaskIntervals,
};
use crate::activities::curation::thumbnail::Thumbnails;
#[cfg(all(test, target_arch = "wasm32"))]
use crate::activities::episode::PlayRequest;
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, FeaturedCamera, HeadingSubject,
};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonSize, ButtonVariant, Place, Select, TextInput,
    TextInputKind,
};

/// One episode the preview could not break down, and why.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeFailure {
    episode: EpisodeId,
    reason: String,
}

impl EpisodeFailure {
    pub(super) fn new(episode: EpisodeId, reason: impl Into<String>) -> Self {
        Self {
            episode,
            reason: reason.into(),
        }
    }
    pub(super) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    fn reason(&self) -> &str {
        &self.reason
    }
}

/// One episode's rows, in time order.
#[derive(Clone, PartialEq)]
pub(super) struct EpisodeSubtasks {
    episode: EpisodeId,
    rows: Vec<SubtaskRow>,
}

impl EpisodeSubtasks {
    /// Group an annotation node's rows by episode, keeping the order the
    /// preview read them in.
    pub(super) fn group(rows: impl Iterator<Item = SubtaskRow>) -> Vec<Self> {
        let mut grouped = Vec::<Self>::new();
        for row in rows {
            match grouped
                .iter_mut()
                .find(|group| &group.episode == row.episode())
            {
                Some(group) => group.rows.push(row),
                None => grouped.push(Self {
                    episode: row.episode().clone(),
                    rows: vec![row],
                }),
            }
        }
        grouped
    }
    /// An episode the node has no rows for, so the player still opens.
    pub(super) fn none(episode: &EpisodeId) -> Self {
        Self {
            episode: episode.clone(),
            rows: Vec::new(),
        }
    }
    pub(super) fn episode(&self) -> &EpisodeId {
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
    /// The row a tab of this episode is playing, out of its own.
    fn row_in(&self, tab: &OpenEpisode) -> Option<&SubtaskRow> {
        self.rows
            .iter()
            .find(|row| tab.holds(row.episode(), row.start_time()))
    }
}

/// Which interval the operator is looking at: the episodes open as tabs, and
/// in the one in front the row they picked, if they picked one.
///
/// Both halves of the review read this, so the index and the recording can
/// never disagree about what is on screen.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ChosenSubtask {
    tabs: OpenEpisodes,
}

impl ChosenSubtask {
    pub(super) fn use_chosen() -> Self {
        Self {
            tabs: OpenEpisodes::use_open(),
        }
    }
    pub(super) fn tabs(&self) -> OpenEpisodes {
        self.tabs
    }
    /// The episode in front.
    pub(super) fn episode(&self) -> Option<EpisodeId> {
        self.tabs.front().map(|tab| tab.episode().clone())
    }
    /// Show one whole episode without playing any particular interval, as the
    /// index does when its heading is pressed.
    pub(super) fn show(self, episode: &EpisodeId) {
        self.tabs.open(episode, None);
    }
    /// Show one row and play it.
    pub(super) fn play_row(self, row: &SubtaskRow) {
        self.tabs
            .open(row.episode(), Some((row.start_time(), row.end_time())));
    }
    /// Show an episode and, when a curation named one, its interval.
    pub(super) fn focus(self, episode: &EpisodeId, interval: Option<(f64, f64)>) {
        self.tabs.open(episode, interval);
    }
    fn holds(&self, row: &SubtaskRow) -> bool {
        self.tabs.shows(row.episode(), row.start_time())
    }
    /// Whether no row at all is chosen, in which case the index's first row
    /// carries its tab stop: a roving list always answers Tab once.
    fn holds_no_row(&self) -> bool {
        self.tabs.front().and_then(|tab| tab.interval()).is_none()
    }
    /// The interval the tab in front has been asked to play, read rather than
    /// taken.
    ///
    /// Taking it was the whole bug: `Signal::write` registers no subscription,
    /// so the effect that took the ask never learned that a new one had
    /// arrived, and only ran again when the episode changed underneath it.
    #[cfg(all(test, target_arch = "wasm32"))]
    fn requested(&self) -> Option<PlayRequest> {
        self.tabs.front().and_then(|tab| tab.play())
    }
}

/// What one annotation node's preview read, as its index shows it: the rows
/// it read, the episodes it could not read, and what to call them.
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
/// Failure sits among the episodes in recording order, because an operator
/// scanning the index looks for it where it belongs.
#[derive(Clone, PartialEq)]
pub(super) enum IndexEntry {
    Measured(EpisodeSubtasks),
    Failed(EpisodeFailure),
}

impl IndexEntry {
    fn episode(&self) -> &EpisodeId {
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
    fn jumps(&self) -> Vec<EpisodeId> {
        self.matching("")
            .iter()
            .map(|entry| entry.episode().clone())
            .collect()
    }
    /// The index as one list in recording order: the groups whose rows say
    /// `wanted` carrying only those rows, and the failures whose episode or
    /// reason says it. An empty filter keeps everything.
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
                    || failure.episode().as_str().contains(&wanted)
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
    thumbnails: Thumbnails,
    chosen: ChosenSubtask,
    /// Break down one failed episode again.
    on_retry: EventHandler<EpisodeId>,
    /// Ask for subtasks like one row, from where its thumbnail sits.
    on_find_similar: EventHandler<(SubtaskRow, Option<Place>)>,
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
            p { class: "curation-annotate-empty", role: "status",
                "No subtasks yet. Run Break down to preview them." }
        };
    }
    rsx! {
        IndexSearch {
            jumps: output.jumps(),
            saying: filter.clone(),
            chosen,
            on_filter: move |text| wanted.set(text),
        }
        if entries.is_empty() {
            p { class: "curation-annotate-empty", role: "status",
                "No subtask says “{filter}”." }
        }
        ol { class: "curation-entries curation-entries-grouped", "aria-label": "Annotated subtasks by episode",
            // The key sits on the loop's own item, so an episode that lands
            // mid-run moves the groups after it rather than rebuilding them,
            // and their rows keep the thumbnails they already have.
            for entry in entries {
                Fragment { key: "{entry.episode()}",
                    match entry {
                        IndexEntry::Measured(group) => rsx! {
                            li { class: "curation-entries-group",
                                "aria-current": (chosen.episode().as_ref() == Some(group.episode())).then_some("true"),
                                // The heading is a heading as well as a button, so a reader
                                // moving by heading walks thirty-nine episodes instead of
                                // finding one heading in the whole workspace.
                                h3 { class: "curation-entries-heading-slot",
                                    button {
                                        r#type: "button",
                                        id: "{IndexHeading::id(group.episode())}",
                                        class: "curation-entries-heading",
                                        onclick: {
                                            let episode = group.episode().clone();
                                            move |_| chosen.show(&episode)
                                        },
                                        span { class: "curation-entries-heading-name", title: "{group.episode()}",
                                            "{Episode::name_of(group.episode())}" }
                                        span { class: "curation-entries-heading-count",
                                            "{Counted::subtasks(group.rows().len())}" }
                                    }
                                }
                                ol { class: "curation-entries",
                                    for (row, key) in group.rows().iter().zip(RowKeys::of(group.rows())) {
                                        SubtaskEntry {
                                            key: "{key}",
                                            row: row.clone(),
                                            standing: RowStanding::plain(chosen.holds(row))
                                                .roving(chosen.holds(row) || resting_stop.as_ref() == Some(row)),
                                            thumbnails,
                                            on_select: {
                                                let row = row.clone();
                                                move |()| chosen.play_row(&row)
                                            },
                                            on_find_similar: {
                                                let row = row.clone();
                                                move |from| on_find_similar.call((row.clone(), from))
                                            },
                                        }
                                    }
                                }
                            }
                        },
                        IndexEntry::Failed(failure) => rsx! {
                            FailedEpisode { failure, on_retry }
                        },
                    }
                }
            }
        }
    }
}

/// The one row an episode the preview could not break down gets: what went
/// wrong, and the one action that could change it.
#[component]
fn FailedEpisode(failure: EpisodeFailure, on_retry: EventHandler<EpisodeId>) -> Element {
    let episode = failure.episode().clone();
    rsx! {
        li { class: "curation-entries-group curation-entries-failed",
            h3 { class: "curation-entries-heading-slot",
                div { class: "curation-entries-heading curation-entries-heading-failed",
                    span { class: "curation-entries-heading-name", title: "{failure.episode()}",
                        "{Episode::name_of(failure.episode())}" }
                    span { class: "curation-entries-heading-count", "not broken down" }
                }
            }
            p { class: "curation-entry-failure",
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
    fn id(episode: &EpisodeId) -> String {
        format!("curation-entries-head-{episode}")
    }
}

/// The two ways into a three-hundred-row index: say what you are looking for,
/// or name the episode you want. Twelve thousand pixels of scroll was the
/// only other one.
#[component]
fn IndexSearch(
    jumps: Vec<EpisodeId>,
    saying: String,
    chosen: ChosenSubtask,
    on_filter: EventHandler<String>,
) -> Element {
    rsx! {
        IndexBar {
            label { r#for: "curation-entries-filter", class: "sr-only", "Filter subtasks" }
            TextInput {
                id: "curation-entries-filter".to_owned(),
                kind: TextInputKind::Search,
                value: saying,
                placeholder: "Filter subtasks",
                oninput: on_filter,
            }
            label { r#for: "curation-entries-jump", class: "sr-only", "Go to episode" }
            Select {
                id: "curation-entries-jump",
                class: "curation-entries-jump",
                value: String::new(),
                onchange: move |episode: String| {
                    let Ok(episode) = EpisodeId::try_from(episode) else {
                        return;
                    };
                    chosen.show(&episode);
                    BrowserFocus::after_render(IndexHeading::id(&episode));
                },
                option { value: "", "Go to episode" }
                for episode in jumps {
                    option { key: "{episode}", value: "{episode}", "{Episode::listed_name_of(&episode)}" }
                }
            }
        }
    }
}

/// One open episode seen whole — every camera it recorded, and the robot
/// when the dataset has a model — with the measurement's subtasks on the
/// shared timeline beneath it, playing the row chosen in its tab.
#[component]
pub(super) fn AnnotatedEpisode(
    episode: AnnotationEpisode,
    subtasks: EpisodeSubtasks,
    context: SubtaskContext,
    tab: OpenEpisode,
    /// Ask for subtasks like the one playing, as every other view of a
    /// subtask offers.
    on_find_similar: EventHandler<SubtaskRow>,
) -> Element {
    let playing = subtasks.row_in(&tab);
    rsx! {
        match playing {
            Some(row) => rsx! {
                EpisodeHeading {
                    subject: HeadingSubject::part(row.description(), subtasks.episode())
                        .with_facts(vec![format!("{} – {}", row.start_time().clock_label(), row.end_time().clock_label())]),
                    actions: {
                        let row = row.clone();
                        rsx! {
                            Button {
                                purpose: ActionPurpose::Navigate,
                                variant: ButtonVariant::Link,
                                size: ButtonSize::Compact,
                                title: "Rank every other subtask by how likely it is the same operation",
                                onclick: move |_| on_find_similar.call(row.clone()),
                                "Find similar"
                            }
                        }
                    },
                }
            },
            None => rsx! {
                EpisodeHeading {
                    subject: HeadingSubject::episode(episode.episode())
                        .with_facts(vec![Counted::subtasks(subtasks.rows().len()).to_string()]),
                }
            },
        }
        section { class: "curation-review-player", "aria-label": "Annotated episode",
            EpisodeInspection {
                episode: episode.episode().clone(),
                featured_camera: FeaturedCamera::measured(context.camera()),
                tracks: vec![SubtaskIntervals::of(subtasks.rows()).choosing(playing).track().into()],
                focus: tab.play(),
            }
        }
    }
}

/// Every ask the player was handed, handed over the way `AnnotatedEpisode`
/// does it: read from the choice while rendering and passed on as a value.
#[cfg(all(test, target_arch = "wasm32"))]
#[derive(Clone, Default, PartialEq)]
struct PlayLog {
    chosen: std::rc::Rc<std::cell::RefCell<Option<ChosenSubtask>>>,
    asked: std::rc::Rc<std::cell::RefCell<Vec<PlayRequest>>>,
}

#[cfg(all(test, target_arch = "wasm32"))]
impl PlayLog {
    fn render(self) -> Element {
        let chosen = ChosenSubtask::use_chosen();
        *self.chosen.borrow_mut() = Some(chosen);
        let asked = self.asked.clone();
        let focus = chosen.requested();
        use_effect(use_reactive(&focus, move |focus| {
            if let Some(focus) = focus {
                asked.borrow_mut().push(focus);
            }
        }));
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

    fn row(episode: EpisodeId, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new(episode, (start, end), "fold").expect("a valid row")
    }

    /// The regression for the click that did nothing.
    ///
    /// Every row here belongs to one episode, so the player never remounts and
    /// the only thing that can move it is the ask itself. Taking the ask
    /// registered no subscription, so the player heard it once at mount and
    /// the transport sat at 0:00 through four clicks. Each ask carries the
    /// row's own start, where playing picks up.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn every_row_of_the_episode_already_open_moves_the_playhead() {
        let harness = PlayLog::default();
        let mut dom = VirtualDom::new_with_props(PlayLog::render, harness.clone());
        dom.rebuild_in_place();
        let chosen = harness.chosen();
        let mut expected = Vec::new();
        for (start, end) in [
            (1.5, 4.5),
            (8.0, 12.0),
            (17.5, 23.0),
            (8.0, 12.0),
            // Pressing one row twice is two asks, so a replay replays.
            (8.0, 12.0),
        ] {
            let row = row(EpisodeId::fixture(1), start, end);
            dom.in_scope(ScopeId::ROOT, || chosen.play_row(&row));
            // Once for the view to read the ask, once for the effect it wakes.
            dom.render_immediate_to_vec();
            dom.render_immediate_to_vec();
            expected.push(PlayRequest::after(expected.last().copied(), start));
        }
        assert_eq!(*harness.asked.borrow(), expected);

        // Opening a whole episode plays nothing, and does not replay the last
        // interval when the player remounts on the new episode.
        dom.in_scope(ScopeId::ROOT, || chosen.show(&EpisodeId::fixture(2)));
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(harness.asked.borrow().len(), 5);
    }

    #[test]
    fn rows_group_by_episode_in_the_order_the_preview_read_them() {
        let groups = EpisodeSubtasks::group(
            [
                row(EpisodeId::fixture(1), 0.0, 8.0),
                row(EpisodeId::fixture(1), 8.0, 13.0),
                row(EpisodeId::fixture(2), 0.0, 4.0),
            ]
            .into_iter(),
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].episode(), &EpisodeId::fixture(1));
        assert_eq!(groups[0].rows().len(), 2);
        assert_eq!(groups[1].rows()[0].start_time(), 0.0);
        assert!(EpisodeSubtasks::group(std::iter::empty()).is_empty());
    }

    #[test]
    fn a_filter_keeps_the_rows_that_say_it_and_drops_the_episodes_that_do_not() {
        let group = EpisodeSubtasks {
            episode: EpisodeId::fixture(1),
            rows: vec![
                SubtaskRow::new(EpisodeId::fixture(1), (0.0, 4.0), "Fold the skirt").unwrap(),
                SubtaskRow::new(EpisodeId::fixture(1), (4.0, 8.0), "Grasp the edge").unwrap(),
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
