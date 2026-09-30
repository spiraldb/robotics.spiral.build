//! Score progress: score every episode that reaches the node with the pinned
//! progress model on the Modal service, and keep the stretches of it
//! progressing fast enough.
//!
//! Each scored frame is a moment with the progress rate it made; how fast is
//! fast enough is the node's own rule, and the moments it keeps join into the
//! stretches the node hands on, which is what its view publishes and what
//! Break down reads. The rule is reviewed as every rule is, and each episode
//! is drawn with `MomentEvidence`. The scores are stored once scored; what
//! the node reads is its own step run over them for each episode's intervals
//! that reach it, held for the session, so moving the threshold scores
//! nothing again.
mod moment;
mod scoring;

pub(super) use moment::Moment;
pub(super) use scoring::{MAX_MOMENTS, ProgressScorer};

use super::preview::{PreviewPlan, PreviewProgress};
use super::primer::{NodePrimer, Primer};
use super::review::{Counted, FailedEpisodes, NodeBar, NodeNotes, NodeReach};
use super::select::{ProgressRule, RuleReview, RuleStrip, RuleView};
use super::state::{CurationController, RunState};
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, HeadingSubject, PlayRequest, StreamCatalog,
};
use crate::components::icons::SpinnerIcon;
use crate::components::timeline::{
    NumericReading, NumericTrack, TimelineInterval, TimelineSample, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone};

/// The episodes reaching one Score progress node, and where its preview
/// stands over them.
#[derive(Clone, PartialEq)]
struct ScoringInput {
    episodes: Vec<Episode>,
    preview: PreviewProgress,
}

impl ScoringInput {
    fn of(controller: CurationController, node: &str) -> Option<Self> {
        let sample = controller.sample()?;
        let (input, _) = controller.populations(node)?;
        let episodes = input
            .episodes()
            .iter()
            .filter_map(|id| sample.episode(id).cloned())
            .collect::<Vec<_>>();
        Some(Self {
            episodes,
            preview: controller.progress(node)?,
        })
    }
    /// The episodes a Run scores: not read yet, or read from intervals that
    /// have changed since.
    fn pending(&self) -> Vec<Episode> {
        let pending = self.preview.pending();
        self.episodes
            .iter()
            .filter(|episode| pending.contains(episode.id()))
            .cloned()
            .collect()
    }
    /// Why the episodes that failed did, when one reason stands for every
    /// one of them, such as the Modal service not being set up.
    fn failure(&self) -> Option<&str> {
        let mut reasons = self
            .preview
            .failed()
            .iter()
            .map(|(_, reason)| reason.as_str());
        let first = reasons.next()?;
        reasons.all(|reason| reason == first).then_some(first)
    }
    fn failed(&self) -> Vec<Episode> {
        self.episodes
            .iter()
            .filter(|episode| {
                self.preview
                    .failed()
                    .iter()
                    .any(|(id, _)| id == episode.id())
            })
            .cloned()
            .collect()
    }
}

