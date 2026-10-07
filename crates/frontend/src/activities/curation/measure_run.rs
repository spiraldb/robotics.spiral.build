//! Running a measurement that asks a Modal service about each episode: which
//! of the episodes reaching it a Run asks about, which failed and why, and
//! the Stop, Retry failed and Run that drive its preview.
//!
//! The pass is the curation's, not the workspace's: choosing another node
//! leaves it running. Stop ends it at once; the episodes in flight stay
//! unread. What the service answered is stored, so no later Run or Publish
//! asks about an episode again.
use super::preview::PreviewProgress;
use super::review::{Counted, FailedEpisodes};
use super::state::{CurationController, RunState};
use crate::activities::episode::Episode;
use crate::components::icons::SpinnerIcon;
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

/// What a measurement does to an episode, as its controls say it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MeasureVerb {
    Score,
    Track,
}

impl MeasureVerb {
    const fn base(self) -> &'static str {
        match self {
            Self::Score => "Score",
            Self::Track => "Track",
        }
    }
    const fn gerund(self) -> &'static str {
        match self {
            Self::Score => "scoring",
            Self::Track => "tracking",
        }
    }
    const fn participle(self) -> &'static str {
        match self {
            Self::Score => "scored",
            Self::Track => "tracked",
        }
    }
}

/// The episodes reaching one measuring node, and where its preview stands
/// over them.
#[derive(Clone, PartialEq)]
pub(super) struct MeasureInput {
    episodes: Vec<Episode>,
    preview: PreviewProgress,
}

impl MeasureInput {
    pub(super) fn of(controller: CurationController, node: &str) -> Option<Self> {
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
    pub(super) fn episodes(&self) -> &[Episode] {
        &self.episodes
    }
    pub(super) fn preview(&self) -> &PreviewProgress {
        &self.preview
    }
    /// The episodes a Run asks about: not read yet, or read from intervals
    /// that have changed since.
    fn pending(&self) -> Vec<Episode> {
        let pending = self.preview.pending();
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
                self.preview
                    .failed()
                    .iter()
                    .any(|(id, _)| id == episode.id())
            })
            .cloned()
            .collect()
    }
    /// How many episodes failed, and why when one reason stands for every
    /// one of them, such as the Modal service not being set up.
    pub(super) fn failures(&self) -> Option<String> {
        let note = FailedEpisodes::note(self.failed().len())?;
        let mut reasons = self
            .preview
            .failed()
            .iter()
            .map(|(_, reason)| reason.as_str());
        let first = reasons.next();
        Some(match first {
            Some(first) if reasons.all(|reason| reason == first) => format!("{note}: {first}"),
            _ => note,
        })
    }
    /// What the node's bar says when what reaches it changed since it ran.
    pub(super) fn stale(&self) -> Option<String> {
        self.preview
            .is_stale()
            .then(|| "Out of date: its input changed".to_owned())
    }
}

/// One measuring node's run: the node, what it does to an episode, and
/// where a refused start is said.
#[derive(Clone, PartialEq)]
pub(super) struct MeasureRun {
    controller: CurationController,
    node: String,
    verb: MeasureVerb,
    problem: Signal<Option<String>>,
}

impl MeasureRun {
    pub(super) fn use_run(controller: CurationController, node: &str, verb: MeasureVerb) -> Self {
        Self {
            controller,
            node: node.to_owned(),
            verb,
            problem: use_signal(|| None),
        }
    }
    /// Why the last Run could not start, when it could not.
    pub(super) fn problem(&self) -> Option<String> {
        (self.problem)()
    }
    /// What the node's card and primer say while it runs.
    pub(super) fn running_line(&self) -> String {
        self.controller
            .activity()
            .get(&self.node)
            .cloned()
            .unwrap_or_else(|| format!("{}…", Self::capitalized(self.verb.gerund())))
    }
    fn state(&self) -> RunState {
        self.controller.run_state(&self.node)
    }
    fn start(&self, episodes: &[Episode]) {
        let episodes = episodes
            .iter()
            .map(|episode| episode.id().clone())
            .collect::<Vec<_>>();
        if !episodes.is_empty() {
            let mut problem = self.problem;
            problem.set(self.controller.run_preview(&self.node, &episodes).err());
        }
    }
    fn stop(&self) {
        self.controller.stop_run(&self.node);
    }
    fn capitalized(word: &str) -> String {
        let mut letters = word.chars();
        letters.next().map_or_else(String::new, |first| {
            first.to_uppercase().chain(letters).collect()
        })
    }
}

/// Stop while the node runs; otherwise Retry failed and Run, for the
/// node's bar. Before the first read the primer holds the run instead.
#[component]
pub(super) fn MeasureRunActions(run: MeasureRun, input: MeasureInput, blocked: bool) -> Element {
    let state = run.state();
    let pending = input.pending();
    let failed = input.failed();
    let verb = run.verb;
    rsx! {
        if state != RunState::Idle {
            Button {
                purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                disabled: state == RunState::Stopping,
                title: "Stop {verb.gerund()}; the episodes in flight stay un{verb.participle()}",
                onclick: {
                    let run = run.clone();
                    move |_| run.stop()
                },
                if state == RunState::Stopping { "Stopping…" } else { "Stop" }
            }
        } else {
            if !failed.is_empty() {
                Button {
                    purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                    disabled: blocked,
                    title: "{verb.base()} the {failed.len()} episodes that failed again",
                    onclick: {
                        let (run, failed) = (run.clone(), failed.clone());
                        move |_| run.start(&failed)
                    },
                    "Retry failed"
                }
            }
            if !pending.is_empty() && !input.preview.is_unmeasured() {
                Button {
                    purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                    disabled: blocked,
                    title: "{verb.base()} the {pending.len()} episodes not yet {verb.participle()}",
                    onclick: {
                        let (run, pending) = (run.clone(), pending.clone());
                        move |_| run.start(&pending)
                    },
                    if pending.len() < input.episodes.len() { "Run {pending.len()} left" } else { "Run" }
                }
            }
        }
    }
}

/// The primer's action before the node's first read: what the run is doing
/// while it runs, else the run that asks about every episode.
#[component]
pub(super) fn MeasureRunStart(run: MeasureRun, input: MeasureInput, blocked: bool) -> Element {
    let pending = input.pending();
    let verb = run.verb;
    rsx! {
        if run.state() != RunState::Idle {
            p { class: "curation-primer-status", role: "status",
                SpinnerIcon {}
                "{run.running_line()}"
            }
        } else if !pending.is_empty() {
            Button {
                purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                disabled: blocked,
                title: "{verb.base()} the {pending.len()} episodes not yet {verb.participle()}",
                onclick: {
                    let (run, pending) = (run.clone(), pending.clone());
                    move |_| run.start(&pending)
                },
                "{verb.base()} {Counted::episodes(pending.len())}"
            }
        }
    }
}
