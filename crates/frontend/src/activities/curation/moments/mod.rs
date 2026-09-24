//! Identify unproductive moments: score every episode that reaches the node
//! with the native progress model, and keep each second of it as a moment
//! with the progress it made.
//!
//! This node measures and never decides. What counts as unproductive is the
//! threshold of a Select placed after it, which reads the same moments and
//! draws them with `MomentEvidence`, the one recording view both share.
mod scoring;

use super::document::{CurationNode, CurationNodeKind, MomentDetection, MomentRow, ProgressCurve};
use super::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use super::rerun::ConfirmedRerun;
use super::review::{
    EvidenceHeading, FailedEpisodes, MeasurementStatus, SliceEntry, SubtaskReview,
};
use super::state::{CurationController, RunState};
use crate::activities::episode::{Episode, EpisodeInspection, PlayRequest, StreamCatalog};
use crate::components::timeline::{NumericTrack, TimelineInterval, TimelineSample, TimelineTrack};
use crate::format::ClockPresentation as _;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use scoring::MomentPass;
use spiral_robotics_common::curation::progress::ProgressScorer;
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
}

/// The workspace shown for a selected Identify unproductive moments node.
#[component]
pub(super) fn DetectionWorkspace(controller: CurationController, node: String) -> Element {
    let studio = StudioProjectState::current();
    let tabs = OpenEpisodes::use_open();
    // The first scored episode, or before anything is scored the first
    // episode at all, opens by itself, so the recording column is never an
    // empty half beside the index.
    let first = node.clone();
    use_effect(move || {
        let opening = DetectionInput::of(controller, &first).and_then(|input| {
            input
                .moments
                .first()
                .map(|row| row.episode().clone())
                .or_else(|| input.episodes.first().map(|episode| episode.id().clone()))
        });
        tabs.open_first(opening.map(|episode| (episode, None)));
    });
    // The pass is the curation's, not this view's: choosing another node
    // leaves it scoring. Stop asks it to score no further episode, and the
    // recording being scored still lands.
    let run = controller.run_state(&node);
    let start = {
        let node = node.clone();
        move |episodes: Vec<Episode>| {
            if episodes.is_empty() {
                return;
            }
            controller.start_run(&node, move |run| async move {
                MomentPass::new(controller, studio, run).run(episodes).await;
            });
        }
    };
    let running = run != RunState::Idle;
    let input = DetectionInput::of(controller, &node);
    // The scorer reads the camera it was trained on, which only some datasets
    // record; elsewhere the node says so rather than failing per episode.
    let camera = ProgressScorer::WarpSimBottles.camera();
    let unsupported = StreamCatalog::use_camera_topics(
        input
            .as_ref()
            .and_then(|input| input.episodes.first())
            .map(Episode::selection),
    )
    .filter(|cameras| !cameras.iter().any(|topic| topic == camera))
    .map(|_| {
        format!("Progress scoring reads the {camera} camera, which this dataset does not record.")
    });
    let Some(input) = input else {
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
    let pending = input.pending();
    let failed = input.failed();
    let scored = input.episodes.len() - pending.len();
    let rescore = {
        let start = start.clone();
        let every = input.episodes.clone();
        move |()| start(every.clone())
    };
    let shown = tabs.front().map(|tab| tab.episode().clone());
    let evidence = input.clone();
    rsx! {
        section { class: "curation-moments-workspace", "aria-label": "Identify unproductive moments",
            MeasurementStatus {
                note: unsupported.clone().or_else(|| FailedEpisodes::note(failed.len())),
                actions: rsx! {
                    if running {
                        Button {
                            purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                            disabled: run == RunState::Stopping,
                            title: "Score no further episode; the one being scored is kept",
                            onclick: {
                                let node = node.clone();
                                move |_| controller.stop_run(&node)
                            },
                            if run == RunState::Stopping { "Stopping…" } else { "Stop" }
                        }
                    } else {
                        if !pending.is_empty() {
                            Button {
                                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                                disabled: unsupported.is_some(),
                                title: "Score the {pending.len()} episodes not yet scored",
                                onclick: {
                                    let start = start.clone();
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
                                    let start = start.clone();
                                    let failed = failed.clone();
                                    move |_| start(failed.clone())
                                },
                                "Retry failed"
                            }
                        }
                        // Scoring runs on this machine and costs nothing, so
                        // it can be asked again — which also draws the finer
                        // progress line for episodes scored before it was
                        // kept.
                        if scored > 0 && unsupported.is_none() {
                            ConfirmedRerun {
                                id: "curation-moments-rescore".to_owned(),
                                label: "Re-score all".to_owned(),
                                question: format!(
                                    "Score all {} episodes again? This replaces the {} moments already stored.",
                                    input.episodes.len(),
                                    input.moments.len(),
                                ),
                                on_confirm: rescore,
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
                                    // A moment is a second, so its count only
                                    // repeats the length.
                                    (None, false) => vec![
                                        episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                                    ],
                                };
                                rsx! {
                                    SliceEntry {
                                        key: "{id}",
                                        title: Episode::name_of(&id),
                                        facts,
                                        score: MomentShare::of(&moments).label(),
                                        score_meaning: MomentShare::MEANING.to_owned(),
                                        chosen: shown.as_ref() == Some(&id),
                                        on_select: move |()| tabs.open(&id, None),
                                    }
                                }
                            }
                        }
                    }
                },
                evidence: rsx! {
                    EpisodeTabs {
                        open: tabs,
                        empty: "Choose an episode to watch its progress.".to_owned(),
                        render: move |tab: OpenEpisode| {
                            let Some(episode) = evidence.episodes.iter().find(|episode| episode.id() == tab.episode()).cloned() else {
                                return rsx! {};
                            };
                            let moments = evidence.moments_of(episode.id());
                            let facts = vec![
                                episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                            ];
                            let curve = evidence.settings.curve(episode.id()).cloned();
                            rsx! { MomentEvidence { moments, curve, episode, facts } }
                        },
                    }
                },
            }
        }
    }
}

/// How fast an episode progressed on the whole: its mean over the seconds the
/// scorer could read, as an index row's measure.
struct MomentShare(Option<f64>);

impl MomentShare {
    /// What the number in an episode's row is.
    const MEANING: &'static str =
        "Mean progress per second the scorer read; negative means the task went backwards";
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
    /// The episode's finer progress, drawn in place of the moments' own
    /// values when the detection kept it.
    #[props(default)]
    curve: Option<ProgressCurve>,
    /// What places the recording, under its name.
    #[props(default)]
    facts: Vec<String>,
    /// The threshold a Select keeps moments by; `None` where nothing decides.
    #[props(default)]
    cutoff: Option<f64>,
    /// The ask to play a stretch, counted, so choosing it again replays it.
    #[props(default)]
    play: Option<PlayRequest>,
) -> Element {
    let samples = match &curve {
        Some(curve) => curve
            .samples()
            .map(|(seconds, value)| TimelineSample::new(seconds, value.unwrap_or(f64::NAN)))
            .collect(),
        None => moments
            .iter()
            .filter_map(|row| {
                let middle = f64::midpoint(row.start_time(), row.end_time());
                Some(TimelineSample::new(middle, row.velocity()?))
            })
            .collect(),
    };
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
        EvidenceHeading { subject: episode.name(), episode: Some(episode.id().clone()), facts }
        div { class: "curation-review-player",
            EpisodeInspection {
                episode,
                tracks: vec![track.into()],
                focus: play,
            }
        }
    }
}
