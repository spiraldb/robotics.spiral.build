//! One recording seen whole: every camera it recorded, the robot when the
//! dataset has a model, and a surface's own evidence, all on one clock.
//!
//! This is the episode view the whole studio shares — Curation's evidence
//! panels, Explore's dataset node and subtask views — so an operator
//! recognizes the same recording wherever it is shown. A surface says which
//! episode, which camera leads, which rows sit under the recording and which
//! span to play; the clock, the cameras and the robot belong to the episode.
use super::{
    camera::EpisodeCamera,
    identity::Episode,
    robot::RobotStage,
    streams::{RecordedStream, StreamCatalog},
    transport::EpisodeTransport,
};
use crate::components::timeline::{
    IntervalTrack, IntervalTrackRow, NumericTrackRow, Timeline, TimelineAction, TimelineContent,
    TimelineEvent, TimelineInteraction, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

/// A span plays to its own end and a moment past it, so the operator sees the
/// motion leave the interval rather than being cut off inside it.
///
/// There is deliberately no lead-in. The playhead landing half a second before
/// the span it was asked for put the cursor outside the highlight that marks
/// the same interval, and every reading of the clock was then half a second
/// wrong in a view whose whole product is where something starts.
const RUN_OUT_SECONDS: f64 = 0.5;

/// Which of an episode's cameras takes the height in this view.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::activities) enum FeaturedCamera {
    /// The episode's first camera, in its own topic order.
    #[default]
    First,
    /// The camera this view is about, such as the one a measurement read. An
    /// episode that did not record it leads with its first.
    Named(String),
    /// Whichever camera shows the workspace best, for a view that is about the
    /// recording rather than about one camera.
    Overview,
}

