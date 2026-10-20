//! One recording seen whole, the same way on every surface that shows one:
//! each camera it recorded, its robot when the dataset has a model, its
//! numeric streams as plots, and a timeline of every stream under the rows a
//! surface brings — docked panels on one clock.
//!
//! A surface says which episode, which camera leads, which rows it adds and
//! which span to play. The clock, the streams, what is plotted and where each
//! panel sits for the room the surface gives belong to the episode, so an
//! operator recognizes the same recording wherever it is shown.
use super::{
    arrangement::{EpisodeArrangement, Size},
    boxes::CameraBoxes,
    camera::EpisodeCamera,
    choices::StreamChoices,
    identity::Episode,
    plots::PlotsPanel,
    reads::StreamReads,
    remembered_layout::RememberedLayout,
    robot::{RobotModel, RobotStage},
    streams::{RecordedStream, StreamCatalog},
    timeline::{EpisodeTimeline, EvidenceTrack},
    transport::EpisodeTransport,
};
use crate::components::icons::CameraStreamIcon;
use crate::components::timeline::{KeyboardFocus, PlaybackClock};
use dioxus::prelude::*;
use dioxus_workbench::{Panel, PanelId, PanelLayout, PanelWorkspace};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, Remount};

/// Which of an episode's cameras leads the inspection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::activities) enum FeaturedCamera {
    /// Whichever camera shows the workspace best, for a view that is about
    /// the recording rather than about one camera.
    #[default]
    Overview,
    /// The camera this view is about, such as the one a measurement read. An
    /// episode that did not record it leads with its overview.
    Named(String),
}

impl FeaturedCamera {
    /// The camera a measurement read, else whichever shows the workspace best.
    pub(in crate::activities) fn measured(camera: Option<&str>) -> Self {
        camera.map_or(Self::Overview, |camera| Self::Named(camera.to_owned()))
    }
    /// `cameras` with the featured one first; the rest keep topic order.
    fn lead<'a>(&self, mut cameras: Vec<&'a RecordedStream>) -> Vec<&'a RecordedStream> {
        let names = cameras
            .iter()
            .map(|camera| camera.name().to_owned())
            .collect::<Vec<_>>();
        let wanted = match self {
            Self::Named(camera) if names.contains(camera) => Some(camera.as_str()),
            _ => StreamCatalog::overview_camera(&names),
        };
        if let Some(place) = wanted.and_then(|wanted| names.iter().position(|name| name == wanted))
        {
            let featured = cameras.remove(place);
            cameras.insert(0, featured);
        }
        cameras
    }
}

/// One ask to play an interval to its end, or on from an instant, and which
/// ask it is.
///
/// The count is what makes two asks different values. The span alone made
/// asking for the same one twice indistinguishable from asking once, and the
/// view — which reacts only to a change — sat still the second time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities) struct PlayRequest {
    start: f64,
    end: Option<f64>,
    asked: usize,
}

impl PlayRequest {
    /// Play from `start` and stop at `end`; an interval of no length is an
    /// instant, played on from.
    pub(in crate::activities) fn span((start, end): (f64, f64)) -> Self {
        Self {
            start,
            end: (end > start).then_some(end),
            asked: 0,
        }
    }
    /// Play on from `start`.
    pub(in crate::activities) fn instant(start: f64) -> Self {
        Self {
            start,
            end: None,
            asked: 0,
        }
    }
    /// This ask made after `previous`, which differs from it even when it
    /// asks the same.
    pub(in crate::activities) fn after(self, previous: Option<Self>) -> Self {
        Self {
            asked: previous.map_or(0, |previous| previous.asked + 1),
            ..self
        }
    }
    pub(super) fn play(self, clock: &mut PlaybackClock) {
        match self.end {
            Some(end) => clock.play_span(self.start, end),
            None => clock.play_from(self.start),
        }
    }
}

#[component]
pub(in crate::activities) fn EpisodeInspection(
    episode: Episode,
    /// Rows drawn above the recorded streams on the timeline.
    #[props(default)]
    tracks: Vec<EvidenceTrack>,
    /// Whether `tracks` are what the surface is about, so the timeline takes
    /// near half the room; otherwise the recording leads, and the timeline
    /// takes what its rows need.
    #[props(default = true)]
    tracks_lead: bool,
    /// What to play, such as the row the operator chose, which plays to its
    /// end. Each new ask plays it; `None` leaves the playhead where the
    /// operator put it.
    #[props(default)]
    focus: Option<PlayRequest>,
    /// Which camera leads the others.
    #[props(default)]
    featured_camera: FeaturedCamera,
    /// Boxes drawn over one of the cameras, such as the objects a
    /// measurement tracked on it.
    #[props(default)]
    camera_boxes: Option<CameraBoxes>,
) -> Element {
    // Another episode is another clock, other streams and other choices, so
    // it mounts afresh rather than rewiring the previous one's.
    let evidence = Evidence {
        tracks,
        lead: tracks_lead,
        boxes: camera_boxes,
    };
    rsx! {
        Remount { identity: format!("{}/{}", episode.dataset().label(), episode.id()),
            InspectedEpisode { episode, evidence, focus, featured_camera }
        }
    }
}

