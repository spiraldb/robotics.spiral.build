//! Track objects: track what the node names on one camera of every episode
//! that reaches it, with SAM 3.1 on the Modal service at the node's rate, and
//! add each episode's timeline of tracked pictures to the table.
//!
//! The tracker follows every object it finds by one track to the end, so
//! the most objects one sampled picture held counts an object hidden for a
//! moment, or carried out of sight. The node keeps nothing by itself: a
//! Select below it keeps by how many it saw. Each episode opens on the
//! tracked camera with every object boxed. The tracks are stored once
//! tracked, and what the node reads is how many objects each picture held.
mod tracking;
mod tracks;

pub(super) use tracks::{StoredTracks, TrackedEpisodeTab};

use super::document::{CurationNode, CurationNodeKind, ObjectTracking};
use super::measure_run::{
    MeasureInput, MeasureRun, MeasureRunActions, MeasureRunStart, MeasureVerb, MeasuredEpisode,
    MeasuredEpisodes,
};
use super::preview::PreviewPlan;
use super::primer::{NodePrimer, Primer};
use super::review::{NodeBar, NodeNotes, NodeReach};
use super::state::CurationController;
use super::thumbnail::Thumbnails;
use crate::activities::episode::{Episode, PlayRequest, StreamCatalog};
use crate::components::icons::SpinnerIcon;
use dioxus::prelude::*;
use spiral_robotics_ui::{EmptyState, Notice, NoticeTone, Select, TextInput, TextInputKind};

/// One Track objects node of the open curation.
#[derive(Clone, PartialEq)]
struct TrackedNode {
    id: String,
    controller: CurationController,
}

impl TrackedNode {
    /// The node's configuration, or nothing when the selected node does not
    /// track objects after all.
    fn settings(&self) -> Option<ObjectTracking> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).map(CurationNode::kind) {
                Some(CurationNodeKind::TrackObjects(tracking)) => Some(tracking.clone()),
                _ => None,
            }
        })
    }
    /// Apply one configuration change.
    fn configure(&self, change: impl FnOnce(&mut ObjectTracking)) {
        let id = self.id.clone();
        let _ = self.controller.edit(|payload| {
            match payload.node_mut(&id).map(CurationNode::kind_mut) {
                Some(CurationNodeKind::TrackObjects(tracking)) => {
                    change(tracking);
                    Ok(())
                }
                _ => Err("That node does not track objects.".to_owned()),
            }
        });
    }
}

