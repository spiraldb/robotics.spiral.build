//! Identify unproductive moments: score every episode that reaches the node
//! with the native progress model, and keep each second of it as a moment
//! with the progress it made.
//!
//! This node measures and never decides. What counts as unproductive is the
//! threshold of a Select placed after it, which reads the same moments and
//! draws them with `MomentEvidence`, the one recording view both share.
mod scoring;

use super::document::{CurationNode, CurationNodeKind, MomentDetection, MomentRow};
use super::review::{Counted, MeasurementStatus, NoSubtaskChosen, SliceEntry, SubtaskReview};
use super::state::CurationController;
use crate::activities::episode::{Episode, EpisodeInspection, PlayRequest};
use crate::components::timeline::{NumericTrack, TimelineInterval, TimelineSample, TimelineTrack};
use crate::format::ClockPresentation as _;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use scoring::MomentPass;
use spiral_robotics_common::curation::progress::ProgressSelection;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone};

/// The episodes reaching one detection node, and what it holds for each.
#[derive(Clone, PartialEq)]
struct DetectionInput {
    episodes: Vec<Episode>,
    moments: Vec<MomentRow>,
    settings: MomentDetection,
}

impl DetectionInput {
    fn of(controller: CurationController, node: &str) -> Option<Self> {
        let sample = controller.sample()?;
        let (input, _) = controller.populations(node)?;
        controller.with_payload(|payload| {
            let settings = match payload.node(node).map(CurationNode::kind) {
                Some(CurationNodeKind::IdentifyUnproductiveMoments(settings)) => settings.clone(),
                _ => return None,
            };
            let episodes = input
                .episodes()
                .iter()
                .filter_map(|id| sample.episode(id).cloned())
                .collect::<Vec<_>>();
            let moments = payload
                .moments_for(node)
                .filter(|row| episodes.iter().any(|episode| episode.id() == row.episode()))
                .cloned()
                .collect();
            Some(Self {
                episodes,
                moments,
                settings,
            })
        })
    }
    fn pending(&self) -> Vec<Episode> {
        self.episodes
            .iter()
            .filter(|episode| !self.settings.is_completed(episode.id()))
            .cloned()
            .collect()
    }
    fn failed(&self) -> Vec<Episode> {
        self.episodes
            .iter()
            .filter(|episode| {
                self.settings
                    .failed_episodes()
                    .iter()
                    .any(|failure| failure.episode() == episode.id())
            })
            .cloned()
            .collect()
    }
    fn moments_of(&self, episode: &EpisodeId) -> Vec<MomentRow> {
        self.moments
            .iter()
            .filter(|row| row.episode() == episode)
            .cloned()
            .collect()
    }
    /// The status line: how far the node has read its input, in its units.
    fn facts(&self, running: bool) -> String {
        let scored = self
            .episodes
            .iter()
            .filter(|episode| self.settings.is_completed(episode.id()))
            .count();
        let failed = self.failed().len();
        let mut facts = format!(
            "{scored}/{} episodes · {}",
            self.episodes.len(),
            Counted::moments(self.moments.len())
        );
        if failed > 0 {
            facts.push_str(&format!(" · {failed} failed"));
        }
        if running {
            facts.insert_str(0, "Scoring one at a time · ");
        }
        facts
    }
}

