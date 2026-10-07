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

use super::measure_run::{
    MeasureInput, MeasureRun, MeasureRunActions, MeasureRunStart, MeasureVerb,
};
use super::preview::PreviewPlan;
use super::primer::{NodePrimer, Primer};
use super::review::{NodeBar, NodeNotes, NodeReach};
use super::select::{ProgressRule, RuleReview, RuleStrip, RuleView};
use super::state::CurationController;
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, HeadingSubject, PlayRequest, StreamCatalog,
};
use crate::components::icons::SpinnerIcon;
use crate::components::timeline::{
    NumericReading, NumericTrack, TimelineInterval, TimelineSample, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_ui::{EmptyState, Notice, NoticeTone};

/// The workspace shown for a selected Score progress node: its threshold on
/// the node's bar beside the run, and the stretches it keeps and leaves out.
#[component]
pub(super) fn ScoringWorkspace(controller: CurationController, node: String) -> Element {
    // The preview runs this node's step, the one a published view chains,
    // over the intervals that reach it, one episode bound at a time.
    let run = MeasureRun::use_run(controller, &node, MeasureVerb::Score);
    let input = MeasureInput::of(controller, &node);
    // The scorer reads the camera it was trained on, which only some datasets
    // record; elsewhere the node says so rather than failing per episode.
    let camera = ProgressScorer::WarpSimBottles.camera();
    let unsupported = StreamCatalog::use_camera_topics(
        input
            .as_ref()
            .and_then(|input| input.episodes().first())
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
    let blocked = unsupported.is_some() || uncompiled.is_some();
    // Nothing scored yet: the workspace says what scoring is for, beside the
    // run that starts it, rather than an empty strip, map, and index.
    let unmeasured = input.preview().is_unmeasured();
    let strips = view.strips();
    let threshold = strips.first().map(RuleStrip::threshold);
    rsx! {
        section { class: "curation-score-progress-workspace", "aria-label": "Score progress",
            NodeBar {
                lead: rsx! { ProgressRule { threshold } },
                note: NodeNotes::join([unsupported, uncompiled, run.problem(), input.stale(), input.failures()]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! {
                    if !unmeasured {
                        p { class: "curation-threshold-tally", role: "status", "{view.tally()}" }
                    }
                    MeasureRunActions { run: run.clone(), input: input.clone(), blocked }
                },
            }
            if unmeasured {
                NodePrimer {
                    primer: Primer::ScoreProgress,
                    action: rsx! { MeasureRunStart { run, input, blocked } },
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
