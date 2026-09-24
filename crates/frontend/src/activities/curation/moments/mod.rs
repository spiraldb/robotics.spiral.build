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
use super::poster::{PosterCut, PosterFrames, PosterStill};
use super::rerun::ConfirmedRerun;
use super::review::{
    Counted, EvidenceHeading, FailedEpisodes, IndexBar, NodeBar, NodeNotes, NodeReach, RowStanding,
    SliceEntry, SliceFace, SliceShape, SubtaskReview, TabStop, TraceLine,
};
use super::state::{CurationController, RunState};
use crate::activities::episode::{
    Episode, EpisodeInspection, EpisodeLabels, PlayRequest, StreamCatalog,
};
use crate::components::timeline::{
    NumericReading, NumericTrack, TimelineInterval, TimelineSample, TimelineTrack,
};
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

/// One episode as the index lists it: scored, failed, or neither yet.
#[derive(Clone, PartialEq)]
enum ListedEpisode {
    Scored { episode: Episode, pace: Option<f64> },
    Failed { episode: Episode, reason: String },
}

impl ListedEpisode {
    fn episode(&self) -> &Episode {
        match self {
            Self::Scored { episode, .. } | Self::Failed { episode, .. } => episode,
        }
    }
}

impl DetectionInput {
    /// The index in the order it is read: failures first, since they need
    /// something done, then the scored episodes slowest first — where the
    /// time a training set would waste is. Episodes not scored yet are only
    /// counted.
    fn listed(&self) -> Vec<ListedEpisode> {
        let mut failed = Vec::new();
        let mut scored = Vec::new();
        for episode in &self.episodes {
            let failure = self
                .settings
                .failed_episodes()
                .iter()
                .find(|failure| failure.episode() == episode.id());
            if let Some(failure) = failure {
                failed.push(ListedEpisode::Failed {
                    episode: episode.clone(),
                    reason: failure.reason().to_owned(),
                });
            } else if self.settings.is_completed(episode.id()) {
                scored.push(ListedEpisode::Scored {
                    episode: episode.clone(),
                    pace: Pace::mean(&self.moments_of(episode.id())),
                });
            }
        }
        let slowest = |listed: &ListedEpisode| match listed {
            ListedEpisode::Scored { pace, .. } => pace.unwrap_or(f64::INFINITY),
            ListedEpisode::Failed { .. } => f64::NEG_INFINITY,
        };
        scored.sort_by(|left, right| slowest(left).total_cmp(&slowest(right)));
        failed.into_iter().chain(scored).collect()
    }
    /// The episode's pace across its recording, drawn small in its row: its
    /// finer curve when the detection kept one, else its moments.
    fn trace(&self, episode: &Episode) -> TraceLine {
        let duration = episode
            .facts()
            .duration_seconds()
            .unwrap_or_default()
            .max(f64::EPSILON);
        let points = match self.settings.curve(episode.id()) {
            Some(curve) => curve
                .samples()
                .map(|(seconds, value)| (seconds / duration, value))
                .collect(),
            None => self
                .moments_of(episode.id())
                .iter()
                .map(|row| {
                    (
                        f64::midpoint(row.start_time(), row.end_time()) / duration,
                        row.velocity(),
                    )
                })
                .collect(),
        };
        TraceLine::new(points, Pace::TRACE_DOMAIN, 1.0)
    }
}