impl FeaturedCamera {
    /// The camera a measurement read, else whichever shows the workspace best.
    pub(in crate::activities) fn measured(camera: Option<&str>) -> Self {
        camera.map_or(Self::Overview, |camera| Self::Named(camera.to_owned()))
    }
    /// Where in `shown` the featured camera sits.
    fn leading(&self, shown: &[&RecordedStream]) -> usize {
        let wanted = match self {
            Self::First => None,
            Self::Named(camera) => Some(camera.clone()),
            Self::Overview => {
                let names = shown
                    .iter()
                    .map(|camera| camera.name().to_owned())
                    .collect::<Vec<_>>();
                StreamCatalog::overview_camera(&names).map(str::to_owned)
            }
        };
        wanted
            .and_then(|wanted| shown.iter().position(|camera| camera.name() == wanted))
            .unwrap_or_default()
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
    /// Where the clock plays: from the span's own start to a moment past its
    /// end.
    fn played(self) -> (f64, f64) {
        (self.span.0, self.span.1 + RUN_OUT_SECONDS)
    }
}

/// A row a surface draws under the recording.
#[derive(Clone, PartialEq)]
pub(in crate::activities) enum EvidenceTrack {
    /// A measurement's value over time, such as a progress score.
    Values(TimelineTrack),
    /// Named spans, such as a measurement's subtasks; activating one plays it.
    Intervals(IntervalTrack),
}

impl From<TimelineTrack> for EvidenceTrack {
    fn from(track: TimelineTrack) -> Self {
        Self::Values(track)
    }
}

impl From<IntervalTrack> for EvidenceTrack {
    fn from(track: IntervalTrack) -> Self {
        Self::Intervals(track)
    }
}

#[component]
pub(in crate::activities) fn EpisodeInspection(
    episode: Episode,
    /// Rows drawn under the recording. A row of intervals with none in it is
    /// left out rather than drawn empty.
    #[props(default)]
    tracks: Vec<EvidenceTrack>,
    /// The span to play, such as the row the operator chose. Each new ask
    /// plays it; `None` leaves the playhead where the operator put it.
    #[props(default)]
    focus: Option<PlayRequest>,
    /// Which camera takes the height while the rest sit under it.
    #[props(default)]
    featured_camera: FeaturedCamera,
) -> Element {
    // Another episode is another clock, other cameras and another robot, so
    // it mounts afresh rather than rewiring the previous one's.
    rsx! {
        InspectedEpisode {
            key: "{episode.dataset().label()}/{episode.id()}",
            episode, tracks, focus, featured_camera,
        }
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
    let mut asked = transport.clone();
    use_effect(use_reactive(&focus, move |focus| {
        if let Some((start, end)) = focus.map(PlayRequest::played) {
            asked.play_interval(start, end);
        }
    }));
    let selection = episode.selection();
    let mut cameras = use_resource(move || StreamCatalog::load(selection.clone()));
    let default_interaction = transport.interaction();
    let current = transport.clone();
    let interaction = TimelineInteraction::new(
        transport.timeline_state(),
        EventHandler::new(move |event: TimelineEvent| {
            if event.target() != current.timeline_state().target() {
                return;
            }
            match event.action() {
                TimelineAction::ActivateInterval(interval) => {
                    let (start, end) = PlayRequest::new(interval.span()).played();
                    current.clone().play_interval(start, end);
                }
                action => default_interaction.emit(action),
            }
        }),
    );
    let mut values = Vec::new();
    let mut intervals = Vec::new();
    for track in tracks {
        match track {
            EvidenceTrack::Values(track) => values.push(track),
            EvidenceTrack::Intervals(track) if !track.is_empty() => intervals.push(track),
            EvidenceTrack::Intervals(_) => {}
        }
    }
    let numeric = !values.is_empty();
    let trackless = values.is_empty() && intervals.is_empty();
    let rows = rsx! {
        for track in values { NumericTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
        for track in intervals { IntervalTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
    };
    // Every surface that mounts this composition names the episode directly
    // above it, so the transport's identity slot would only repeat it — as a
    // raw identifier, pressed against the transport. "Tracks" goes with it
    // when there are no numeric rows for it to head.
    let content = TimelineContent::new(rows).with_identity(rsx! {});
    let content = if numeric {
        content
    } else {
        content.with_corner(rsx! {})
    };
    rsx! {
        section {
            class: "episode-inspection",
            "aria-label": "Recorded episode",
            "data-tracks": trackless.then_some("none"),
            match cameras.read().as_ref() {
                Some(Ok(catalog)) => {
                    let shown = catalog.videos().collect::<Vec<_>>();
                    let lead = featured_camera.leading(&shown);
                    let rest = shown
                        .iter()
                        .enumerate()
                        .filter(|(place, _)| *place != lead)
                        .map(|(_, camera)| *camera)
                        .collect::<Vec<_>>();
                    rsx! {
                        if shown.is_empty() {
                            p { class: "episode-inspection-status", "This episode has no recorded cameras." }
                        }
                        div { class: "episode-inspection-tiles",
                            if let Some(camera) = shown.get(lead) {
                                section { class: "episode-inspection-tile episode-inspection-featured",
                                    h5 { title: camera.label(), "{catalog.camera_title(camera)}" }
                                    EpisodeCamera { request: camera.read().clone(), transport: transport.clone() }
                                }
                            }
                            div { class: "episode-inspection-strip",
                                for camera in rest {
                                    section { key: "{camera.key()}", class: "episode-inspection-tile",
                                        h5 { title: camera.label(), "{catalog.camera_title(camera)}" }
                                        EpisodeCamera { request: camera.read().clone(), transport: transport.clone() }
                                    }
                                }
                                RobotStage {
                                    episode: episode.clone(), streams: catalog.clone(),
                                    transport: transport.clone(),
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    div { class: "episode-inspection-status", role: "alert",
                        p { "Could not read recorded cameras: {error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                            onclick: move |_| cameras.restart(), "Retry cameras" }
                    }
                },
                None => rsx! { p { class: "episode-inspection-status", role: "status", "Loading recorded cameras…" } },
            }
            Timeline { interaction, content }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_plays_from_its_own_start_to_a_moment_past_its_end() {
        assert_eq!(PlayRequest::new((8.0, 12.0)).played(), (8.0, 12.5));
    }

    #[test]
    fn asking_for_the_same_span_again_is_a_new_ask() {
        let first = PlayRequest::after(None, (8.0, 12.0));
        let again = PlayRequest::after(Some(first), (8.0, 12.0));
        assert_ne!(first, again);
        assert_eq!(first.played(), again.played());
    }

    #[test]
    fn a_measured_camera_leads_and_an_unmeasured_view_looks_at_the_workspace() {
        assert_eq!(
            FeaturedCamera::measured(Some("/top")),
            FeaturedCamera::Named("/top".into())
        );
        assert_eq!(FeaturedCamera::measured(None), FeaturedCamera::Overview);
    }
}
