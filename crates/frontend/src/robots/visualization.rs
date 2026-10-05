//! Robots drawn from their configured models.
use dioxus::prelude::*;
use spiral_robotics_common::robots::{JointTelemetry, visualization::RobotVisualization};
use spiral_robotics_viewport::RobotModel;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::RobotClient;

#[derive(Clone, PartialEq)]
pub(crate) struct ConfiguredModel {
    pub(crate) model: Rc<RobotModel>,
    pub(crate) configuration: Option<RobotVisualization>,
}
impl ConfiguredModel {
    /// A dataset's robot, whose meshes travel with its description.
    pub(crate) fn parse(
        urdf: &str,
        meshes: &BTreeMap<String, String>,
    ) -> Result<Rc<RobotModel>, String> {
        RobotModel::parse_encoded(urdf, meshes).map(Rc::new)
    }

    /// The models `robots` are drawn with, by robot ID. Each model is fetched
    /// from the robot connector, which the browser then caches for good, and
    /// parsed again only when the list of robots and models changes.
    pub(crate) fn use_configured(
        robots: Memo<Vec<(String, RobotVisualization)>>,
    ) -> Resource<Result<BTreeMap<String, Self>, String>> {
        let client = RobotClient::current();
        use_resource(move || {
            let robots = robots();
            let requests = robots
                .iter()
                .map(|(_, configuration)| configuration.model().clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|model| {
                    let request = client.robot_model(&model);
                    (model, request)
                })
                .collect::<Vec<_>>();
            async move {
                let mut models = BTreeMap::new();
                for (id, request) in requests {
                    let files = request.await.map_err(|error| error.to_string())?;
                    let model = Self::parse(files.urdf(), files.meshes())?;
                    models.insert(id, model);
                }
                Ok(robots
                    .into_iter()
                    .map(|(robot, configuration)| {
                        let model = models[configuration.model()].clone();
                        let configured = Self {
                            model,
                            configuration: Some(configuration),
                        };
                        (robot, configured)
                    })
                    .collect())
            }
        })
    }

    pub(crate) fn measured_positions(&self, telemetry: &[JointTelemetry]) -> BTreeMap<String, f32> {
        self.configuration
            .as_ref()
            .expect("project robot configuration")
            .measured_positions(telemetry)
    }
}