/// The workspace shown for a selected Score progress node: its threshold on
/// the node's bar beside the run, and the stretches it keeps and leaves out.
#[component]
pub(super) fn ScoringWorkspace(controller: CurationController, node: String) -> Element {
    // The preview runs this node's step, the one a published view chains,
    // over the intervals that reach it, one episode bound at a time. The
    // pass is the curation's, not this view's: choosing another node leaves
    // it scoring. Stop ends it at once; the episodes in flight stay unscored.
    let run = controller.run_state(&node);
    let problem = use_signal(|| Option::<String>::None);
    let start = {
        let node = node.clone();
        move |episodes: Vec<Episode>| {
            let episodes = episodes
                .iter()
                .map(|episode| episode.id().clone())
                .collect::<Vec<_>>();
            if !episodes.is_empty() {
                let mut problem = problem;
                problem.set(controller.run_preview(&node, &episodes).err());
            }
        }
    };
    let running = run != RunState::Idle;
    let input = ScoringInput::of(controller, &node);
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
    .map(|_| format!("Progress is scored from {camera}, which this dataset does not record"));
    let uncompiled = controller
        .with_payload(|payload| PreviewPlan::check(payload, &node))
        .err()
        .map(|error| error.message().to_owned());
    let (Some(input), Some(view)) = (input, RuleView::of(controller, &node)) else {
        return rsx! {
            section { class: "curation-score-progress-workspace",
                match controller.sample_error() {
                    Some(error) => rsx! {
                        Notice { tone: NoticeTone::Danger, "The episodes could not be read: {error}" }
                    },
                    None => rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Loading episodes…" } } },
                }
            }
        };
    };
    let pending = input.pending();
    let failed = input.failed();
    let unscored = pending.len();
    let stale = input
        .preview
        .is_stale()
        .then(|| "Out of date: what reaches this node changed since it ran".to_owned());
    let failures = FailedEpisodes::note(failed.len()).map(|note| match input.failure() {
        Some(reason) => format!("{note}: {reason}"),
        None => note,
    });
    let blocked = unsupported.is_some() || uncompiled.is_some();
    // Nothing scored yet: the workspace says what scoring is for, beside the
    // run that starts it, rather than an empty strip, map, and index.
    let unmeasured = input.preview.is_unmeasured();
    let running_line = controller
        .activity()
        .get(&node)
        .cloned()
        .unwrap_or_else(|| "Scoring…".to_owned());
    let strips = view.strips();
    let threshold = strips.first().map(RuleStrip::threshold);
    rsx! {
        section { class: "curation-score-progress-workspace", "aria-label": "Score progress",
            NodeBar {
                lead: rsx! { ProgressRule { threshold } },
                note: NodeNotes::join([unsupported.clone(), uncompiled.clone(), problem(), stale, failures]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! {
                    if !unmeasured {
                        p { class: "curation-threshold-tally", role: "status", "{view.tally()}" }
                    }
                    if running {
                        Button {
                            purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                            disabled: run == RunState::Stopping,
                            title: "Stop scoring; the episodes in flight stay unscored",
                            onclick: {
                                let node = node.clone();
                                move |_| controller.stop_run(&node)
                            },
                            if run == RunState::Stopping { "Stopping…" } else { "Stop" }
                        }
                    } else {
                        if !failed.is_empty() {
                            Button {
                                purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                                disabled: blocked,
                                title: "Score the {failed.len()} episodes that failed again",
                                onclick: {
                                    let start = start.clone();
                                    let failed = failed.clone();
                                    move |_| start(failed.clone())
                                },
                                "Retry failed"
                            }
                        }
                        // Scoring runs on the Modal service and what it scores
                        // is stored, so no later Run or Publish scores an
                        // episode again. Before the first score the primer
                        // holds the run.
                        if !pending.is_empty() && !unmeasured {
                            Button {
                                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                                disabled: blocked,
                                title: "Score the {unscored} episodes not yet scored",
                                onclick: {
                                    let start = start.clone();
                                    let fresh = pending.clone();
                                    move |_| start(fresh.clone())
                                },
                                if unscored < input.episodes.len() { "Run {unscored} left" } else { "Run" }
                            }
                        }
                    }
                },
            }
            if unmeasured {
                NodePrimer {
                    primer: Primer::ScoreProgress,
                    action: rsx! {
                        if running {
                            p { class: "curation-primer-status", role: "status",
                                SpinnerIcon {}
                                "{running_line}"
                            }
                        } else if !pending.is_empty() {
                            Button {
                                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                                disabled: blocked,
                                title: "Score the {unscored} episodes not yet scored",
                                onclick: {
                                    let start = start.clone();
                                    let fresh = pending.clone();
                                    move |_| start(fresh.clone())
                                },
                                "Score {Counted::episodes(unscored)}"
                            }
                        }
                    },
                }
            } else {
                RuleReview { view, strips }
            }
        }
    }
}

/// Progress rate: the scorer's `velocity`, progress per unit time against the
/// expert demonstrations it was trained on, so 1× is the expert rate, below
/// it slower, and below zero the task went backwards.
pub(super) struct ProgressRate;

impl ProgressRate {
    /// How a progress rate is printed wherever it is read.
    pub(super) const READING: NumericReading = NumericReading::new(2, "×");
    /// The mean over the moments the scorer could read.
    pub(super) fn mean(moments: &[Moment]) -> Option<f64> {
        let read = moments
            .iter()
            .filter_map(Moment::progress_rate)
            .collect::<Vec<_>>();
        (!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64)
    }
    pub(super) fn label(rate: f64) -> String {
        format!("{rate:.2}×")
    }
}

/// How one moment reads against the node's cutoff, for the timeline to mark.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MomentMark {
    /// Not read by the scorer, and so kept.
    Unread,
    /// Below the cutoff, and so left out.
    Below,
}

