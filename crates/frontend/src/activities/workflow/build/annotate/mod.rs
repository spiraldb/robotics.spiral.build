//! Break down subtasks: browser-owned annotation of every episode in the
//! dataset, one model call per episode, rows persisted through the
//! controller after each episode. Owned by the Annotation stream.
mod episodes;
mod journal;
mod native;
mod output;
mod pipeline;
mod provider;
mod request;
mod sheet;

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use super::document::{BuildNode, BuildNodeKind, SubtaskAnnotation, SubtaskRow};
use super::poster::{PosterCut, PosterFrames};
use super::rerun::ConfirmedRerun;
use super::review::{EpisodeNames, NoSubtaskChosen, SubtaskContext, SubtaskReview};
use super::state::BuildController;
use crate::activities::data::DatasetClient;
use crate::client::model::ModelClient;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use episodes::AnnotationEpisode;
use journal::BuildJournal;
use native::NativeSheets;
use output::{AnnotatedEpisode, ChosenSubtask, EpisodeSubtasks, SubtaskIndex};
use pipeline::{AnnotationPass, EPISODES_IN_FLIGHT, PassSettings};
use provider::ProviderAnnotator;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Chip, ChipTone, Disclosure, Field, FieldLayout, Select,
    TextInput, TextInputKind,
};

/// Which episodes one run covers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RunScope {
    /// Everything not yet recorded as completed: the ordinary Run.
    Pending,
    /// Only the episodes whose last attempt failed.
    Failed,
    /// Every episode, replacing the rows already stored.
    All,
}

impl RunScope {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "Run",
            Self::Failed => "Retry failed",
            Self::All => "Re-annotate all",
        }
    }
    /// What pressing it does, in the operator's own terms.
    fn explanation(self, episodes: usize) -> String {
        match self {
            Self::Pending => format!("Annotate the {episodes} episodes not yet done"),
            Self::Failed => format!("Try the {episodes} episodes that failed again"),
            Self::All => {
                format!("Annotate all {episodes} episodes again, replacing what is stored")
            }
        }
    }
    fn purpose(self) -> ActionPurpose {
        match self {
            Self::Pending => ActionPurpose::Start,
            Self::Failed => ActionPurpose::Retry,
            Self::All => ActionPurpose::Replace,
        }
    }
    /// The episodes this run annotates, in recording order.
    fn episodes(
        self,
        all: &[AnnotationEpisode],
        settings: &SubtaskAnnotation,
    ) -> Vec<AnnotationEpisode> {
        all.iter()
            .filter(|episode| match self {
                Self::All => true,
                Self::Pending => !settings.is_completed(episode.id()),
                Self::Failed => settings
                    .failed_episodes()
                    .iter()
                    .any(|failure| failure.episode() == episode.id()),
            })
            .cloned()
            .collect()
    }
}

/// One Break down subtasks node, read and edited through the controller.
#[derive(Clone, PartialEq)]
struct AnnotationNode {
    id: String,
    controller: BuildController,
}

impl AnnotationNode {
    fn new(id: String, controller: BuildController) -> Self {
        Self { id, controller }
    }
    /// The node's configuration and progress, or nothing when the selected
    /// node is not an annotation after all.
    fn settings(&self) -> Option<SubtaskAnnotation> {
        self.controller.with_payload(
            |payload| match payload.node(&self.id).map(BuildNode::kind) {
                Some(BuildNodeKind::BreakDownSubtasks(annotation)) => Some(annotation.clone()),
                _ => None,
            },
        )
    }
    fn rows(&self) -> Vec<SubtaskRow> {
        self.controller
            .with_payload(|payload| payload.subtasks_for(&self.id).cloned().collect())
    }
    /// Apply one configuration change, reporting what the document refused.
    fn configure(
        &self,
        change: impl FnOnce(&mut SubtaskAnnotation) -> Result<(), String>,
    ) -> Result<(), String> {
        let id = self.id.clone();
        self.controller.edit(
            |payload| match payload.node_mut(&id).map(BuildNode::kind_mut) {
                Some(BuildNodeKind::BreakDownSubtasks(annotation)) => change(annotation),
                _ => Err("That node does not annotate subtasks.".into()),
            },
        )
    }
}

