//! The episodes a node's workspace has open, one tab each, beside its index —
//! the way Data opens episodes beside its rail.
//!
//! Every row a node lists belongs to an episode, so choosing a row opens that
//! episode in a tab of its own, or brings its tab forward and plays the row's
//! stretch in it. Several can stay open to be compared side by side, since
//! the tabs are an ordinary workbench: they dock, split, and close.
use crate::activities::episode::{Episode, PlayRequest};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{EmptyState, KindMark};

/// The tile the tabs open in.
const TABS_TILE: &str = "curation-episodes";
/// The one tab a workspace may keep ahead of its episodes.
const LEADING_PANEL: &str = "curation-episodes-leading";

/// The one tab a workspace keeps ahead of its episodes and never closes —
/// a dataset's overview, a category's profile — named and marked the way
/// the episode tabs beside it are.
#[derive(Clone, PartialEq)]
pub(super) struct LeadingTab {
    title: &'static str,
    mark: (char, String),
    content: Element,
}

impl LeadingTab {
    /// A tab titled `title`, marked by `letter` in `hue`.
    pub(super) fn new(title: &'static str, letter: char, hue: String, content: Element) -> Self {
        Self {
            title,
            mark: (letter, hue),
            content,
        }
    }
    /// The dataset a workspace reads, as Data shows it.
    pub(super) fn dataset(content: Element) -> Self {
        Self::new(
            "Overview",
            'D',
            "var(--tone-kind-dataset)".to_owned(),
            content,
        )
    }
}

/// One open episode, and the stretch of it last chosen.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenEpisode {
    episode: EpisodeId,
    /// The chosen stretch; `None` for the episode whole.
    interval: Option<(f64, f64)>,
    /// Each ask to play from `interval`'s start, counted, so choosing it
    /// again replays it.
    play: Option<PlayRequest>,
}

impl OpenEpisode {
    pub(super) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(super) fn interval(&self) -> Option<(f64, f64)> {
        self.interval
    }
    pub(super) fn play(&self) -> Option<PlayRequest> {
        self.play
    }
    /// Whether this is the stretch of `episode` that starts at `start`, which
    /// is how every row is named within its episode across Curation.
    pub(super) fn holds(&self, episode: &EpisodeId, start: f64) -> bool {
        &self.episode == episode && self.interval.map(|(from, _)| from) == Some(start)
    }
    fn panel(episode: &EpisodeId) -> PanelId {
        PanelId::new(format!("curation-episode-{episode}"))
    }
}

/// Which episodes a workspace has open, in the order they were opened, and
/// which tab is in front.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct OpenEpisodes {
    open: Signal<Vec<OpenEpisode>>,
    front: Signal<Option<PanelId>>,
    /// Whether the next open steps the tab in front on to another episode,
    /// as J and K do, rather than opening a tab beside it.
    stepping: Signal<bool>,
    /// Whether the tab in front was opened for the operator, showing the
    /// list's first row, rather than chosen by them.
    following: Signal<bool>,
}

