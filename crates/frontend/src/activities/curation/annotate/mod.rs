//! Break down subtasks: the node's step splits every episode that reaches it
//! into subtasks with `spql.ai.breakdown_task`, and its preview runs that step
//! over each episode's intervals that reach it, as the curation's own run.
//! What the preview read is held for the session, and every node below reads
//! it.
mod breakdown;
mod episodes;
mod output;
mod subtask;

#[cfg(test)]
pub(super) use subtask::{CategoryPlacement, JudgmentValue};
pub(super) use subtask::{MAX_SUBTASKS, SubtaskColumns, SubtaskRow};

use super::document::{CurationNode, CurationNodeKind, SubtaskAnnotation};
use super::episode_tabs::{EpisodeTabs, OpenEpisode};
use super::preview::{PreviewPlan, PreviewProgress};
use super::rerun::ConfirmedRerun;
use super::review::{
    EpisodeOrder, FailedEpisodes, NodeBar, NodeNotes, NodeReach, SubtaskContext, SubtaskReview,
};
use super::state::{CurationController, RunState};
use super::thumbnail::Thumbnails;
use crate::activities::episode::Episode;
use crate::components::icons::SettingsIcon;
use dioxus::prelude::*;
use episodes::{AnnotationEpisode, RecordedTopics};
use output::{
    AnnotatedEpisode, AnnotationOutput, ChosenSubtask, EpisodeFailure, EpisodeSubtasks,
    SubtaskIndex,
};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ChevronIcon, Field, FieldLayout, Place,
    Select, TextInput, TextInputKind,
};

/// Which episodes one run covers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RunScope {
    /// Everything the preview has not read yet, or read from rows that have
    /// changed since: the ordinary Run.
    Pending,
    /// Only the episodes whose last read failed.
    Failed,
    /// Every episode, replacing what the preview holds.
    All,
}

impl RunScope {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "Run",
            Self::Failed => "Retry failed",
            Self::All => "Run all again",
        }
    }
    /// What pressing it does, in the operator's own terms.
    fn explanation(self, episodes: usize) -> String {
        match self {
            Self::Pending => {
                format!("Break down the {episodes} episodes not yet read, or out of date")
            }
            Self::Failed => format!("Try the {episodes} episodes that failed again"),
            Self::All => format!("Break down all {episodes} episodes again"),
        }
    }
    fn purpose(self) -> ActionPurpose {
        match self {
            Self::Pending => ActionPurpose::Start,
            Self::Failed => ActionPurpose::Retry,
            Self::All => ActionPurpose::Replace,
        }
    }
    /// The episodes this run reads, in recording order.
    fn episodes(
        self,
        all: &[AnnotationEpisode],
        progress: &PreviewProgress,
    ) -> Vec<AnnotationEpisode> {
        let pending = progress.pending();
        all.iter()
            .filter(|episode| match self {
                Self::All => true,
                Self::Pending => pending.contains(episode.id()),
                Self::Failed => progress
                    .failed()
                    .iter()
                    .any(|(failed, _)| failed == episode.id()),
            })
            .cloned()
            .collect()
    }
}

/// One Break down subtasks node, read and edited through the controller.
#[derive(Clone, PartialEq)]
struct AnnotationNode {
    id: String,
    controller: CurationController,
}

impl AnnotationNode {
    fn new(id: String, controller: CurationController) -> Self {
        Self { id, controller }
    }
    /// The node's configuration, or nothing when the selected node is not an
    /// annotation after all.
    fn settings(&self) -> Option<SubtaskAnnotation> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).map(CurationNode::kind) {
                Some(CurationNodeKind::BreakDownSubtasks(annotation)) => Some(annotation.clone()),
                _ => None,
            }
        })
    }
    /// The rows the node hands on: its preview's, of the episodes reaching it.
    fn rows(&self) -> Vec<SubtaskRow> {
        self.controller
            .populations(&self.id)
            .map(|(_, output)| output.subtasks().to_vec())
            .unwrap_or_default()
    }
    /// Apply one configuration change, reporting what the document refused.
    fn configure(
        &self,
        change: impl FnOnce(&mut SubtaskAnnotation) -> Result<(), String>,
    ) -> Result<(), String> {
        let id = self.id.clone();
        self.controller.edit(
            |payload| match payload.node_mut(&id).map(CurationNode::kind_mut) {
                Some(CurationNodeKind::BreakDownSubtasks(annotation)) => change(annotation),
                _ => Err("That node does not annotate subtasks.".into()),
            },
        )
    }
}