/// The rows a surface draws above the recorded streams, whether they lead
/// the recording, and the boxes it draws over a camera.
#[derive(Clone, PartialEq)]
struct Evidence {
    tracks: Vec<EvidenceTrack>,
    lead: bool,
    boxes: Option<CameraBoxes>,
}

impl Evidence {
    /// The boxes drawn over `camera`, when the surface marks that one.
    fn boxes_over(&self, camera: &RecordedStream) -> Option<CameraBoxes> {
        self.boxes
            .as_ref()
            .filter(|boxes| boxes.camera() == camera.name())
            .cloned()
    }
    /// `arrangement` with the timeline room these rows claim.
    fn arranging(&self, arrangement: EpisodeArrangement) -> EpisodeArrangement {
        match (self.tracks.is_empty(), self.lead) {
            (true, _) => arrangement,
            (false, true) => arrangement.led_by_evidence(),
            (false, false) => {
                arrangement.beside_rows(self.tracks.iter().map(EvidenceTrack::row_height).sum())
            }
        }
    }
}

/// An inspected episode once its streams are known: the streams its panels
/// show, the clock they all read, the operator's choices among the streams,
/// what has been read of them, and the dataset's robot model.
#[derive(Clone, PartialEq)]
pub(super) struct LoadedEpisode {
    catalog: StreamCatalog,
    transport: EpisodeTransport,
    choices: StreamChoices,
    reads: StreamReads,
    robot: Resource<Result<Option<RobotModel>, String>>,
}

impl LoadedEpisode {
    pub(super) fn catalog(&self) -> &StreamCatalog {
        &self.catalog
    }
    pub(super) fn transport(&self) -> &EpisodeTransport {
        &self.transport
    }
    pub(super) fn choices(&self) -> StreamChoices {
        self.choices
    }
    pub(super) fn reads(&self) -> StreamReads {
        self.reads
    }
}