impl MomentMark {
    fn of(rate: Option<f64>, cutoff: Option<f64>) -> Option<Self> {
        match (rate, cutoff) {
            (None, _) => Some(Self::Unread),
            (Some(rate), Some(cutoff)) if rate < cutoff => Some(Self::Below),
            _ => None,
        }
    }
    /// The timeline's intervals: each run of touching moments marked alike
    /// as one interval, so an episode of thousands of frames draws a few.
    fn intervals(moments: &[Moment], cutoff: Option<f64>) -> Vec<TimelineInterval> {
        let mut runs = Vec::<(Self, f64, f64)>::new();
        for moment in moments {
            let Some(mark) = Self::of(moment.progress_rate(), cutoff) else {
                continue;
            };
            match runs.last_mut() {
                Some((last, _, end)) if *last == mark && *end == moment.start_time() => {
                    *end = moment.end_time();
                }
                _ => runs.push((mark, moment.start_time(), moment.end_time())),
            }
        }
        runs.into_iter()
            .map(|(mark, start, end)| match (mark, cutoff) {
                (Self::Unread, _) => {
                    TimelineInterval::unknown(start, end, "Not read by the scorer, so kept")
                }
                (Self::Below, cutoff) => TimelineInterval::excluded(
                    start,
                    end,
                    format!("Progress rate below {:.2}×", cutoff.unwrap_or_default()),
                ),
            })
            .collect()
    }
}

/// One episode as its moments measured it: the recording with its progress
/// rate drawn
/// on the timeline, the stretches below `cutoff` marked as left out, and the
/// frames the scorer could not read marked as unknown.
#[component]
pub(super) fn MomentEvidence(
    episode: Episode,
    moments: Vec<Moment>,
    /// What places the recording, under its name.
    facts: Vec<String>,
    /// The threshold the node keeps moments by; `None` where nothing decides.
    cutoff: Option<f64>,
    /// The ask to play a stretch, counted, so choosing it again replays it.
    play: Option<PlayRequest>,
) -> Element {
    let samples = moments
        .iter()
        .filter_map(|row| {
            let middle = f64::midpoint(row.start_time(), row.end_time());
            Some(TimelineSample::new(middle, row.progress_rate()?))
        })
        .collect();
    let intervals = MomentMark::intervals(&moments, cutoff);
    let track = TimelineTrack::new(
        "curation-progress",
        "Progress rate",
        NumericTrack::new(samples, cutoff, intervals)
            .read_as(ProgressRate::READING)
            .with_reference(1.0, "1×", "1× is the expert rate"),
    );
    rsx! {
        EpisodeHeading { subject: HeadingSubject::episode(&episode).with_facts(facts) }
        div { class: "curation-review-player",
            EpisodeInspection {
                episode,
                tracks: vec![track.into()],
                focus: play,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    #[test]
    fn touching_moments_marked_alike_are_one_interval() {
        let episode = EpisodeId::fixture(1);
        let moment = |start: f64, rate: Option<f64>| {
            Moment::new(episode.clone(), (start, start + 0.5), rate).unwrap()
        };
        let moments = [
            moment(0.0, Some(0.1)),
            moment(0.5, Some(0.2)),
            moment(1.0, None),
            moment(1.5, Some(0.9)),
            moment(2.0, Some(0.1)),
        ];
        let marks = |cutoff| MomentMark::intervals(&moments, cutoff).len();
        // Below 0.5: [0, 1) and [2, 2.5); unread: [1, 1.5).
        assert_eq!(marks(Some(0.5)), 3);
        // Nothing decides, so only the unread frame is marked.
        assert_eq!(marks(None), 1);
    }
}
