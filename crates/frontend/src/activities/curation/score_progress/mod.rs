//! Score progress: score every episode that reaches the node with a progress
//! model on the Modal service — the released one, or one the project
//! published from Training — and add each episode's timeline of progress
//! rates to the table.
//!
//! Each scored frame holds the progress rate it made until the next one. The
//! node keeps nothing by itself: a Select below it keeps the stretches
//! progressing fast enough. Its workspace lists the episodes it scored, each
//! drawn with `MomentEvidence`. The scores are stored once scored, under the
//! model that scored them, and what the node reads is its own entries of each
//! episode, held for the session.
mod scoring;

pub(super) use scoring::ProgressScorer;

use super::document::{CurationNode, CurationNodeKind, ProgressScoring, PublishedProgressModel};
use super::measure_run::{
    MeasureInput, MeasureRun, MeasureRunActions, MeasureRunStart, MeasureVerb, MeasuredEpisode,
    MeasuredEpisodes,
};
use super::measured_timeline::MeasuredTimeline;
use super::preview::PreviewPlan;
use super::primer::{NodePrimer, Primer};
use super::review::{NodeBar, NodeNotes, NodeReach};
use super::state::CurationController;
use super::thumbnail::Thumbnails;
use crate::activities::episode::{
    Episode, EpisodeHeading, EpisodeInspection, HeadingSubject, PlayRequest, StreamCatalog,
};
use crate::components::icons::SpinnerIcon;
use crate::components::timeline::{
    NumericReading, NumericTrack, TimelineInterval, TimelineSample, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_common::training::PublishedModel;
use spiral_robotics_ui::{EmptyState, Notice, NoticeTone, Select};

/// One Score progress node of the open curation.
#[derive(Clone, PartialEq)]
struct ScoredNode {
    id: String,
    controller: CurationController,
}

impl ScoredNode {
    /// The node's configuration, or nothing when the selected node does not
    /// score progress after all.
    fn settings(&self) -> Option<ProgressScoring> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).map(CurationNode::kind) {
                Some(CurationNodeKind::ScoreProgress(scoring)) => Some(scoring.clone()),
                _ => None,
            }
        })
    }
    /// Score with `published`, or with the released model.
    fn score_with(&self, published: Option<PublishedProgressModel>) {
        let id = self.id.clone();
        let _ = self.controller.edit(|payload| {
            match payload.node_mut(&id).map(CurationNode::kind_mut) {
                Some(CurationNodeKind::ScoreProgress(scoring)) => {
                    scoring.set_published(published);
                    Ok(())
                }
                _ => Err("That node does not score progress.".to_owned()),
            }
        });
    }
}

