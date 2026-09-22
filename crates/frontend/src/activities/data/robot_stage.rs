//! The recorded robot of an episode, driven solely by the episode clock.
//!
//! One read answers both questions a composition has: *does this dataset ship
//! a robot model at all*, and *where were its joints at this instant*. A
//! dataset without a model reads as `Ok(None)`, which is how [`RobotStage`]
//! knows to draw nothing rather than a tile explaining its own emptiness.
use super::{
    DatasetClient, EpisodeTransport, OpenedEpisode,
    episode_streams::{EpisodeStreams, StreamCatalog},
    message_buffer::MessageBuffer,
};
use crate::panels::scene::{SceneSource, Viewport};
use crate::robots::RobotSelection;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetRobot, JointBinding, MessageWindow};
use std::{collections::BTreeMap, rc::Rc};

/// A dataset's robot model together with the recorded streams that move it,
/// read once for one episode.
#[derive(Clone)]
pub(super) struct RecordedRobot {
    model: Rc<DatasetRobot>,
    joints: Rc<Vec<JointSamples>>,
}
impl PartialEq for RecordedRobot {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.model, &other.model) && Rc::ptr_eq(&self.joints, &other.joints)
    }
}
impl RecordedRobot {
    /// Reads the model and the joint messages covering `bounds`. `Ok(None)`
    /// means the dataset has no robot model; nothing is drawn for it.
    fn use_episode(
        episode: &OpenedEpisode,
        streams: &StreamCatalog,
        bounds: (u64, u64),
    ) -> Resource<Result<Option<Self>, String>> {
        let dataset = episode.dataset().clone();
        let streams = streams.clone();
        use_resource(move || {
            let dataset = dataset.clone();
            let streams = streams.clone();
            async move {
                let Some(model) = DatasetClient::current()
                    .robot(&dataset)
                    .await
                    .map_err(|error| error.to_string())?
                else {
                    return Ok(None);
                };
                let joints = JointSamples::load(&streams, &model, bounds).await?;
                Ok(Some(Self {
                    model: Rc::new(model),
                    joints: Rc::new(joints),
                }))
            }
        })
    }
    /// Every bound joint's position at `time`, or `None` when one of the
    /// streams has nothing to say yet.
    fn pose(&self, time: u64) -> Option<BTreeMap<String, f32>> {
        self.joints
            .iter()
            .map(|samples| samples.at(time))
            .collect::<Option<Vec<_>>>()
            .map(|poses| poses.into_iter().flatten().collect())
    }
}

/// The robot as one tile of an episode composition, drawn only when the
/// dataset has a model. While the model is being read, and forever after for
/// a dataset without one, this renders nothing at all: the composition's grid
/// simply has one fewer tile.
#[component]
pub(super) fn RobotStage(
    episode: OpenedEpisode,
    streams: StreamCatalog,
    transport: EpisodeTransport,
) -> Element {
    let robot = RecordedRobot::use_episode(&episode, &streams, transport.range().bounds());
    let read = robot.read();
    match read.as_ref() {
        None | Some(Ok(None)) => rsx! {},
        Some(Ok(Some(robot))) => rsx! {
            section { class: "episode-inspection-tile episode-inspection-stage",
                h5 { "Robot" }
                div { class: "episode-inspection-view",
                    RobotScene { robot: robot.clone(), transport }
                }
            }
        },
        Some(Err(error)) => rsx! {
            section { class: "episode-inspection-tile episode-inspection-stage",
                h5 { "Robot" }
                p { class: "episode-inspection-status", role: "alert", "3D view unavailable: {error}" }
            }
        },
    }
}

/// Data's dockable stage, which explains itself: it is the panel an operator
/// opened on purpose, so an episode without a model says so instead of
/// leaving the dock blank.
#[component]
pub(super) fn RobotStagePanel(
    episode: Option<OpenedEpisode>,
    streams: Option<EpisodeStreams>,
    transport: EpisodeTransport,
) -> Element {
    let Some(episode) = episode else {
        return rsx! { div { class: "data-plots-empty", "Open an episode to see its robot." } };
    };
    let streams = match streams {
        Some(EpisodeStreams::Ready(streams)) => streams,
        Some(EpisodeStreams::Failed(_)) => {
            return rsx! { div { class: "data-plots-empty", role: "status", "3D view unavailable." } };
        }
        _ => return rsx! { div { class: "data-plots-empty", role: "status", "Loading 3D view…" } },
    };
    rsx! { DatasetStage { key: "{episode.dataset().label()}:{episode.record().id()}", episode, streams, transport } }
}

#[component]
fn DatasetStage(
    episode: OpenedEpisode,
    streams: StreamCatalog,
    transport: EpisodeTransport,
) -> Element {
    let mut robot = RecordedRobot::use_episode(&episode, &streams, transport.range().bounds());
    let read = robot.read();
    match read.as_ref() {
        None => rsx! { div { class: "data-plots-empty", role: "status", "Loading 3D view…" } },
        Some(Ok(None)) => {
            rsx! { div { class: "data-plots-empty", "No robot model in this dataset." } }
        }
        Some(Err(_)) => {
            rsx! { div { class: "data-plots-empty", role: "status", p { "3D view unavailable." } button { class: "data-text-action", onclick: move |_| robot.restart(), "Retry" } } }
        }
        Some(Ok(Some(robot))) => rsx! { RobotScene { robot: robot.clone(), transport } },
    }
}

/// The recorded robot drawn at the shared playhead.
#[component]
fn RobotScene(robot: RecordedRobot, transport: EpisodeTransport) -> Element {
    let robot_selection = RobotSelection::use_empty();
    let positions = robot.pose(transport.range().ns_at(transport.seconds()));
    rsx! {
        Viewport { robot_selection,
            source: SceneSource::Dataset { robot: robot.model.clone(), positions },
            presentation: Default::default()
        }
    }
}

struct JointSamples {
    bindings: Vec<JointBinding>,
    buffer: Rc<MessageBuffer>,
}
impl JointSamples {
    fn at(&self, time: u64) -> Option<BTreeMap<String, f32>> {
        let message = self.buffer.json_at(time).ok()??;
        self.bindings
            .iter()
            .map(|binding| {
                Some((
                    binding.joint().to_owned(),
                    binding.read_position(message.get("value")?)?,
                ))
            })
            .collect()
    }
    async fn load(
        streams: &StreamCatalog,
        robot: &DatasetRobot,
        bounds: (u64, u64),
    ) -> Result<Vec<Self>, String> {
        let mut groups = BTreeMap::<(String, String), Vec<JointBinding>>::new();
        for binding in robot.bindings() {
            groups
                .entry((binding.topic().into(), binding.schema().into()))
                .or_default()
                .push(binding.clone());
        }
        let mut samples = Vec::new();
        for ((topic, schema), bindings) in groups {
            let mut matching = streams
                .streams()
                .iter()
                .filter(|stream| stream.name() == topic && stream.schemas().contains(&schema));
            let stream = matching
                .next()
                .ok_or("Recorded joint stream is unavailable")?;
            if matching.next().is_some() {
                return Err("Joint stream has ambiguous schema revisions".into());
            }
            let read = stream
                .read()
                .clone()
                .with_window(MessageWindow::new(bounds, vec![]));
            samples.push(Self {
                bindings,
                buffer: MessageBuffer::load(read, true).await?,
            });
        }
        Ok(samples)
    }
}
