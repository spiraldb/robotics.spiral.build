//! One episode as a Select judged it: its recording with the ranges the
//! Select keeps on the timeline, and the evidence of every condition — the
//! progress rate against its cutoff, the objects counted against theirs, the
//! subtasks judged — and the tracked objects boxed on their camera when a
//! condition reads them.
use super::SelectView;
use crate::activities::curation::document::{CurationNode, CurationNodeKind, SelectCondition};
use crate::activities::curation::score_progress::ProgressRate;
use crate::activities::curation::track_objects::StoredTracks;
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, EvidenceTrack, FeaturedCamera, HeadingSubject,
    PlayRequest,
};
use crate::components::timeline::{
    IntervalTrack, NumericReading, NumericTrack, TimelineInterval, TimelineSample, TimelineTrack,
};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;

impl SelectView {
    /// The rows `episode`'s timeline draws for this Select: what it keeps,
    /// then each condition's evidence.
    fn tracks(&self, episode: &Episode) -> Vec<EvidenceTrack> {
        let id = episode.id();
        let kept = self
            .kept(id)
            .ranges()
            .iter()
            .map(|(start, end)| {
                let range = format!("{} – {}", start.clock_label(), end.clock_label());
                TimelineInterval::labeled(*start, *end, range)
            })
            .collect();
        let mut tracks = vec![IntervalTrack::new("curation-kept", "Kept", kept).into()];
        for condition in self.selection.conditions() {
            match condition {
                SelectCondition::Progressing { node, minimum } => {
                    if let Some(timeline) = self.controller.timeline(node, id) {
                        tracks.push(ProgressRate::track(&timeline, Some(*minimum)).into());
                    }
                }
                SelectCondition::Visible { node, fewest } => {
                    if let Some(timeline) = self.controller.timeline(node, id) {
                        let samples = timeline
                            .moments()
                            .iter()
                            .filter_map(|moment| {
                                Some(TimelineSample::new(moment.start(), moment.value()?))
                            })
                            .collect();
                        let label = self.with_graph(|graph| condition.name(graph));
                        tracks.push(
                            TimelineTrack::new(
                                format!("curation-visible-{node}"),
                                label,
                                NumericTrack::new(samples, Some(*fewest as f64), Vec::new())
                                    .read_as(NumericReading::new(0, "")),
                            )
                            .into(),
                        );
                    }
                }
                SelectCondition::Similar { node, .. }
                | SelectCondition::InCategories { node, .. } => {
                    let rows = self.controller.with_reading(|_, reader| {
                        reader
                            .map(|reader| reader.subtasks(node, id).to_vec())
                            .unwrap_or_default()
                    });
                    let spans = rows
                        .iter()
                        .map(|row| {
                            let judged = match (row.similarity(), row.category()) {
                                (Some(similarity), _) => {
                                    format!("{} · {:.0}%", row.description(), similarity * 100.0)
                                }
                                (None, Some(category)) => {
                                    format!("{} · {category}", row.description())
                                }
                                _ => row.description().to_owned(),
                            };
                            TimelineInterval::labeled(row.start_time(), row.end_time(), judged)
                        })
                        .collect();
                    tracks.push(
                        IntervalTrack::new(format!("curation-subtasks-{node}"), "Subtasks", spans)
                            .into(),
                    );
                }
                SelectCondition::Episodes(_) => {}
            }
        }
        tracks
    }
    /// Where the first Track objects node a condition reads stored its
    /// tracks, so the episode opens on its camera with every object boxed.
    fn stored_tracks(&self) -> Option<StoredTracks> {
        self.selection.conditions().iter().find_map(|condition| {
            let node = match condition {
                SelectCondition::Episodes(condition) => condition.number().node()?,
                SelectCondition::Visible { node, .. } => node,
                _ => return None,
            };
            self.with_graph(|graph| match graph.node(node).map(CurationNode::kind) {
                Some(CurationNodeKind::TrackObjects(tracking)) => StoredTracks::of(tracking),
                _ => None,
            })
        })
    }
}

/// The episode opened beside a Select's index.
#[component]
pub(super) fn SelectEvidence(
    view: SelectView,
    episode: Episode,
    play: Option<PlayRequest>,
) -> Element {
    let tracked = view.stored_tracks();
    let boxes = use_resource(use_reactive(
        (&episode, &tracked),
        move |(episode, tracked)| async move {
            match tracked {
                Some(tracked) => tracked.boxes(&episode).await.ok(),
                None => None,
            }
        },
    ));
    let camera_boxes = boxes.read().clone().flatten();
    let featured = match &tracked {
        Some(tracked) => FeaturedCamera::Named(tracked.camera().to_owned()),
        None => view.with_graph(|graph| graph.configured_camera()),
    };
    let kept = view.kept(episode.id()).seconds().clock_label();
    let facts = match episode.facts().duration_seconds() {
        Some(duration) => vec![format!("keeps {kept} of {}", duration.clock_label())],
        None => vec![format!("keeps {kept}")],
    };
    let tracks = view.tracks(&episode);
    rsx! {
        EpisodeHeading { subject: HeadingSubject::episode(&episode).with_facts(facts) }
        div { class: "curation-review-player",
            EpisodeInspection {
                episode,
                tracks,
                focus: play,
                featured_camera: featured,
                camera_boxes,
            }
        }
    }
}