/// The workspace shown for a selected Identify unproductive moments node.
#[component]
pub(super) fn DetectionWorkspace(controller: CurationController, node: String) -> Element {
    let studio = StudioProjectState::current();
    let mut chosen = use_signal(|| None::<EpisodeId>);
    // `running` is what the pass is doing; `wanted` what the operator asked
    // for. Stop clears `wanted`, and the recording being scored still lands.
    let mut running = use_signal(|| false);
    let mut wanted = use_signal(|| false);
    let start = {
        let node = node.clone();
        move |episodes: Vec<Episode>| {
            if *running.peek() || episodes.is_empty() {
                return;
            }
            wanted.set(true);
            running.set(true);
            let pass = MomentPass::new(node.clone(), controller, studio, wanted);
            spawn(async move {
                pass.run(episodes).await;
                running.set(false);
                wanted.set(false);
            });
        }
    };
    let Some(input) = DetectionInput::of(controller, &node) else {
        return rsx! {
            section { class: "curation-moments-workspace",
                match controller.sample_error() {
                    Some(error) => rsx! {
                        Notice { tone: NoticeTone::Danger, "The episodes could not be read: {error}" }
                    },
                    None => rsx! { EmptyState { p { "Reading episodes…" } } },
                }
            }
        };
    };
    // The scorer reads the original recordings, which only some datasets
    // have bound; elsewhere the node says so rather than failing per episode.
    let unsupported = controller.with_payload(|payload| {
        payload
            .dataset()
            .map(|dataset| ProgressSelection::for_dataset(dataset).validate())
            .and_then(Result::err)
    });
    let pending = input.pending();
    let failed = input.failed();
    let shown = chosen()
        .or_else(|| input.moments.first().map(|row| row.episode().clone()))
        .and_then(|id| {
            input
                .episodes
                .iter()
                .find(|episode| episode.id() == &id)
                .cloned()
        });
    rsx! {
        section { class: "curation-moments-workspace", "aria-label": "Identify unproductive moments",
            MeasurementStatus {
                facts: input.facts(running()),
                note: unsupported.clone(),
                actions: rsx! {
                    if running() {
                        Button {
                            purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                            disabled: !wanted(),
                            title: "Score no further episode; the one being scored is kept",
                            onclick: move |_| wanted.set(false),
                            if wanted() { "Stop" } else { "Stopping…" }
                        }
                    } else {
                        if !pending.is_empty() {
                            Button {
                                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                                disabled: unsupported.is_some(),
                                title: "Score the {pending.len()} episodes not yet scored",
                                onclick: {
                                    let mut start = start.clone();
                                    let pending = pending.clone();
                                    move |_| start(pending.clone())
                                },
                                if pending.len() < input.episodes.len() { "Run {pending.len()} left" } else { "Run" }
                            }
                        }
                        if !failed.is_empty() {
                            Button {
                                purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                                disabled: unsupported.is_some(),
                                title: "Score the {failed.len()} episodes that failed again",
                                onclick: {
                                    let mut start = start.clone();
                                    let failed = failed.clone();
                                    move |_| start(failed.clone())
                                },
                                "Retry failed"
                            }
                        }
                    }
                },
            }
            SubtaskReview {
                index: rsx! {
                    ol { class: "curation-entries", "aria-label": "Episodes, each with the seconds it was scored in",
                        for episode in input.episodes.iter() {
                            {
                                let id = episode.id().clone();
                                let moments = input.moments_of(&id);
                                let failure = input
                                    .settings
                                    .failed_episodes()
                                    .iter()
                                    .find(|failure| failure.episode() == &id)
                                    .map(|failure| failure.reason().to_owned());
                                let facts = match (&failure, moments.is_empty()) {
                                    (Some(reason), _) => vec![format!("Failed: {reason}")],
                                    (None, true) => vec!["Not scored yet".to_owned()],
                                    (None, false) => vec![
                                        episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                                        Counted::moments(moments.len()).to_string(),
                                    ],
                                };
                                rsx! {
                                    SliceEntry {
                                        key: "{id}",
                                        title: Episode::name_of(&id),
                                        facts,
                                        place: (0.0, 1.0),
                                        score: MomentShare::of(&moments).label(),
                                        chosen: shown.as_ref().is_some_and(|shown| shown.id() == &id),
                                        on_select: move |()| chosen.set(Some(id.clone())),
                                    }
                                }
                            }
                        }
                    }
                },
                evidence: match shown {
                    Some(episode) => rsx! {
                        MomentEvidence {
                            key: "{episode.id()}",
                            moments: input.moments_of(episode.id()),
                            episode,
                        }
                    },
                    None => rsx! {
                        NoSubtaskChosen { prompt: "Run the detection, then choose an episode to watch its progress." }
                    },
                },
            }
        }
    }
}

/// How fast an episode progressed on the whole: its mean over the seconds the
/// scorer could read, as an index row's measure.
struct MomentShare(Option<f64>);

impl MomentShare {
    fn of(moments: &[MomentRow]) -> Self {
        let read = moments
            .iter()
            .filter_map(MomentRow::velocity)
            .collect::<Vec<_>>();
        Self((!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64))
    }
    fn label(&self) -> Option<String> {
        self.0.map(|mean| format!("{mean:.2}"))
    }
}

/// One episode as its moments measured it: the recording with its progress
/// drawn on the timeline, the stretches below `cutoff` marked as left out,
/// and the seconds the scorer could not read marked as unknown.
#[component]
pub(super) fn MomentEvidence(
    episode: Episode,
    moments: Vec<MomentRow>,
    /// The threshold a Select keeps moments by; `None` where nothing decides.
    #[props(default)]
    cutoff: Option<f64>,
    /// The stretch to play.
    #[props(default)]
    focus: Option<(f64, f64)>,
) -> Element {
    let samples = moments
        .iter()
        .filter_map(|row| {
            let middle = f64::midpoint(row.start_time(), row.end_time());
            Some(TimelineSample::new(middle, row.velocity()?))
        })
        .collect();
    let intervals = moments
        .iter()
        .filter_map(|row| {
            let span = (row.start_time(), row.end_time());
            match (row.velocity(), cutoff) {
                (None, _) => Some(TimelineInterval::unknown(
                    span.0,
                    span.1,
                    "The scorer could not read this second, so it is kept",
                )),
                (Some(velocity), Some(cutoff)) if velocity < cutoff => {
                    Some(TimelineInterval::excluded(
                        span.0,
                        span.1,
                        format!("Progress {velocity:.2}, below {cutoff}"),
                    ))
                }
                _ => None,
            }
        })
        .collect();
    let track = TimelineTrack::new(
        "curation-progress",
        "Progress",
        NumericTrack::new(samples, cutoff, intervals),
    );
    rsx! {
        div { class: "curation-review-player",
            EpisodeInspection {
                episode,
                tracks: vec![track.into()],
                focus: focus.map(PlayRequest::new),
            }
        }
    }
}