impl OpenEpisodes {
    /// The workspace's open episodes, provided to its index too, so the
    /// index's keys can step them.
    pub(super) fn use_open() -> Self {
        let tabs = Self {
            open: use_signal(Vec::new),
            front: use_signal(|| None),
            stepping: use_signal(|| false),
            following: use_signal(|| false),
        };
        use_context_provider(|| tabs)
    }
    /// The open episodes of the workspace an index sits in, if any.
    pub(super) fn use_around() -> Option<Self> {
        use_hook(try_consume_context::<Self>)
    }
    /// Make the next open step the tab in front on, as Data's J and K do.
    pub(super) fn step_next(mut self) {
        self.stepping.set(true);
    }
    /// Open `episode` in a tab of its own, or bring its tab forward, and play
    /// on from `interval`'s start when one is given. After `step_next`, an
    /// episode not yet open takes the place of the one in front instead.
    pub(super) fn open(mut self, episode: &EpisodeId, interval: Option<(f64, f64)>) {
        let stepping = std::mem::take(&mut *self.stepping.write());
        let front = self.front.peek().clone();
        {
            let mut open = self.open.write();
            let in_front = front.and_then(|front| {
                open.iter()
                    .position(|tab| OpenEpisode::panel(&tab.episode) == front)
            });
            match open.iter_mut().find(|tab| &tab.episode == episode) {
                Some(tab) => {
                    tab.interval = interval;
                    tab.play = interval.map(|(start, _)| PlayRequest::after(tab.play, start));
                }
                None => {
                    let tab = OpenEpisode {
                        episode: episode.clone(),
                        interval,
                        play: interval.map(|(start, _)| PlayRequest::new(start)),
                    };
                    match in_front.filter(|_| stepping) {
                        Some(position) => open[position] = tab,
                        None => open.push(tab),
                    }
                }
            }
        }
        self.front.set(Some(OpenEpisode::panel(episode)));
        self.following.set(false);
    }
    /// Open the list's first row, `first`, until the operator chooses one:
    /// when nothing is open yet, so the recording column is never an empty
    /// half beside a full index, and again whenever the first row changes
    /// while the tab in front is still the one opened for them, so it
    /// settles on the list's first row rather than on whichever row arrived
    /// first.
    pub(super) fn open_first(self, first: Option<(EpisodeId, Option<(f64, f64)>)>) {
        if self.open.peek().is_empty() || *self.following.peek() {
            self.show_first(first);
        }
    }
    /// Show the list's first row, `first`, in the tab in front, in place of
    /// what it shows, as a tab opened for the operator.
    pub(super) fn show_first(mut self, first: Option<(EpisodeId, Option<(f64, f64)>)>) {
        if let Some((episode, interval)) = first {
            self.stepping.set(!self.open.peek().is_empty());
            self.open(&episode, interval);
            self.following.set(true);
        }
    }
    /// The episode tab in front, if an episode tab is.
    pub(super) fn front(&self) -> Option<OpenEpisode> {
        let front = self.front.read().clone()?;
        self.open
            .read()
            .iter()
            .find(|tab| OpenEpisode::panel(&tab.episode) == front)
            .cloned()
    }
    /// Whether the tab in front shows the stretch of `episode` starting at
    /// `start`: the row the index marks as chosen.
    pub(super) fn shows(&self, episode: &EpisodeId, start: f64) -> bool {
        self.front().is_some_and(|tab| tab.holds(episode, start))
    }
    fn activated(mut self, panel: PanelId) {
        self.front.set(Some(panel));
        self.following.set(false);
    }
    /// Closing the tab in front brings the newest remaining one forward.
    fn close(mut self, panel: &PanelId) {
        self.following.set(false);
        self.open
            .write()
            .retain(|tab| &OpenEpisode::panel(&tab.episode) != panel);
        if self.front.peek().as_ref() == Some(panel) {
            let next = self
                .open
                .peek()
                .last()
                .map(|tab| OpenEpisode::panel(&tab.episode))
                .or_else(|| Some(PanelId::new(LEADING_PANEL)));
            self.front.set(next);
        }
    }
}

/// The open episodes as tabs, each drawn by `render`, after the workspace's
/// leading tab when it has one.
#[component]
pub(super) fn EpisodeTabs(
    open: OpenEpisodes,
    render: Callback<OpenEpisode, Element>,
    #[props(default)] leading: Option<LeadingTab>,
    /// What an empty column says when nothing is open and there is no
    /// leading tab to show instead.
    empty: String,
) -> Element {
    let tabs = open.open.read().clone();
    if tabs.is_empty() && leading.is_none() {
        return rsx! {
            div { class: "curation-episode-tabs-empty",
                EmptyState { title: "Nothing open", p { "{empty}" } }
            }
        };
    }
    let mut panels = Vec::new();
    if let Some(LeadingTab {
        title,
        mark: (letter, hue),
        content,
    }) = leading
    {
        panels.push(
            Panel::new(
                LEADING_PANEL,
                title,
                TABS_TILE,
                rsx! { div { class: "curation-leading-tab", {content} } },
            )
            .with_tab_icon(rsx! { KindMark { letter, hue, label: title } }),
        );
    }
    for tab in tabs {
        let panel = OpenEpisode::panel(&tab.episode);
        let title = Episode::name_of(&tab.episode);
        panels.push(
            Panel::new(
                panel,
                title,
                TABS_TILE,
                rsx! { div { class: "curation-episode-tab", {render.call(tab)} } },
            )
            .with_closable(true)
            .with_tab_icon(rsx! { KindMark { letter: 'E', hue: "var(--tone-kind-episode)", label: "Episode" } }),
        );
    }
    rsx! {
        div { class: "curation-episode-tabs",
            PanelWorkspace {
                panels,
                initial_layout: PanelLayout::new(LayoutNode::tile(TABS_TILE, std::iter::empty::<PanelId>())),
                active_panel: open.front.read().clone(),
                on_panel_activate: move |panel: PanelId| open.activated(panel),
                on_panel_close: move |panel: PanelId| open.close(&panel),
            }
        }
    }
}
