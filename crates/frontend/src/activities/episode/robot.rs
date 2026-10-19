//! The recorded robot of an episode, driven solely by the episode clock.
//!
//! Whether a dataset ships a robot model is read once per inspected episode,
//! because it decides whether the recording has a 3D view to lay out at all.
//! Where the joints were at each instant is read by that view itself.
use super::{messages::MessageBuffer, streams::StreamCatalog, transport::EpisodeTransport};
use crate::activities::data::DatasetClient;
use crate::panels::scene::{SceneSource, Viewport};
use crate::robots::RobotSelection;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetRobot, DatasetSelection, JointBinding, MessageWindow,
};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant};
use std::{collections::BTreeMap, rc::Rc};

/// A dataset's robot model.
#[derive(Clone)]
pub(super) struct RobotModel(Rc<DatasetRobot>);

impl PartialEq for RobotModel {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl RobotModel {
    /// The model the version of `dataset` the project inspected ships, or
    /// `None` when it has none to draw.
    pub(super) fn use_dataset(
        dataset: &DatasetSelection,
    ) -> Resource<Result<Option<Self>, String>> {
        let dataset = dataset.clone();
        use_resource(move || {
            let dataset = dataset.clone();
            async move {
                let client = DatasetClient::current();
                let inspection = client
                    .inspect_dataset(dataset)
                    .await
                    .map_err(|error| error.to_string())?;
                client
                    .robot(inspection.dataset())
                    .await
                    .map(|model| model.map(|model| Self(Rc::new(model))))
                    .map_err(|error| error.to_string())
            }
        })
    }
}

/// The episode's robot at the shared playhead.
#[component]
pub(super) fn RobotStage(
    model: RobotModel,
    streams: StreamCatalog,
    transport: EpisodeTransport,
) -> Element {
    let bounds = transport.range().bounds();
    let joint_model = model.clone();
    let mut joints = use_resource(move || {
        let streams = streams.clone();
        let model = joint_model.clone();
        async move {
            JointSamples::load(&streams, &model.0, bounds)
                .await
                .map(Rc::new)
        }
    });
    match &*joints.read() {
        None => {
            rsx! { p { class: "episode-panel-status", role: "status", "Loading the recorded robot…" } }
        }
        Some(Err(error)) => rsx! {
            div { class: "episode-panel-status", role: "alert",
                p { "The robot's joints could not be read: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    onclick: move |_| joints.restart(), "Retry" }
            }
        },
        Some(Ok(joints)) => rsx! {
            RobotScene { robot: RecordedRobot { model: model.0.clone(), joints: joints.clone() }, transport }
        },
    }
}

/// A dataset's robot model together with the recorded streams that move it,
/// read once for one episode.
#[derive(Clone)]
struct RecordedRobot {
    model: Rc<DatasetRobot>,
    joints: Rc<Vec<JointSamples>>,
}

impl PartialEq for RecordedRobot {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.model, &other.model) && Rc::ptr_eq(&self.joints, &other.joints)
    }
}

impl RecordedRobot {
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
            let mut matching = streams.streams().iter().filter(|stream| {
                stream.name() == topic && stream.schema() == Some(schema.as_str())
            });
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
                buffer: MessageBuffer::load(read).await?,
            });
        }
        Ok(samples)
    }
}