/// The workspace shown for a selected Break down subtasks node.
#[component]
pub(super) fn AnnotationWorkspace(controller: BuildController, node: String) -> Element {
    let studio = StudioProjectState::current();
    let annotation = AnnotationNode::new(node.clone(), controller);
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    let mut episodes = use_resource(use_reactive(&dataset, move |dataset| async move {
        match dataset {
            Some(dataset) => AnnotationEpisode::load(&dataset).await,
            None => Err("Choose a dataset for this Build graph first.".to_owned()),
        }
    }));
    let listed = episodes
        .read()
        .as_ref()
        .and_then(|loaded| loaded.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let cameras = AnnotationEpisode::use_cameras(listed.first().cloned());

    // A new node starts on the camera that shows the whole workspace, written
    // into the node so the picker shows the choice and a run can start at once.
    let default_camera = annotation.clone();
    use_effect(use_reactive(&cameras, move |cameras| {
        let Some(settings) = default_camera.settings() else {
            return;
        };
        if settings.camera().is_some() {
            return;
        }
        let Some(camera) = episodes::preferred_camera(&cameras).map(str::to_owned) else {
            return;
        };
        let _ = default_camera.configure(|annotation| {
            annotation.set_camera(Some(camera));
            Ok(())
        });
    }));

    let chosen = ChosenSubtask::use_chosen();
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

    let model = ModelClient::current();
    // Both clients belong to this mount, and a pass outlives the click that
    // started it, so they are captured here rather than read inside its task.
    let datasets = DatasetClient::current();
    // `running` is what the pass is actually doing; `wanted` is what the
    // operator asked for. Pressing Stop clears `wanted`, which stops the next
    // episode from starting, but the episodes already paid for finish and
    // save. Collapsing the two would let the interface report idle while
    // eight model calls were still billing, and would let a second pass start
    // underneath the first.
    let mut running = use_signal(|| false);
    let mut wanted = use_signal(|| false);
    let start = {
        let annotation = annotation.clone();
        let episodes = listed.clone();
        move |scope: RunScope| {
            if *running.peek() {
                return;
            }
            wanted.set(true);
            let Some(settings) = annotation.settings() else {
                return;
            };
            let Some(camera) = settings.camera().map(str::to_owned) else {
                return;
            };
            let chosen = scope.episodes(&episodes, &settings);
            if chosen.is_empty() {
                return;
            }
            let interval = settings.sample_interval_seconds();
            let node = annotation.id.clone();
            let model_name = settings.model().to_owned();
            let model = model.clone();
            let datasets = datasets.clone();
            running.set(true);
            spawn(async move {
                // The dataset's own words for the job, when it records them,
                // are worth more to the model than any of our phrasing.
                let task = match chosen.first() {
                    Some(episode) => episode.task(&datasets).await,
                    None => None,
                };
                AnnotationPass::new(
                    PassSettings::new(node.clone(), model_name, interval).with_task(task),
                    NativeSheets::new(datasets, camera, interval),
                    ProviderAnnotator::new(model),
                    BuildJournal::new(node.clone(), controller, studio, wanted),
                )
                .run(chosen)
                .await;
                running.set(false);
                wanted.set(false);
                controller.report_activity(&node, None);
            });
        }
    };

    // Every hook above runs for any selected node, so a graph edit that
    // replaces this node cannot leave the workspace with a shorter hook list.
    let Some(settings) = annotation.settings() else {
        return rsx! {
            section { class: "build-annotate-workspace", role: "alert",
                "This node does not break down subtasks." }
        };
    };
    let rows = annotation.rows();
    let groups = EpisodeSubtasks::group(rows.clone().into_iter());
    let context = SubtaskContext::new(
        dataset.clone(),
        settings.camera().map(str::to_owned),
        EpisodeNames::of(listed.iter().map(AnnotationEpisode::id)),
    );
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
    let shown = chosen
        .episode()
        .or_else(|| groups.first().map(|group| group.episode().to_owned()))
        .and_then(|id| {
            let episode = listed.iter().find(|episode| episode.id() == id)?.clone();
            let subtasks = groups.iter().find(|group| group.episode() == id).cloned();
            Some((episode, subtasks))
        });
    let done = settings.completed_episodes().len();
    let failed = settings.failed_episodes().len();
    // Every node's progress travels to the canvas, so a graph the operator has
    // scrolled away from still shows which measurement is working.
    let reported = node.clone();
    use_effect(use_reactive(
        (&running(), &done, &listed.len()),
        move |(running, done, all)| {
            controller.report_activity(
                &reported,
                running.then(|| format!("{done} of {all} episodes")),
            );
        },
    ));
    rsx! {
        section { class: "build-annotate-workspace", "aria-label": "Break down subtasks",
            AnnotationRun {
                settings: settings.clone(),
                episodes: listed.len(),
                loading: episodes.read().is_none(),
                running: running(),
                stopping: running() && !wanted(),
                actions: rsx! {
                    if running() {
                        Button {
                            purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                            disabled: !wanted(),
                            title: "Start no further episode; the ones already in flight finish and are saved",
                            onclick: move |_| wanted.set(false),
                            if wanted() { "Stop" } else { "Stopping…" }
                        }
                    } else {
                        for scope in [RunScope::Pending, RunScope::Failed] {
                            {
                                let count = scope.episodes(&listed, &settings).len();
                                rsx! {
                                    Button {
                                        key: "{scope.label()}",
                                        purpose: scope.purpose(),
                                        variant: if scope == RunScope::Pending { ButtonVariant::Solid } else { ButtonVariant::Outline },
                                        disabled: settings.camera().is_none() || count == 0,
                                        title: scope.explanation(count),
                                        onclick: { let mut start = start.clone(); move |_| start(scope) },
                                        if scope == RunScope::Pending && count > 0 && done > 0 {
                                            "{scope.label()} {count} left"
                                        } else {
                                            "{scope.label()}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
            AnnotationTools {
                annotation: annotation.clone(),
                settings: settings.clone(),
                cameras,
                disabled: running(),
                measured: done > 0,
                reannotate: {
                    let count = RunScope::All.episodes(&listed, &settings).len();
                    let mut start = start.clone();
                    rsx! {
                        if count > 0 && !running() {
                            ConfirmedRerun {
                                id: "build-annotate-replace".to_owned(),
                                label: RunScope::All.label().to_owned(),
                                question: format!(
                                    "Annotate all {count} episodes again? This replaces the {} subtasks already stored, and costs another full run.",
                                    rows.len(),
                                ),
                                on_confirm: move |()| start(RunScope::All),
                            }
                        }
                    }
                },
            }
            match episodes.read().as_ref() {
                Some(Err(error)) => rsx! {
                    div { class: "build-annotate-failure", role: "alert",
                        p { "{error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                            onclick: move |_| episodes.restart(), "Retry the episode list" }
                    }
                },
                _ => rsx! {},
            }
            if failed > 0 {
                AnnotationFailures { settings: settings.clone(), context: context.clone(), open: !running() }
            }
            SubtaskReview {
                index: rsx! {
                    SubtaskIndex { groups: groups.clone(), context: context.clone(), frames, chosen }
                },
                evidence: match shown {
                    Some((episode, subtasks)) => rsx! {
                        AnnotatedEpisode {
                            key: "{episode.id()}",
                            episode: episode.clone(),
                            subtasks: subtasks.unwrap_or_else(|| EpisodeSubtasks::none(episode.id())),
                            context: context.clone(),
                            chosen,
                        }
                    },
                    None => rsx! {
                        NoSubtaskChosen {
                            prompt: "Run the annotation, then choose a subtask to watch it play.",
                        }
                    },
                },
            }
        }
    }
}

/// What the run has reached, in the strip Workflow and Curation already use
/// for the same job: one phase, fixed facts, and the action that changes them.
#[component]
fn AnnotationRun(
    settings: SubtaskAnnotation,
    episodes: usize,
    loading: bool,
    running: bool,
    stopping: bool,
    actions: Element,
) -> Element {
    let done = settings.completed_episodes().len();
    let failed = settings.failed_episodes().len();
    let (phase, state) = match (loading, running, stopping, done) {
        (true, ..) => ("is-validating", "Reading episodes"),
        (_, true, true, _) => ("is-running", "Stopping"),
        (_, true, false, _) => ("is-running", "Annotating"),
        (_, _, _, 0) => ("is-idle", "Ready"),
        _ if done >= episodes && episodes > 0 => ("is-complete", "Complete"),
        _ => ("is-idle", "Part way"),
    };
    rsx! {
        div { class: "run-strip build-annotate-run",
            span { class: "run-phase {phase}", role: "status",
                span { class: "run-phase-dot", "aria-hidden": "true" }
                "{state}"
            }
            span { class: "build-annotate-count", role: "status",
                "{done} of {episodes} episodes · {Remaining(done, episodes)}"
            }
            if running {
                span { class: "build-annotate-inflight",
                    "up to {EPISODES_IN_FLIGHT} at once" }
            }
            if failed > 0 {
                Chip { tone: ChipTone::Danger, "{failed} failed" }
            }
            span { class: "build-annotate-run-actions", {actions} }
        }
    }
}

/// What is left to do, said once beside the count rather than implied by it.
#[allow(non_snake_case)]
fn Remaining(done: usize, all: usize) -> String {
    if done == 0 {
        "nothing measured yet".to_owned()
    } else if done >= all {
        "every episode measured".to_owned()
    } else {
        format!("{} to go", all - done)
    }
}

/// The episodes a run could not annotate, and why.
#[component]
fn AnnotationFailures(settings: SubtaskAnnotation, context: SubtaskContext, open: bool) -> Element {
    let failed = settings.failed_episodes();
    rsx! {
        Disclosure { class: "build-annotate-failures", open,
            summary: rsx! { "{failed.len()} episodes could not be annotated" },
            ul {
                for failure in failed {
                    li { key: "{failure.episode()}",
                        strong { title: "{failure.episode()}", "{context.name(failure.episode())}" }
                        span { "{failure.reason()}" }
                    }
                }
            }
        }
    }
}

/// The camera, model, and sampling one pass runs with, plus the run that
/// replaces what they already produced.
///
/// Before the first run these are the whole job, so they are open. After it
/// they are settings for a run that has happened, so they fold away behind
/// one line that says what was used.
#[component]
fn AnnotationTools(
    annotation: AnnotationNode,
    settings: SubtaskAnnotation,
    cameras: Vec<String>,
    disabled: bool,
    measured: bool,
    reannotate: Element,
) -> Element {
    let camera = settings.camera().unwrap_or_default().to_owned();
    let known = cameras.contains(&camera);
    let pick = annotation.clone();
    let name = annotation.clone();
    let pace = annotation.clone();
    let used = format!(
        "{} · {} · every {}s",
        settings.camera().unwrap_or("no camera"),
        settings.model(),
        settings.sample_interval_seconds(),
    );
    let fields = rsx! {
        div { class: "build-annotate-settings",
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
                    kind: TextInputKind::Decimal, disabled,
                    oninput: move |value: String| {
                        if let Ok(seconds) = value.trim().parse::<f64>() {
                            let _ = pace.configure(|annotation| {
                                annotation.set_sample_interval_seconds(seconds)
                            });
                        }
                    },
                }
            }
            span { class: "build-annotate-replace-slot", {reannotate} }
        }
    };
    if !measured {
        return fields;
    }
    rsx! {
        Disclosure { class: "build-annotate-tools",
            summary: rsx! { "Settings · " span { class: "build-annotate-used", "{used}" } },
            {fields}
        }
    }
}
