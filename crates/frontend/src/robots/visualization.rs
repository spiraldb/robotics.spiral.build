use base64::Engine as _;
use spiral_robotics_common::robots::{
    JointTelemetry,
    visualization::{RobotVisualization, TelemetryCoordinate},
};
use spiral_robotics_viewport::RobotModel;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, PartialEq)]
pub(crate) struct ConfiguredModel {
    pub(crate) model: Rc<RobotModel>,
    pub(crate) configuration: Option<RobotVisualization>,
}
impl ConfiguredModel {
    pub(crate) fn parse(
        urdf: &str,
        meshes: &BTreeMap<String, String>,
    ) -> Result<Rc<RobotModel>, String> {
        let meshes = meshes
            .iter()
            .map(|(name, bytes)| {
                base64::engine::general_purpose::STANDARD
                    .decode(bytes)
                    .map(|bytes| (name.clone(), bytes))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?;
        RobotModel::parse(urdf, &meshes).map(Rc::new)
    }
    pub(crate) fn new(configuration: RobotVisualization) -> Result<Self, String> {
        Ok(Self {
            model: Self::parse(configuration.urdf(), configuration.meshes())?,
            configuration: Some(configuration),
        })
    }
    pub(crate) fn measured_positions(&self, telemetry: &[JointTelemetry]) -> BTreeMap<String, f32> {
        self.configuration
            .as_ref()
            .expect("project robot configuration")
            .positions(|name, input| {
                let joint = telemetry.iter().find(|joint| joint.name() == name)?;
                match input {
                    TelemetryCoordinate::Degrees => joint.degrees(),
                    TelemetryCoordinate::Normalized => joint.normalized(),
                }
            })
    }
}
