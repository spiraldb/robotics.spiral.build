//! Identify unproductive moments: score every episode that reaches the node
//! with the native progress model, and keep each scored frame as a moment
//! with the pace it made.
//!
//! This node measures and never decides. What counts as unproductive is the
//! threshold of a Select placed after it, which reads the same moments and
//! draws them with `MomentEvidence`, the one recording view both share. What
//! the node reads is its own step run over each episode's intervals that
//! reach it, held for the session.
mod moment;
mod scoring;

pub(super) use moment::Moment;
pub(super) use scoring::{MAX_MOMENTS, ProgressScorer};

use super::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use super::population::Population;
use super::poster::{PosterCut, PosterFrames, PosterStill};
use super::preview::{PreviewPlan, PreviewProgress};
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
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone};
use std::collections::BTreeMap;

/// The episodes reaching one detection node, and what its preview holds for
/// each: the moments it read, by episode, and the episodes it read or could
/// not.
#[derive(Clone, PartialEq)]
struct DetectionInput {
    episodes: Vec<Episode>,
    moments: BTreeMap<EpisodeId, Vec<Moment>>,
    progress: PreviewProgress,
}

impl DetectionInput {
    fn of(controller: CurationController, node: &str) -> Option<Self> {
        let sample = controller.sample()?;
        let (input, output) = controller.populations(node)?;
        let episodes = input
            .episodes()
            .iter()
            .filter_map(|id| sample.episode(id).cloned())
            .collect::<Vec<_>>();
        let mut moments = BTreeMap::<EpisodeId, Vec<Moment>>::new();
        if let Population::Moments(read) = output {
            for moment in read {
                moments
                    .entry(moment.episode().clone())
                    .or_default()
                    .push(moment);
            }
        }
        Some(Self {
            episodes,
            moments,
            progress: controller.progress(node)?,
        })
    }
    /// The episodes a Run scores: not read yet, or read from intervals that
    /// have changed since.
    fn pending(&self) -> Vec<Episode> {
        let pending = self.progress.pending();
        self.episodes
            .iter()
            .filter(|episode| pending.contains(episode.id()))
            .cloned()
            .collect()
    }
    fn failed(&self) -> Vec<Episode> {
        self.episodes
            .iter()
            .filter(|episode| {
                self.progress
                    .failed()
                    .iter()
                    .any(|(id, _)| id == episode.id())
            })
            .cloned()
            .collect()
    }
    fn moments_of(&self, episode: &EpisodeId) -> &[Moment] {
        self.moments.get(episode).map_or(&[], Vec::as_slice)
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
                .progress
                .failed()
                .iter()
                .find(|(id, _)| id == episode.id());
            if let Some((_, reason)) = failure {
                failed.push(ListedEpisode::Failed {
                    episode: episode.clone(),
                    reason: reason.clone(),
                });
            } else if self.moments.contains_key(episode.id()) {
                scored.push(ListedEpisode::Scored {
                    episode: episode.clone(),
                    pace: Pace::mean(self.moments_of(episode.id())),
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
    /// The episode's pace across its recording, drawn small in its row.
    fn trace(&self, episode: &Episode) -> TraceLine {
        let duration = episode
            .facts()
            .duration_seconds()
            .unwrap_or_default()
            .max(f64::EPSILON);
        let points = self
            .moments_of(episode.id())
            .iter()
            .map(|row| {
                (
                    f64::midpoint(row.start_time(), row.end_time()) / duration,
                    row.pace(),
                )
            })
            .collect();
        TraceLine::new(points, Pace::TRACE_DOMAIN, 1.0)
    }
}

/// The workspace shown for a selected Identify unproductive moments node.
#[component]
pub(super) fn DetectionWorkspace(controller: CurationController, node: String) -> Element {
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
    // The preview runs this node's step, the one a published view chains,
    // over the intervals that reach it, one episode bound at a time. The
    // pass is the curation's, not this view's: choosing another node leaves
    // it scoring. Stop scores no further episode; the ones in flight land.
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
    let uncompiled = controller
        .with_payload(|payload| PreviewPlan::check(payload, &node))
        .err()
        .map(|error| error.message().to_owned());
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
    let unscored = pending.len();
    let stale = input
        .progress
        .is_stale()
        .then(|| "Out of date: what reaches this node changed since it ran".to_owned());
    let blocked = unsupported.is_some() || uncompiled.is_some();
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
                note: NodeNotes::join([unsupported.clone(), uncompiled.clone(), problem(), stale, FailedEpisodes::note(failed.len())]),
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
                        // Scoring runs on this machine and costs nothing; what
                        // it reads is held until the page reloads.
                        if !pending.is_empty() {
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
                            let moments = evidence.moments_of(episode.id()).to_vec();
                            let mut facts = vec![
                                episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                            ];
                            facts.extend(Pace::mean(&moments).map(|pace| format!("mean pace {}", Pace::label(pace))));
                            rsx! { MomentEvidence { moments, episode, facts } }
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
    /// The mean over the moments the scorer could read.
    pub(super) fn mean(moments: &[Moment]) -> Option<f64> {
        let read = moments.iter().filter_map(Moment::pace).collect::<Vec<_>>();
        (!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64)
    }
    pub(super) fn label(pace: f64) -> String {
        format!("{pace:.2}×")
    }
}

/// How one moment reads against a Select's cutoff, for the timeline to mark.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MomentMark {
    /// Not read by the scorer, and so kept.
    Unread,
    /// Below the cutoff, and so left out.
    Below,
}

impl MomentMark {
    fn of(pace: Option<f64>, cutoff: Option<f64>) -> Option<Self> {
        match (pace, cutoff) {
            (None, _) => Some(Self::Unread),
            (Some(pace), Some(cutoff)) if pace < cutoff => Some(Self::Below),
            _ => None,
        }
    }
    /// The timeline's intervals: each run of touching moments marked alike
    /// as one interval, so an episode of thousands of frames draws a few.
    fn intervals(moments: &[Moment], cutoff: Option<f64>) -> Vec<TimelineInterval> {
        let mut runs = Vec::<(Self, f64, f64)>::new();
        for moment in moments {
            let Some(mark) = Self::of(moment.pace(), cutoff) else {
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
                    format!("Pace below {:.2}×", cutoff.unwrap_or_default()),
                ),
            })
            .collect()
    }
}

/// One episode as its moments measured it: the recording with its pace drawn
/// on the timeline, the stretches below `cutoff` marked as left out, and the
/// frames the scorer could not read marked as unknown.
#[component]
pub(super) fn MomentEvidence(
    episode: Episode,
    moments: Vec<Moment>,
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
    let samples = moments
        .iter()
        .filter_map(|row| {
            let middle = f64::midpoint(row.start_time(), row.end_time());
            Some(TimelineSample::new(middle, row.pace()?))
        })
        .collect();
    let intervals = MomentMark::intervals(&moments, cutoff);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_moments_marked_alike_are_one_interval() {
        let episode = EpisodeId::fixture(1);
        let moment = |start: f64, pace: Option<f64>| {
            Moment::new(episode.clone(), (start, start + 0.5), pace).unwrap()
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
