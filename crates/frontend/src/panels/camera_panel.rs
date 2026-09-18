//! Camera panel identity, source setup, and independent first-open camera rows.

use super::camera::{CameraEditor, CameraEditorSource, RecordedCameraSourceRequest};
use crate::robots::Selection;
use dioxus::prelude::*;
use dioxus_workbench::{DockZone, LayoutNode, Panel, PanelId, PanelLayout, SplitAxis};
use spiral_robotics_common::robots::{CameraBinding, RobotSummary};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AssignedCamera {
    robot: String,
    role: String,
    device: String,
}

impl From<(&RobotSummary, &CameraBinding)> for AssignedCamera {
    fn from((robot, camera): (&RobotSummary, &CameraBinding)) -> Self {
        Self {
            robot: robot.id().to_owned(),
            role: camera.name().to_owned(),
            device: camera.id().to_owned(),
        }
    }
}

impl AssignedCamera {
    pub(crate) fn panel_id(&self) -> PanelId {
        PanelId::from(format!("camera:{}:{}", self.robot, self.role))
    }

    pub(crate) fn selection(&self) -> Selection {
        Selection::Camera {
            robot: self.robot.clone(),
            role: self.role.clone(),
        }
    }

    pub(crate) fn live_source(&self) -> CameraEditorSource {
        CameraEditorSource::Live {
            robot: self.robot.clone(),
            role: self.role.clone(),
            device: self.device.clone(),
        }
    }

    pub(crate) fn recorded_source(
        &self,
        request: RecordedCameraSourceRequest<'_>,
    ) -> CameraEditorSource {
        request.for_live_assignment(&(self.robot.clone(), self.role.clone(), self.device.clone()))
    }

    pub(crate) fn panel(&self, source: CameraEditorSource) -> Panel {
        let id = self.panel_id();
        Panel::new(
            id.clone(),
            format!("Camera · {}", self.role),
            "cameras",
            rsx! {
                CameraEditor { element_id: id.as_str().recorded_element_id(), source }
            },
        )
        .with_home_zone(DockZone::Bottom)
        .with_class("camera-editor")
    }

    pub(crate) fn unassigned_panel() -> Panel {
        Panel::new(
            "camera-empty",
            "Camera",
            "cameras",
            rsx! {
                CameraEditor {
                    element_id: "recorded-camera-empty".to_owned(),
                    source: CameraEditorSource::Empty {
                        title: "No camera assigned".to_owned(),
                        detail: "Assign a camera to this arm in Settings.".to_owned(),
                    },
                }
            },
        )
        .with_class("camera-editor")
    }
}

trait CameraPanelPresentation {
    fn recorded_element_id(&self) -> String;
}

impl CameraPanelPresentation for str {
    fn recorded_element_id(&self) -> String {
        let safe = self
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        format!("recorded-{safe}")
    }
}

/// Give each camera a visible row. The caller decides where this subtree goes.
pub(crate) trait CameraPanelRows {
    fn camera_rows(&self) -> Option<LayoutNode>;
}

impl CameraPanelRows for [PanelId] {
    fn camera_rows(&self) -> Option<LayoutNode> {
        CameraRows::new(self).map(|rows| rows.layout(0))
    }
}

/// Standard workbench, scene/cameras, and lower visualization arrangement.
pub(crate) struct CameraWorkspaceLayout;

impl CameraWorkspaceLayout {
    pub(crate) fn initial(
        panels: &[Panel],
        lower_panel: impl Into<PanelId>,
        workbench_fraction: f64,
    ) -> PanelLayout {
        let camera_ids = panels
            .iter()
            .filter(|panel| panel.home.as_str() == "cameras")
            .map(|panel| panel.id.clone())
            .collect::<Vec<_>>();
        let primary = LayoutNode::tile("primary", ["scene".to_owned()]);
        let editors = match camera_ids.camera_rows() {
            Some(cameras) => LayoutNode::split(
                "editor-columns",
                SplitAxis::Horizontal,
                2.0 / 3.0,
                primary,
                cameras,
            ),
            None => primary,
        };
        let visualization = LayoutNode::split(
            "visualization-rows",
            SplitAxis::Vertical,
            0.58,
            editors,
            LayoutNode::tile("timeline", [lower_panel.into()]),
        );
        PanelLayout::new(LayoutNode::split(
            "studio-columns",
            SplitAxis::Horizontal,
            workbench_fraction,
            LayoutNode::tile("workbench", ["workbench".to_owned()]),
            visualization,
        ))
    }
}

struct CameraRows<'a>(&'a [PanelId]);

impl<'a> CameraRows<'a> {
    fn new(panels: &'a [PanelId]) -> Option<Self> {
        (!panels.is_empty()).then_some(Self(panels))
    }

    fn layout(&self, offset: usize) -> LayoutNode {
        let tile_id = if offset == 0 {
            "cameras".to_owned()
        } else {
            format!("cameras-{}", offset + 1)
        };
        let first = LayoutNode::tile(tile_id, [self.0[0].clone()]);
        match Self::new(&self.0[1..]) {
            None => first,
            Some(rest) => LayoutNode::split(
                format!("camera-rows-{}", offset + 1),
                SplitAxis::Vertical,
                1.0 / self.0.len() as f64,
                first,
                rest.layout(offset + 1),
            ),
        }
    }
}
