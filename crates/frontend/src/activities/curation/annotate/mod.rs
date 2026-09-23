//! Break down subtasks: browser-owned annotation of every episode that
//! reaches the node, one model call per episode, rows persisted through the
//! controller after each episode.
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

use super::document::{CurationNode, CurationNodeKind, SubtaskAnnotation, SubtaskRow};
use super::poster::{PosterCut, PosterFrames};
use super::rerun::ConfirmedRerun;
use super::review::{
    EpisodeOrder, MeasurementStatus, NoSubtaskChosen, SubtaskContext, SubtaskReview,
};
use super::state::CurationController;
use crate::activities::data::DatasetClient;
use crate::components::icons::SettingsIcon;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use episodes::AnnotationEpisode;
use journal::CurationJournal;
use native::NativeSheets;
use output::{AnnotatedEpisode, AnnotationOutput, ChosenSubtask, EpisodeSubtasks, SubtaskIndex};
use pipeline::{AnnotationPass, EPISODES_IN_FLIGHT, PassSettings};
use provider::ProviderAnnotator;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, Field, FieldLayout, Place, Select,
    TextInput, TextInputKind,
};
use std::fmt::{Display, Formatter, Result as FormatResult};

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
    controller: CurationController,
}

impl AnnotationNode {
    fn new(id: String, controller: CurationController) -> Self {
        Self { id, controller }
    }
    /// The node's configuration and progress, or nothing when the selected
    /// node is not an annotation after all.
    fn settings(&self) -> Option<SubtaskAnnotation> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).map(CurationNode::kind) {
                Some(CurationNodeKind::BreakDownSubtasks(annotation)) => Some(annotation.clone()),
                _ => None,
            }
        })
    }
    /// The rows the node hands on: those of the episodes reaching it.
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
    let studio = StudioProjectState::current();
    let annotation = AnnotationNode::new(node.clone(), controller);
    let dataset = controller.with_payload(|payload| payload.dataset().cloned());
    let listed = AnnotationEpisode::reaching(controller, &node);
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
        let Some(camera) =
            crate::activities::episode::StreamCatalog::overview_camera(&cameras).map(str::to_owned)
        else {
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

    // The client belongs to this mount, and a pass outlives the click that
    // started it, so it is captured here rather than read inside its task.
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
        move |chosen: Vec<AnnotationEpisode>| {
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
            if chosen.is_empty() {
                return;
            }
            let interval = settings.sample_interval_seconds();
            let node = annotation.id.clone();
            let model_name = settings.model().to_owned();
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
                    ProviderAnnotator,
                    CurationJournal::new(node.clone(), controller, studio, wanted),
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
            section { class: "curation-annotate-workspace", role: "alert",
                "This node does not break down subtasks." }
        };
    };
    let rows = annotation.rows();
    let groups = EpisodeSubtasks::group(rows.clone().into_iter());
    let context = SubtaskContext::new(
        dataset.clone(),
        settings.camera().map(str::to_owned),
        EpisodeOrder::of(listed.iter().map(AnnotationEpisode::id)),
    );
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
    let shown = chosen
        .episode()
        .or_else(|| groups.first().map(|group| group.episode().clone()))
        .and_then(|id| {
            let episode = listed.iter().find(|episode| episode.id() == &id)?.clone();
            let subtasks = groups.iter().find(|group| group.episode() == &id).cloned();
            Some((episode, subtasks))
        });
    let done = listed
        .iter()
        .filter(|episode| settings.is_completed(episode.id()))
        .count();
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
    let replaceable = RunScope::All.episodes(&listed, &settings).len();
    let retry = {
        let listed = listed.clone();
        let mut start = start.clone();
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
            MeasurementStatus {
                facts: AnnotationFacts::new(settings.clone(), (done, listed.len()), running()).to_string(),
                note: if settings.camera().is_none() {
                    Some("Choose a camera in Settings before running.".to_owned())
                } else { None },
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
                                let episodes = scope.episodes(&listed, &settings);
                                let mut start = start.clone();
                                rsx! {
                                    if count > 0 {
                                        Button {
                                            key: "{scope.label()}",
                                            purpose: scope.purpose(),
                                            variant: if scope == RunScope::Pending { ButtonVariant::Solid } else { ButtonVariant::Outline },
                                            disabled: settings.camera().is_none(),
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
                    AnnotationSettings {
                        annotation: annotation.clone(),
                        settings: settings.clone(),
                        cameras,
                        disabled: running(),
                        reannotate: {
                            let mut start = start.clone();
                            let episodes = RunScope::All.episodes(&listed, &settings);
                            rsx! {
                                if replaceable > 0 && !running() {
                                    ConfirmedRerun {
                                        id: "curation-annotate-replace".to_owned(),
                                        label: RunScope::All.label().to_owned(),
                                        question: format!(
                                            "Annotate all {replaceable} episodes again? This replaces the {} subtasks already stored, and costs another full run.",
                                            rows.len(),
                                        ),
                                        on_confirm: move |()| start(episodes.clone()),
                                    }
                                }
                            }
                        },
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
                        output: AnnotationOutput::new(
                            groups.clone(),
                            settings.failed_episodes().to_vec(),
                            context.clone(),
                        ),
                        frames,
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

/// The one line a Break down subtasks node reports itself in: what it has
/// measured, and what it measured with.
///
/// It used to be four bands and six phrasings of one fact — `Part way`,
/// `39 of 40 episodes`, `1 to go`, `1 failed`, `Run 1 left`, `Retry failed` —
/// standing between the operator and the first row.
#[derive(Clone, PartialEq)]
struct AnnotationFacts {
    settings: SubtaskAnnotation,
    /// How many of the episodes reaching the node it has annotated, of how many.
    reached: (usize, usize),
    running: bool,
}

impl AnnotationFacts {
    fn new(settings: SubtaskAnnotation, reached: (usize, usize), running: bool) -> Self {
        Self {
            settings,
            reached,
            running,
        }
    }
}

impl Display for AnnotationFacts {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FormatResult {
        let (done, episodes) = self.reached;
        let failed = self.settings.failed_episodes().len();
        if self.running {
            write!(
                formatter,
                "Annotating up to {EPISODES_IN_FLIGHT} at once · "
            )?;
        }
        write!(formatter, "{done}/{episodes} episodes")?;
        if failed > 0 {
            write!(formatter, " · {} failed", failed)?;
        }
        write!(
            formatter,
            " · {} · {} · every {}s",
            self.settings.camera().unwrap_or("no camera"),
            self.settings.model(),
            self.settings.sample_interval_seconds(),
        )
    }
}

/// The camera, model, and sampling one pass runs with, plus the run that
/// replaces what they already produced — behind the ⚙ on the status line
/// rather than in a band of their own.
/// The settings dialog answers Escape and the backdrop, so it needs a name
/// focus can reach it by.
const SETTINGS_DIALOG_ID: &str = "curation-annotate-settings";

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
    let pace = annotation.clone();
    rsx! {
        span { class: "curation-annotate-settings-slot",
            Button {
                purpose: ActionPurpose::Navigate,
                variant: ButtonVariant::Icon,
                aria_haspopup: "dialog",
                aria_expanded: open(),
                aria_label: "Settings",
                title: "Camera, model, and sampling",
                onclick: move |_| open.toggle(),
                SettingsIcon {}
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
                                    let _ = pace.configure(|annotation| {
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