/// The workspace shown for a selected Score progress node: the model it
/// scores with and the run on the node's bar, and the episodes it scored
/// beside the one opened.
#[component]
pub(super) fn ScoringWorkspace(controller: CurationController, node: String) -> Element {
    let scored = ScoredNode {
        id: node.clone(),
        controller,
    };
    let run = MeasureRun::use_run(controller, &node, MeasureVerb::Score);
    let input = MeasureInput::of(controller, &node);
    let settings = scored.settings();
    let scorer = settings
        .as_ref()
        .map_or(ProgressScorer::Released, ProgressScorer::from);
    // The scorer reads the camera its model was trained on, which only some
    // datasets record; elsewhere the node says so rather than failing per
    // episode.
    let camera = scorer.camera().to_owned();
    let thumbnails = Thumbnails::use_camera(controller, Some(&camera));
    let unsupported = StreamCatalog::use_camera_topics(
        input
            .as_ref()
            .and_then(|input| input.episodes().first())
            .map(Episode::selection),
    )
    .filter(|cameras| !cameras.contains(&camera))
    .map(|_| format!("Progress is scored from {camera}, which this dataset does not record"));
    let models = controller.progress_models();
    let available = scorer.availability(models.as_ref());
    let uncompiled = controller
        .with_payload(|payload| PreviewPlan::check(payload, &node))
        .err()
        .map(|error| error.message().to_owned());
    let (Some(input), Some(_)) = (input, settings) else {
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
    let blocked = unsupported.is_some() || uncompiled.is_some() || available.is_err();
    // Nothing scored yet: the workspace says what scoring is for, beside the
    // run that starts it, rather than an empty index.
    let unmeasured = input.preview().is_unmeasured();
    let episodes = input
        .episodes()
        .iter()
        .filter_map(|episode| {
            let timeline = controller.timeline(&node, episode.id())?;
            let mean = timeline
                .mean_within((0.0, f64::INFINITY))
                .map(ProgressRate::label);
            Some(MeasuredEpisode::new(episode.clone(), mean))
        })
        .collect::<Vec<_>>();
    let listed = models.and_then(Result::ok);
    rsx! {
        section { class: "curation-score-progress-workspace", "aria-label": "Score progress",
            NodeBar {
                lead: rsx! { ScoringSettings { scored, scorer, models: listed } },
                note: NodeNotes::join([unsupported, uncompiled, available.err().flatten(), run.problem(), input.failures()]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! { MeasureRunActions { run: run.clone(), input: input.clone(), blocked } },
            }
            if unmeasured {
                NodePrimer {
                    primer: Primer::ScoreProgress,
                    action: rsx! { MeasureRunStart { run, input, blocked } },
                }
            } else {
                MeasuredEpisodes {
                    episodes,
                    meaning: "Mean progress rate; 1× is the expert rate",
                    thumbnails,
                    render: move |(episode, play): (Episode, Option<PlayRequest>)| {
                        let timeline = controller.timeline(&node, episode.id()).unwrap_or_default();
                        rsx! { MomentEvidence { episode, timeline, facts: Vec::new(), cutoff: None, play } }
                    },
                }
            }
        }
    }
}

/// "Score with [model]": the released model, or one the project published,
/// each by its name. `models` is `None` until they are read, or when they
/// cannot be.
#[component]
fn ScoringSettings(
    scored: ScoredNode,
    scorer: ProgressScorer,
    models: Option<Vec<PublishedModel>>,
) -> Element {
    const RELEASED: &str = "released";
    const UNLISTED: &str = "unlisted";
    let listed = models.clone().unwrap_or_default();
    let chosen = match scorer {
        ProgressScorer::Released => RELEASED.to_owned(),
        ProgressScorer::Published(_) => scorer
            .listed_in(&listed)
            .map_or_else(|| UNLISTED.to_owned(), |place| place.to_string()),
    };
    let choices = listed.clone();
    rsx! {
        div { class: "curation-score-progress-settings",
            label { class: "curation-score-progress-label", r#for: "curation-score-progress-model", "Score with" }
            Select {
                id: "curation-score-progress-model",
                value: chosen.clone(),
                disabled: models.is_none(),
                "aria-label": "Progress model",
                onchange: move |value: String| {
                    let published = value
                        .parse::<usize>()
                        .ok()
                        .and_then(|place| choices.get(place))
                        .map(PublishedProgressModel::from);
                    scored.score_with(published);
                },
                option { value: RELEASED, selected: chosen == RELEASED, "Released model" }
                for (place, model) in listed.iter().enumerate() {
                    option { key: "{place}", value: "{place}", selected: chosen == place.to_string(),
                        "{model.name()}" }
                }
                if chosen == UNLISTED {
                    option { value: UNLISTED, disabled: true, selected: true,
                        if models.is_some() { "Not published" } else { "Published model" }
                    }
                }
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
    pub(super) fn label(rate: f64) -> String {
        format!("{rate:.2}×")
    }
    /// One episode's progress rate as a row of its timeline: the stretches
    /// below `cutoff` marked as left out, and the frames the scorer could not
    /// read as unknown.
    pub(super) fn track(timeline: &MeasuredTimeline, cutoff: Option<f64>) -> TimelineTrack {
        let moments = timeline.moments();
        let samples = moments
            .iter()
            .filter_map(|moment| {
                let middle = f64::midpoint(moment.start(), moment.end());
                Some(TimelineSample::new(middle, moment.value()?))
            })
            .collect();
        TimelineTrack::new(
            "curation-progress",
            "Progress rate",
            NumericTrack::new(samples, cutoff, MomentMark::intervals(timeline, cutoff))
                .read_as(Self::READING)
                .with_reference(1.0, "1×", "1× is the expert rate"),
        )
    }
}

/// How one moment reads against a cutoff, for the timeline to mark.
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
    fn intervals(timeline: &MeasuredTimeline, cutoff: Option<f64>) -> Vec<TimelineInterval> {
        let mut runs = Vec::<(Self, f64, f64)>::new();
        for moment in timeline.moments() {
            let Some(mark) = Self::of(moment.value(), cutoff) else {
                continue;
            };
            match runs.last_mut() {
                Some((last, _, end)) if *last == mark && *end == moment.start() => {
                    *end = moment.end();
                }
                _ => runs.push((mark, moment.start(), moment.end())),
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

/// One episode as its scores measured it: the recording with its progress
/// rate drawn on the timeline, the stretches below `cutoff` marked as left
/// out, and the frames the scorer could not read marked as unknown.
#[component]
pub(super) fn MomentEvidence(
    episode: Episode,
    timeline: MeasuredTimeline,
    /// What places the recording, under its name.
    facts: Vec<String>,
    /// The threshold a Select keeps moments by; `None` where nothing decides.
    cutoff: Option<f64>,
    /// The ask to play a stretch, counted, so choosing it again replays it.
    play: Option<PlayRequest>,
) -> Element {
    let track = ProgressRate::track(&timeline, cutoff);
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
    use crate::activities::curation::measured_timeline::TimelineEntry;

    #[test]
    fn touching_moments_marked_alike_are_one_interval() {
        let timeline = MeasuredTimeline::new(
            [Some(0.1), Some(0.2), None, Some(0.9), Some(0.1), Some(0.5)]
                .into_iter()
                .enumerate()
                .map(|(place, rate)| TimelineEntry::new(place as f64 * 0.5, rate))
                .collect(),
        );
        let marks = |cutoff| MomentMark::intervals(&timeline, cutoff).len();
        // Below 0.5: [0, 1) and [2, 2.5); unread: [1, 1.5).
        assert_eq!(marks(Some(0.5)), 3);
        // Nothing decides, so only the unread frame is marked.
        assert_eq!(marks(None), 1);
    }
}