/// The workspace shown for a selected Track objects node: what it tracks, on
/// which camera and how often, on the node's bar beside the run, and the
/// episodes it tracked with the most each showed at once.
#[component]
pub(super) fn TrackingWorkspace(controller: CurationController, node: String) -> Element {
    let tracked = TrackedNode {
        id: node.clone(),
        controller,
    };
    let run = MeasureRun::use_run(controller, &node, MeasureVerb::Track);
    let input = MeasureInput::of(controller, &node);
    // Every episode of a dataset records the same cameras, and the first one
    // is known before anything reaches the node.
    let cameras = StreamCatalog::use_camera_topics(
        controller
            .sample()
            .and_then(|sample| sample.episodes().first().map(Episode::selection)),
    );
    // A new node starts on the camera that shows the whole workspace, so a
    // run can start as soon as it names what to track.
    let starting = tracked.clone();
    use_effect(use_reactive(&cameras, move |cameras| {
        let (Some(cameras), Some(settings)) = (cameras, starting.settings()) else {
            return;
        };
        if settings.camera().is_none()
            && let Some(overview) = StreamCatalog::overview_camera(&cameras)
        {
            let overview = overview.to_owned();
            starting.configure(|tracking| tracking.set_camera(Some(overview)));
        }
    }));
    let unsupported = cameras
        .as_ref()
        .filter(|cameras| cameras.is_empty())
        .map(|_| "This dataset records no camera to track objects on".to_owned());
    let uncompiled = controller
        .with_payload(|payload| PreviewPlan::check(payload, &node))
        .err()
        .map(|error| error.message().to_owned());
    let settings = tracked.settings();
    let thumbnails = Thumbnails::use_camera(
        controller,
        settings.as_ref().and_then(ObjectTracking::camera),
    );
    let (Some(input), Some(settings)) = (input, settings) else {
        return rsx! {
            section { class: "curation-track-objects-workspace",
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
    // Nothing tracked yet: the workspace says what tracking is for, beside
    // the run that starts it, rather than an empty index.
    let unmeasured = input.preview().is_unmeasured();
    let stored = StoredTracks::of(&settings);
    let episodes = input
        .episodes()
        .iter()
        .filter_map(|episode| {
            let most = controller.timeline(&node, episode.id())?.most();
            Some(MeasuredEpisode::new(
                episode.clone(),
                most.map(|most| format!("{most:.0}")),
            ))
        })
        .collect::<Vec<_>>();
    rsx! {
        section { class: "curation-track-objects-workspace", "aria-label": "Track objects",
            NodeBar {
                lead: rsx! { TrackedSettings { tracked, settings: settings.clone(), cameras: cameras.unwrap_or_default() } },
                note: NodeNotes::join([unsupported, uncompiled, run.problem(), input.failures()]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! { MeasureRunActions { run: run.clone(), input: input.clone(), blocked } },
            }
            if unmeasured {
                NodePrimer {
                    primer: Primer::TrackObjects,
                    action: rsx! { MeasureRunStart { run, input, blocked } },
                }
            } else if let Some(stored) = stored {
                MeasuredEpisodes {
                    episodes,
                    meaning: "The most seen in one picture",
                    thumbnails,
                    render: move |(episode, play): (Episode, Option<PlayRequest>)| rsx! {
                        TrackedEpisodeTab { episode, tracks: stored.clone(), focus: play }
                    },
                }
            }
        }
    }
}

/// What the node tracks, typed and kept on Enter or when the field is left,
/// the camera it tracks on, and how many pictures a second.
#[component]
fn TrackedSettings(
    tracked: TrackedNode,
    settings: ObjectTracking,
    cameras: Vec<String>,
) -> Element {
    let mut draft = use_signal(|| settings.prompt().to_owned());
    let saved = settings.prompt().to_owned();
    use_effect(use_reactive(&saved, move |saved| draft.set(saved)));
    let keep = {
        let tracked = tracked.clone();
        move || {
            let typed = draft.peek().clone();
            tracked.configure(|tracking| tracking.set_prompt(&typed));
        }
    };
    let camera = settings.camera().unwrap_or_default().to_owned();
    let keep_on_enter = keep.clone();
    let paced = tracked.clone();
    rsx! {
        // Not a form: with the rate field beside it, Enter would submit none.
        div { class: "curation-track-objects-settings",
            label { class: "curation-track-objects-label", r#for: "curation-track-objects-prompt", "Track" }
            TextInput {
                id: "curation-track-objects-prompt",
                class: "curation-track-objects-prompt",
                value: draft(),
                placeholder: "bottle",
                "aria-label": "What to track",
                oninput: move |value| draft.set(value),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter {
                        keep_on_enter();
                    }
                },
                onblur: move |_| keep(),
            }
            label { class: "curation-track-objects-label", r#for: "curation-track-objects-camera", "on" }
            Select {
                id: "curation-track-objects-camera",
                value: camera.clone(),
                disabled: cameras.is_empty(),
                "aria-label": "Camera to track on",
                onchange: move |value: String| {
                    tracked.configure(|tracking| tracking.set_camera(Some(value.clone())));
                },
                option { value: "", disabled: true, selected: camera.is_empty(), "Choose a camera" }
                for topic in cameras.iter() {
                    option { key: "{topic}", value: "{topic}", selected: *topic == camera,
                        "{StreamCatalog::camera_title_among(topic, &cameras)}" }
                }
            }
            label { class: "curation-track-objects-label", r#for: "curation-track-objects-rate", "at" }
            TextInput {
                id: "curation-track-objects-rate",
                class: "curation-track-objects-rate",
                kind: TextInputKind::Number,
                value: settings.rate().to_string(),
                min: "1",
                max: "{ObjectTracking::MAX_RATE}",
                step: "1",
                "aria-label": "Pictures a second",
                oninput: move |value: String| {
                    if let Ok(rate) = value.trim().parse::<u32>() {
                        paced.configure(|tracking| {
                            let _ = tracking.set_rate(rate);
                        });
                    }
                },
            }
            span { class: "curation-track-objects-label", "pictures/s" }
        }
    }
}