/// The workspace shown for a selected Identify unproductive moments node.
#[component]
pub(super) fn DetectionWorkspace(controller: CurationController, node: String) -> Element {
    let studio = StudioProjectState::current();
    let tabs = OpenEpisodes::use_open();
    // The first episode listed — the slowest scored one, or before anything
    // is scored the first episode at all — opens by itself, so the recording
    // column is never an empty half beside the index.
    let first = node.clone();
    use_effect(move || {
        let opening = DetectionInput::of(controller, &first).and_then(|input| {
            input
                .listed()
                .first()
                .map(|listed| listed.episode().id().clone())
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
    .map(|_| format!("Pace is read from {camera}, which this dataset does not record"));
    let listed = input
        .as_ref()
        .map(DetectionInput::listed)
        .unwrap_or_default();
    let stills = listed
        .iter()
        .map(|listed| {
            let episode = listed.episode();
            PosterStill::within(
                episode.id(),
                (0.0, episode.facts().duration_seconds().unwrap_or_default()),
            )
        })
        .collect::<Vec<_>>();
    let frames = PosterFrames::use_frames(PosterCut::of(
        controller
            .with_payload(|payload| payload.dataset().cloned())
            .as_ref(),
        Some(camera),
        &stills,
    ));
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
    let unscored = pending.len() - failed.len().min(pending.len());
    let rescore = {
        let start = start.clone();
        let every = input.episodes.clone();
        move |()| start(every.clone())
    };
    let shown = tabs.front().map(|tab| tab.episode().clone());
    let chosen = listed
        .iter()
        .position(|listed| shown.as_ref() == Some(listed.episode().id()));
    let labels = EpisodeLabels::of(input.episodes.iter().map(Episode::facts));
    let evidence = input.clone();
    rsx! {
        section { class: "curation-moments-workspace", "aria-label": "Identify unproductive moments",
            NodeBar {
                lead: rsx! {
                    p { class: "curation-node-bar-reads",
                        "Pace against expert demonstrations, read from "
                        code { "{camera}" }
                    }
                },
                note: NodeNotes::join([unsupported.clone(), FailedEpisodes::note(failed.len())]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
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
                        // Scoring runs on this machine and costs nothing, so
                        // it can be asked again — which also draws the finer
                        // pace line for episodes scored before it was kept.
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
                    }
                },
            }
            SubtaskReview {
                index: rsx! {
                    if !listed.is_empty() {
                        IndexBar {
                            p { class: "curation-index-caption", "{Counted::episodes(listed.len())} · slowest first" }
                        }
                    }
                    ol { class: "curation-entries", "aria-label": "Scored episodes, slowest first; each row ends with its mean pace",
                        for (position, listed) in listed.iter().enumerate() {
                            {
                                let episode = listed.episode().clone();
                                let id = episode.id().clone();
                                let label = labels.label(episode.facts());
                                let face = SliceFace::episode(&episode, label.head());
                                let face = match listed {
                                    ListedEpisode::Scored { pace, .. } => face
                                        .shaped(SliceShape::Trace(input.trace(&episode)))
                                        .measured(pace.map(Pace::label).unwrap_or_else(|| "—".to_owned()), Pace::MEANING),
                                    ListedEpisode::Failed { reason, .. } => face.saying(vec![format!("Failed: {reason}")]),
                                };
                                rsx! {
                                    SliceEntry {
                                        key: "{id}",
                                        face,
                                        frames,
                                        standing: RowStanding::plain(chosen == Some(position))
                                            .roving(TabStop::at(position, chosen)),
                                        on_select: move |()| tabs.open(&id, None),
                                    }
                                }
                            }
                        }
                    }
                    if unscored > 0 {
                        p { class: "curation-index-footer",
                            if listed.is_empty() {
                                "{Counted::episodes(unscored)} not scored yet. Run scores them on this machine."
                            } else {
                                "{Counted::episodes(unscored)} not scored yet"
                            }
                        }
                    }
                },
                evidence: rsx! {
                    EpisodeTabs {
                        open: tabs,
                        empty: "Choose an episode to watch its pace.".to_owned(),
                        render: move |tab: OpenEpisode| {
                            let Some(episode) = evidence.episodes.iter().find(|episode| episode.id() == tab.episode()).cloned() else {
                                return rsx! {};
                            };
                            let moments = evidence.moments_of(episode.id());
                            let mut facts = vec![
                                episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                            ];
                            facts.extend(Pace::mean(&moments).map(|pace| format!("mean pace {}", Pace::label(pace))));
                            let curve = evidence.settings.curve(episode.id()).cloned();
                            rsx! { MomentEvidence { moments, curve, episode, facts } }
                        },
                    }
                },
            }
        }
    }
}

/// Pace: progress per unit time against the expert demonstrations the
/// scorer was trained on, so 1× is expert pace, below it slower, and below
/// zero the task went backwards.
pub(super) struct Pace;

impl Pace {
    /// How pace is printed wherever it is read.
    pub(super) const READING: NumericReading = NumericReading::new(2, "×");
    /// The span every row's small pace line shares, so rows compare: a
    /// little below standing still to about twice expert pace.
    const TRACE_DOMAIN: (f64, f64) = (-0.5, 2.0);
    const MEANING: &'static str = "Mean pace against expert demonstrations: 1× is expert pace, below 0 the task went backwards";
    /// The mean over the seconds the scorer could read.
    pub(super) fn mean(moments: &[MomentRow]) -> Option<f64> {
        let read = moments
            .iter()
            .filter_map(MomentRow::velocity)
            .collect::<Vec<_>>();
        (!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64)
    }
    pub(super) fn label(pace: f64) -> String {
        format!("{pace:.2}×")
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
                    "Not read by the scorer, so kept",
                )),
                (Some(velocity), Some(cutoff)) if velocity < cutoff => {
                    Some(TimelineInterval::excluded(
                        span.0,
                        span.1,
                        format!("Pace {velocity:.2}×, below {cutoff:.2}×"),
                    ))
                }
                _ => None,
            }
        })
        .collect();
    let track = TimelineTrack::new(
        "curation-progress",
        "Pace",
        NumericTrack::new(samples, cutoff, intervals)
            .read_as(Pace::READING)
            .with_reference(1.0, "1×", "1× is expert pace"),
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