/// The workspace shown for a selected Break down subtasks node.
#[component]
pub(super) fn AnnotationWorkspace(controller: CurationController, node: String) -> Element {
    let annotation = AnnotationNode::new(node.clone(), controller);
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    let listed = AnnotationEpisode::reaching(controller, &node);
    // Every episode of a dataset records the same topics, and the first one
    // is known before anything reaches the node.
    let topics = RecordedTopics::use_topics(
        controller
            .sample()
            .and_then(|sample| sample.episodes().first().map(Episode::selection)),
    );

    // What the dataset records decides two settings, written into the node so
    // its step can read them and a run can start at once: a new node starts
    // on the camera that shows the whole workspace, and every node reads the
    // operator's instruction when the dataset records one.
    let recorded = annotation.clone();
    use_effect(use_reactive(&topics, move |topics| {
        let (Some(topics), Some(settings)) = (topics, recorded.settings()) else {
            return;
        };
        let camera = settings
            .camera()
            .map(str::to_owned)
            .or_else(|| topics.overview_camera());
        let task_topic = topics.task_topic();
        if settings.camera().map(str::to_owned) == camera
            && settings.task_topic().map(str::to_owned) == task_topic
        {
            return;
        }
        let _ = recorded.configure(|annotation| {
            annotation.set_camera(camera);
            annotation.set_task_topic(task_topic);
            Ok(())
        });
    }));

    let chosen = ChosenSubtask::use_chosen();
    // The first episode the index lists, or the first episode at all, opens
    // by itself, so the recording column is never an empty half and what it
    // shows is the index's first heading.
    let first = annotation.clone();
    use_effect(move || {
        let rows = first.rows();
        let reaching = AnnotationEpisode::reaching(controller, &first.id);
        let opening = reaching
            .iter()
            .find(|episode| rows.iter().any(|row| row.episode() == episode.id()))
            .or_else(|| reaching.first())
            .map(|episode| episode.id().clone());
        chosen
            .tabs()
            .open_first(opening.map(|episode| (episode, None)));
    });
    let focus_node = node.clone();
    use_effect(move || {
        let Some(focus) = controller.focus() else {
            return;
        };
        if focus.node() != focus_node {
            return;
        }
        controller.take_focus();
        chosen.focus(focus.episode(), focus.interval());
    });

    // The preview runs this node's step, the one a published view chains,
    // over the intervals that reach it, one query per episode. The pass is
    // the curation's, not this view's: choosing another node leaves it
    // reading. Stop starts no further episode; the ones in flight land.
    let run = controller.run_state(&node);
    let problem = use_signal(|| Option::<String>::None);
    let start = {
        let node = node.clone();
        move |chosen: Vec<AnnotationEpisode>| {
            let episodes = chosen
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

    // Every hook above runs for any selected node, so a graph edit that
    // replaces this node cannot leave the workspace with a shorter hook list.
    let Some(settings) = annotation.settings() else {
        return rsx! {
            section { class: "curation-annotate-workspace", role: "alert",
                "This node does not break down subtasks." }
        };
    };
    let uncompiled = controller
        .with_payload(|payload| PreviewPlan::check(payload, &node))
        .err()
        .map(|error| error.message().to_owned());
    let progress = controller.progress(&node).unwrap_or_default();
    let failures = progress
        .failed()
        .iter()
        .map(|(episode, reason)| EpisodeFailure::new(episode.clone(), reason))
        .collect::<Vec<_>>();
    let stale = progress
        .is_stale()
        .then(|| "Out of date: what reaches this node changed since it ran".to_owned());
    let rows = annotation.rows();
    let groups = EpisodeSubtasks::group(rows.clone().into_iter());
    let context = SubtaskContext::new(
        dataset.clone(),
        settings.camera().map(str::to_owned),
        EpisodeOrder::of(listed.iter().map(AnnotationEpisode::id)),
    );
    let thumbnails = Thumbnails::use_camera(controller, context.camera());
    let evidence = (listed.clone(), groups.clone(), context.clone());
    let done = progress.read().len();
    let replaceable = RunScope::All.episodes(&listed, &progress).len();
    let retry = {
        let listed = listed.clone();
        let start = start.clone();
        move |episode: EpisodeId| {
            start(
                listed
                    .iter()
                    .filter(|candidate| candidate.id() == &episode)
                    .cloned()
                    .collect(),
            );
        }
    };
    rsx! {
        section { class: "curation-annotate-workspace", "aria-label": "Break down subtasks",
            NodeBar {
                // What the node holds is on its card and a run's progress on
                // the card and in the top bar; the bar says what the pass runs
                // with, which is also the way into changing it.
                lead: rsx! {
                    AnnotationSettings {
                        annotation: annotation.clone(),
                        settings: settings.clone(),
                        cameras: topics.as_ref().map(RecordedTopics::cameras).unwrap_or_default(),
                        disabled: running,
                        reannotate: {
                            let start = start.clone();
                            let episodes = RunScope::All.episodes(&listed, &progress);
                            rsx! {
                                if replaceable > 0 && !running && uncompiled.is_none() {
                                    ConfirmedRerun {
                                        id: "curation-annotate-replace".to_owned(),
                                        label: RunScope::All.label().to_owned(),
                                        question: format!(
                                            "Break down all {replaceable} episodes again? This replaces the {} subtasks the preview holds, and costs another full run.",
                                            rows.len(),
                                        ),
                                        on_confirm: move |()| start(episodes.clone()),
                                    }
                                }
                            }
                        },
                    }
                },
                note: NodeNotes::join([uncompiled.clone(), problem(), stale, FailedEpisodes::note(failures.len())]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! {
                    if running {
                        Button {
                            purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                            disabled: run == RunState::Stopping,
                            title: "Start no further episode; the one in flight still lands",
                            onclick: {
                                let node = node.clone();
                                move |_| controller.stop_run(&node)
                            },
                            if run == RunState::Stopping { "Stopping…" } else { "Stop" }
                        }
                    } else {
                        for scope in [RunScope::Failed, RunScope::Pending] {
                            {
                                let episodes = scope.episodes(&listed, &progress);
                                let count = episodes.len();
                                let start = start.clone();
                                rsx! {
                                    if count > 0 {
                                        Button {
                                            key: "{scope.label()}",
                                            purpose: scope.purpose(),
                                            variant: if scope == RunScope::Pending { ButtonVariant::Solid } else { ButtonVariant::Outline },
                                            disabled: uncompiled.is_some(),
                                            title: scope.explanation(count),
                                            onclick: move |_| start(episodes.clone()),
                                            if scope == RunScope::Pending && done > 0 {
                                                "{scope.label()} {count} left"
                                            } else {
                                                "{scope.label()}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
            if let Some(error) = controller.sample_error() {
                div { class: "curation-annotate-failure", role: "alert",
                    p { "{error}" }
                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                        onclick: move |_| controller.reload_sample(), "Retry the episode list" }
                }
            }
            SubtaskReview {
                index: rsx! {
                    SubtaskIndex {
                        output: AnnotationOutput::new(groups.clone(), failures.clone(), context.clone()),
                        thumbnails,
                        chosen,
                        on_retry: retry,
                        on_find_similar: {
                            let annotation = node.clone();
                            move |(row, from): (SubtaskRow, Option<Place>)| {
                                controller.find_similar(&annotation, &row, from);
                            }
                        },
                    }
                },
                evidence: rsx! {
                    EpisodeTabs {
                        open: chosen.tabs(),
                        empty: "Choose a subtask to watch it play.".to_owned(),
                        render: move |tab: OpenEpisode| {
                            let (listed, groups, context) = &evidence;
                            let Some(episode) = listed.iter().find(|episode| episode.id() == tab.episode()).cloned() else {
                                return rsx! {};
                            };
                            let subtasks = groups
                                .iter()
                                .find(|group| group.episode() == tab.episode())
                                .cloned()
                                .unwrap_or_else(|| EpisodeSubtasks::none(tab.episode()));
                            let annotation = node.clone();
                            rsx! {
                                AnnotatedEpisode {
                                    episode,
                                    subtasks,
                                    context: context.clone(),
                                    tab,
                                    on_find_similar: move |row: SubtaskRow| controller.find_similar(&annotation, &row, None),
                                }
                            }
                        },
                    }
                },
            }
        }
    }
}

/// The settings dialog answers Escape and the backdrop, so it needs a name
/// focus can reach it by.
const SETTINGS_DIALOG_ID: &str = "curation-annotate-settings";

/// The camera, model, and sampling one pass runs with, said on the node's
/// bar as the button that changes them, plus the run that replaces what they
/// already produced.
#[component]
fn AnnotationSettings(
    annotation: AnnotationNode,
    settings: SubtaskAnnotation,
    cameras: Vec<String>,
    disabled: bool,
    reannotate: Element,
) -> Element {
    let mut open = use_signal(|| false);
    // Focus has to be inside the dialog for its own Escape handler to hear
    // the key, the way the document switcher's is.
    use_effect(move || {
        if open() {
            BrowserFocus::after_render(SETTINGS_DIALOG_ID.to_owned());
        }
    });
    let camera = settings.camera().unwrap_or_default().to_owned();
    let known = cameras.contains(&camera);
    let pick = annotation.clone();
    let name = annotation.clone();
    let interval = annotation.clone();
    rsx! {
        span { class: "curation-annotate-settings-slot",
            Button {
                purpose: ActionPurpose::Navigate,
                variant: ButtonVariant::Quiet,
                class: "curation-node-bar-settings",
                aria_haspopup: "dialog",
                aria_expanded: open(),
                title: "Change the camera, model, and sampling",
                onclick: move |_| open.toggle(),
                SettingsIcon {}
                if camera.is_empty() {
                    span { class: "curation-node-bar-missing", "Choose a camera" }
                } else {
                    code { "{camera}" }
                    code { "{settings.model()}" }
                    code { "every {settings.sample_interval_seconds()} s" }
                    if let Some(topic) = settings.task_topic() {
                        code { title: "Each episode's task is the instruction it records here", "task from {topic}" }
                    }
                }
                ChevronIcon { open: open() }
            }
            if open() {
                div { class: "popover-backdrop", onclick: move |_| open.set(false) }
                div {
                    id: SETTINGS_DIALOG_ID,
                    class: "curation-annotate-settings",
                    role: "dialog",
                    tabindex: "-1",
                    "aria-label": "Annotation settings",
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Escape {
                            event.prevent_default();
                            open.set(false);
                        }
                    },
                    Field { label: "Camera".to_owned(), layout: FieldLayout::Stacked,
                        Select {
                            value: camera.clone(), disabled: disabled || cameras.is_empty(),
                            onchange: move |value: String| {
                                let _ = pick.configure(|annotation| {
                                    annotation.set_camera(Some(value.clone()));
                                    Ok(())
                                });
                            },
                            option { value: "", disabled: true, selected: camera.is_empty(), "Choose a camera" }
                            if !camera.is_empty() && !known {
                                option { value: camera.clone(), selected: true, "{camera} (not in this dataset)" }
                            }
                            for topic in cameras.iter() {
                                option { key: "{topic}", value: "{topic}", selected: *topic == camera, "{topic}" }
                            }
                        }
                    }
                    Field { label: "Model".to_owned(), layout: FieldLayout::Stacked,
                        TextInput {
                            value: settings.model().to_owned(), disabled,
                            placeholder: "provider/model",
                            oninput: move |value: String| {
                                let _ = name.configure(|annotation| annotation.set_model(value.clone()));
                            },
                        }
                    }
                    Field { label: "Sample every (s)".to_owned(), layout: FieldLayout::Stacked,
                        TextInput {
                            value: format!("{}", settings.sample_interval_seconds()),
                            kind: TextInputKind::Number, disabled,
                            min: "0.1", step: "0.1",
                            oninput: move |value: String| {
                                if let Ok(seconds) = value.trim().parse::<f64>() {
                                    let _ = interval.configure(|annotation| {
                                        annotation.set_sample_interval_seconds(seconds)
                                    });
                                }
                            },
                        }
                    }
                    span { class: "curation-annotate-replace-slot", {reannotate} }
                }
            }
        }
    }
}
