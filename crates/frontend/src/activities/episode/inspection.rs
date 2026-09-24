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
    camera::EpisodeCamera,
    choices::StreamChoices,
    identity::Episode,
    plots::PlotsPanel,
    robot::{RobotModel, RobotStage},
    streams::{RecordedStream, StreamCatalog},
    timeline::{EpisodeTimeline, EvidenceTrack},
    transport::EpisodeTransport,
};
use crate::components::icons::CameraStreamIcon;
use crate::components::timeline::KeyboardFocus;
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

/// One ask to play a span, and which ask it is.
///
/// The count is what makes two asks different values. A span alone made
/// asking for the same one twice indistinguishable from asking once, and the
/// view — which reacts only to a change — sat still the second time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities) struct PlayRequest {
    span: (f64, f64),
    asked: usize,
}

impl PlayRequest {
    pub(in crate::activities) fn new(span: (f64, f64)) -> Self {
        Self { span, asked: 0 }
    }
    /// The ask after `previous`, which differs from it even for the same span.
    pub(in crate::activities) fn after(previous: Option<Self>, span: (f64, f64)) -> Self {
        Self {
            span,
            asked: previous.map_or(0, |previous| previous.asked + 1),
        }
    }
}

#[component]
pub(in crate::activities) fn EpisodeInspection(
    episode: Episode,
    /// Rows drawn above the recorded streams on the timeline.
    #[props(default)]
    tracks: Vec<EvidenceTrack>,
    /// The span to play, such as the row the operator chose. Each new ask
    /// plays it; `None` leaves the playhead where the operator put it.
    #[props(default)]
    focus: Option<PlayRequest>,
    /// Which camera leads the others.
    #[props(default)]
    featured_camera: FeaturedCamera,
) -> Element {
    // Another episode is another clock, other streams and other choices, so
    // it mounts afresh rather than rewiring the previous one's.
    rsx! {
        Remount { identity: format!("{}/{}", episode.dataset().label(), episode.id()),
            InspectedEpisode { episode, tracks, focus, featured_camera }
        }
    }
}

/// An inspected episode once its streams are known: the streams its panels
/// show, the clock they all read, the operator's choices among the streams,
/// and the dataset's robot model.
#[derive(Clone, PartialEq)]
pub(super) struct LoadedEpisode {
    catalog: StreamCatalog,
    transport: EpisodeTransport,
    choices: StreamChoices,
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
}

#[component]
fn InspectedEpisode(
    episode: Episode,
    tracks: Vec<EvidenceTrack>,
    focus: Option<PlayRequest>,
    featured_camera: FeaturedCamera,
) -> Element {
    let transport = EpisodeTransport::use_episode(&episode);
    transport.use_page_keys();
    let mut asked = transport.clone();
    use_effect(use_reactive(&focus, move |focus| {
        if let Some(focus) = focus {
            asked.play_span(focus.span);
        }
    }));
    let selection = episode.selection();
    let mut catalog = use_resource(move || StreamCatalog::load(selection.clone()));
    let robot = RobotModel::use_dataset(episode.dataset());
    let choices = StreamChoices::use_choices();
    let range = episode.range();
    use_effect(move || {
        if let Some(Ok(catalog)) = &*catalog.read() {
            let mut choices = choices;
            choices.open(catalog, range);
        }
    });
    let mut keys = transport.clone();
    let loaded = match (&*catalog.read(), &*robot.read()) {
        (Some(Ok(streams)), Some(_)) => Ok(Some(LoadedEpisode {
            catalog: streams.clone(),
            transport: transport.clone(),
            choices,
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
                Ok(Some(loaded)) => rsx! { InspectionDock { loaded, evidence: tracks, featured_camera } },
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
    evidence: Vec<EvidenceTrack>,
    featured_camera: FeaturedCamera,
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
                rsx! { EpisodeCamera { request: camera.read().clone(), transport: transport.clone(), title } },
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
    let arrangement =
        EpisodeArrangement::new(panels.iter().map(|panel| panel.id.clone()).collect())
            .led_by_evidence(!evidence.is_empty());
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
                ArrangedPanels { panels, arranged: arrangement.layout(room), choices }
            }
        }
    }
}

/// The panels in the arrangement chosen for their room, where the operator
/// may rearrange them until the room calls for another arrangement — a new
/// arrangement is a new scope, which the dock takes up whole.
#[component]
fn ArrangedPanels(panels: Vec<Panel>, arranged: PanelLayout, choices: StreamChoices) -> Element {
    let mut layout = use_signal(|| arranged.clone());
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
    rsx! {
        PanelWorkspace {
            panels,
            layout,
            layout_scope: arranged.encode(),
            initial_layout: arranged,
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
        let first = PlayRequest::after(None, (8.0, 12.0));
        let again = PlayRequest::after(Some(first), (8.0, 12.0));
        assert_ne!(first, again);
        assert_eq!(first.span, again.span);
    }
}
