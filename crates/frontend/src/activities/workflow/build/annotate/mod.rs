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
use super::state::BuildController;
use crate::activities::data::DatasetClient;
use crate::client::model::ModelClient;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use episodes::AnnotationEpisode;
use journal::BuildJournal;
use native::NativeSheets;
use output::{AnnotatedEpisode, EpisodeSubtasks, SubtaskTable};
use pipeline::{AnnotationPass, PassSettings};
use provider::ProviderAnnotator;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Field, FieldLayout, Select, TextInput, TextInputKind,
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

    let mut selected = use_signal(|| None::<String>);
    let mut play = use_signal(|| None::<(f64, f64)>);
    let focus_node = node.clone();
    use_effect(move || {
        let Some(focus) = controller.focus() else {
            return;
        };
        if focus.node() != focus_node {
            return;
        }
        controller.take_focus();
        selected.set(Some(focus.episode().to_owned()));
        play.set(focus.interval());
    });

    let model = ModelClient::current();
    // Both clients belong to this mount, and a pass outlives the click that
    // started it, so they are captured here rather than read inside its task.
    let datasets = DatasetClient::current();
    let mut running = use_signal(|| false);
    let start = {
        let annotation = annotation.clone();
        let episodes = listed.clone();
        move |scope: RunScope| {
            if *running.peek() {
                return;
            }
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
                    BuildJournal::new(node, controller, studio, running),
                )
                .run(chosen)
                .await;
                running.set(false);
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
    let groups = EpisodeSubtasks::group(annotation.rows().into_iter());
    let shown = selected()
        .or_else(|| groups.first().map(|group| group.episode().to_owned()))
        .and_then(|id| {
            let episode = listed.iter().find(|episode| episode.id() == id)?.clone();
            let subtasks = groups.iter().find(|group| group.episode() == id).cloned();
            Some((episode, subtasks))
        });
    rsx! {
        section { class: "build-annotate-workspace", "aria-label": "Break down subtasks",
            AnnotationSettings { annotation: annotation.clone(), settings: settings.clone(), cameras, disabled: running() }
            div { class: "build-annotate-actions", role: "group", "aria-label": "Annotation run",
                for scope in [RunScope::Pending, RunScope::Failed, RunScope::All] {
                    Button {
                        key: "{scope.label()}",
                        purpose: scope.purpose(),
                        variant: if scope == RunScope::Pending { ButtonVariant::Solid } else { ButtonVariant::Outline },
                        disabled: running() || settings.camera().is_none()
                            || scope.episodes(&listed, &settings).is_empty(),
                        onclick: { let mut start = start.clone(); move |_| start(scope) },
                        "{scope.label()}"
                    }
                }
                Button {
                    purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline, disabled: !running(),
                    title: "Finish the episode in flight, save it, and stop",
                    onclick: move |_| running.set(false),
                    "Stop"
                }
            }
            AnnotationProgress {
                settings: settings.clone(), episodes: listed.len(),
                loading: episodes.read().is_none(), running: running(),
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
            SubtaskTable { groups: groups.clone(), selected, play }
            if let Some((episode, subtasks)) = shown {
                AnnotatedEpisode {
                    key: "{episode.id()}",
                    episode: episode.clone(),
                    subtasks: subtasks.unwrap_or_else(|| EpisodeSubtasks::none(episode.id())),
                    play,
                }
            }
        }
    }
}

/// The camera, model, and sampling one pass runs with.
#[component]
fn AnnotationSettings(
    annotation: AnnotationNode,
    settings: SubtaskAnnotation,
    cameras: Vec<String>,
    disabled: bool,
) -> Element {
    let camera = settings.camera().unwrap_or_default().to_owned();
    let known = cameras.contains(&camera);
    let pick = annotation.clone();
    let name = annotation.clone();
    let pace = annotation.clone();
    rsx! {
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
        }
    }
}

/// How far the node has got, and what it could not annotate.
#[component]
fn AnnotationProgress(
    settings: SubtaskAnnotation,
    episodes: usize,
    loading: bool,
    running: bool,
) -> Element {
    let done = settings.completed_episodes().len();
    let failed = settings.failed_episodes();
    rsx! {
        div { class: "build-annotate-progress",
            p { role: "status",
                if loading { "Reading the dataset's episodes…" }
                else { "{done} of {episodes} episode(s) annotated." }
                match settings.camera() {
                    Some(camera) => rsx! { " Frames come from {camera}, selected and decoded by Spiral." },
                    None => rsx! { " Choose the camera to sample." },
                }
                if running { " Annotating…" }
            }
            if !failed.is_empty() {
                details { class: "build-annotate-failures", open: !running,
                    summary { "{failed.len()} episode(s) failed" }
                    ul {
                        for failure in failed {
                            li { key: "{failure.episode()}",
                                strong { "{failure.episode()}" }
                                span { "{failure.reason()}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
