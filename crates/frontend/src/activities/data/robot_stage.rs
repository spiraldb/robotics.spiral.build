//! A dockable recorded robot, driven solely by the episode clock.
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

#[component]
pub(super) fn RobotStage(
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
    let selection = episode.dataset().clone();
    let bounds = transport.range().bounds();
    let robot_selection = RobotSelection::use_empty();
    let mut robot = use_resource(move || {
        let selection = selection.clone();
        let streams = streams.clone();
        async move {
            let Some(robot) = DatasetClient::current()
                .robot(&selection)
                .await
                .map_err(|error| error.to_string())?
            else {
                return Ok::<_, String>(None);
            };
            let samples = JointSamples::load(&streams, &robot, bounds).await?;
            Ok(Some((Rc::new(robot), samples)))
        }
    });
    let result = robot.read();
    match result.as_ref() {
        None => rsx! { div { class: "data-plots-empty", role: "status", "Loading 3D view…" } },
        Some(Ok(None)) => {
            rsx! { div { class: "data-plots-empty", "No robot model in this dataset." } }
        }
        Some(Err(_)) => {
            rsx! { div { class: "data-plots-empty", role: "status", p { "3D view unavailable." } button { class: "data-text-action", onclick: move |_| robot.restart(), "Retry" } } }
        }
        Some(Ok(Some((robot, samples)))) => {
            let time = transport.range().ns_at(transport.seconds());
            let positions = samples
                .iter()
                .map(|sample| sample.at(time))
                .collect::<Option<Vec<_>>>()
                .map(|poses| poses.into_iter().flatten().collect());
            rsx! { Viewport { robot_selection,
                source: SceneSource::Dataset { robot: robot.clone(), positions }, presentation: Default::default()
            } }
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