#[component]
fn InspectedEpisode(
    episode: Episode,
    evidence: Evidence,
    focus: Option<PlayRequest>,
    featured_camera: FeaturedCamera,
) -> Element {
    let transport = EpisodeTransport::use_episode(&episode);
    transport.use_page_keys();
    let mut asked = transport.clone();
    use_effect(use_reactive(&focus, move |focus| {
        if let Some(focus) = focus {
            asked.play(focus);
        }
    }));
    let opened = episode.clone();
    let mut catalog = use_resource(move || StreamCatalog::load(opened.clone()));
    let robot = RobotModel::use_dataset(episode.dataset());
    let choices = StreamChoices::use_choices();
    let reads = StreamReads::use_reads(transport.range());
    use_effect(move || {
        if let Some(Ok(catalog)) = &*catalog.read() {
            let mut choices = choices;
            choices.open(catalog);
        }
    });
    let mut keys = transport.clone();
    let loaded = match (&*catalog.read(), &*robot.read()) {
        (Some(Ok(streams)), Some(_)) => Ok(Some(LoadedEpisode {
            catalog: streams.clone(),
            transport: transport.clone(),
            choices,
            reads,
            robot,
        })),
        (Some(Err(error)), _) => Err(error.clone()),
        _ => Ok(None),
    };
    rsx! {
        section { class: "episode-inspection", "aria-label": "Recorded episode",
            // Space, the arrows and Home/End drive the clock from any panel;
            // the timeline answers them itself, and the page listener takes
            // them while focus rests outside every panel.
            onkeydown: move |event: KeyboardEvent| {
                if !KeyboardFocus::editing() && keys.handle_key(&event) {
                    event.prevent_default();
                    event.stop_propagation();
                }
            },
            match loaded {
                Ok(Some(loaded)) => rsx! {
                    InspectionDock {
                        loaded,
                        evidence,
                        featured_camera,
                        remembered: RememberedLayout::of(episode.dataset()),
                    }
                },
                Ok(None) => rsx! { p { class: "episode-panel-status", role: "status", "Reading the recorded streams…" } },
                Err(error) => rsx! {
                    div { class: "episode-panel-status", role: "alert",
                        p { "The recorded streams could not be read: {error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                            onclick: move |_| catalog.restart(), "Retry" }
                    }
                },
            }
        }
    }
}

/// The episode's panels docked for the room they are given. The room is
/// measured before anything is laid out, so the first arrangement is already
/// the one that fits.
#[component]
fn InspectionDock(
    loaded: LoadedEpisode,
    evidence: Evidence,
    featured_camera: FeaturedCamera,
    remembered: RememberedLayout,
) -> Element {
    let mut room = use_signal(|| None::<Size>);
    let choices = loaded.choices();
    let transport = loaded.transport().clone();
    let catalog = loaded.catalog().clone();
    let cameras = featured_camera.lead(
        catalog
            .videos()
            .filter(|camera| choices.camera_open(camera.key()))
            .collect(),
    );
    let mut panels = cameras
        .iter()
        .map(|camera| {
            let title = catalog.camera_title(camera);
            Panel::new(
                EpisodeArrangement::camera(camera.key()),
                title.clone(),
                "episode-view-0",
                rsx! { EpisodeCamera { camera: (*camera).clone(), reads: loaded.reads(), transport: transport.clone(), title, boxes: evidence.boxes_over(camera) } },
            )
            .with_closable(true)
            .with_tab_icon(rsx! { CameraStreamIcon {} })
            .with_class("episode-camera-dock")
        })
        .collect::<Vec<_>>();
    let mut robot = loaded.robot;
    let stage = match &*robot.read() {
        Some(Ok(Some(model))) => Some(rsx! {
            RobotStage { model: model.clone(), streams: catalog.clone(), transport: transport.clone() }
        }),
        Some(Err(error)) => Some(rsx! {
            div { class: "episode-panel-status", role: "alert",
                p { "The robot model could not be read: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    onclick: move |_| robot.restart(), "Retry" }
            }
        }),
        _ => None,
    };
    if let Some(stage) = stage {
        panels.push(
            Panel::new(EpisodeArrangement::STAGE, "Scene", "episode-view-0", stage)
                .with_class("episode-stage-dock"),
        );
    }
    let arrangement = evidence.arranging(EpisodeArrangement::new(
        panels.iter().map(|panel| panel.id.clone()).collect(),
    ));
    let evidence = evidence.tracks;
    panels.push(
        Panel::new(
            EpisodeArrangement::PLOTS,
            "Plots",
            "episode-plots",
            rsx! { PlotsPanel { loaded: loaded.clone() } },
        )
        .with_class("episode-plots-dock"),
    );
    panels.push(
        Panel::new(
            EpisodeArrangement::TIMELINE,
            "Timeline",
            "episode-timeline",
            rsx! { EpisodeTimeline { loaded: loaded.clone(), evidence } },
        )
        .with_class("episode-timeline-dock"),
    );
    rsx! {
        div { class: "episode-inspection-dock",
            // A hidden activity measures nothing; the arrangement waits for a
            // room with size rather than folding every view into one tab.
            onresize: move |event: Event<ResizeData>| {
                if let Ok(size) = event.data().get_content_box_size()
                    && size.width > 0.0
                    && size.height > 0.0
                {
                    let measured = Some(Size::new(size.width, size.height));
                    if *room.peek() != measured {
                        room.set(measured);
                    }
                }
            },
            if let Some(room) = room() {
                ArrangedPanels {
                    panels,
                    designed: arrangement.layout(room),
                    remembered,
                    choices,
                }
            }
        }
    }
}

/// The panels in the arrangement the operator last made of this dataset's
/// episodes, or else the one designed for their room. Until the operator
/// rearranges them, the designed arrangement follows the room — a new one is
/// a new scope, which the dock takes up whole. Once they do, what they made
/// is kept for the dataset and outlasts every change of room.
#[component]
fn ArrangedPanels(
    panels: Vec<Panel>,
    designed: PanelLayout,
    remembered: RememberedLayout,
    choices: StreamChoices,
) -> Element {
    let kept = use_hook(|| remembered.read());
    let mut arranged_by_hand = use_signal(|| kept.is_some());
    let mut layout = use_signal(|| kept.unwrap_or_else(|| designed.clone()));
    // Asking for a plot brings Plots forward when it waits behind a camera.
    let asks = choices.plot_asks();
    use_effect(use_reactive(&asks, move |asks| {
        if asks > 0 {
            layout
                .write()
                .activate(&PanelId::new(EpisodeArrangement::PLOTS));
        }
    }));
    let mut closing = choices;
    // Taking up the dataset's scope loads the layout as it stands, so the
    // change that made it the operator's is not undone.
    let (scope, initial) = if arranged_by_hand() {
        (Some(remembered.scope()), layout.peek().clone())
    } else {
        (designed.encode(), designed.clone())
    };
    rsx! {
        PanelWorkspace {
            panels,
            layout,
            layout_scope: scope,
            initial_layout: initial,
            reset_layout: designed,
            on_layout_change: move |changed: PanelLayout| {
                remembered.keep(&changed);
                if !arranged_by_hand() {
                    arranged_by_hand.set(true);
                }
            },
            on_panel_close: move |panel: PanelId| {
                if let Some(camera) = EpisodeArrangement::camera_key(&panel) {
                    closing.set_camera(camera, false);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measured_camera_leads_and_an_unmeasured_view_looks_at_the_workspace() {
        assert_eq!(
            FeaturedCamera::measured(Some("/top")),
            FeaturedCamera::Named("/top".into())
        );
        assert_eq!(FeaturedCamera::measured(None), FeaturedCamera::Overview);
    }

    #[test]
    fn asking_for_the_same_span_again_is_a_new_ask() {
        let first = PlayRequest::span((8.0, 12.0)).after(None);
        let again = PlayRequest::span((8.0, 12.0)).after(Some(first));
        assert_ne!(first, again);
        assert_eq!((first.start, first.end), (again.start, again.end));
        assert_eq!(PlayRequest::span((8.0, 8.0)), PlayRequest::instant(8.0));
    }
}
