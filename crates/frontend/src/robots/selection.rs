//! Selection belongs to a mounted view; it does not own hardware or project state.
use crate::robots::RobotSetupSnapshot;
use dioxus::prelude::*;
use spiral_robotics_common::robots::RobotSummary;
/// The one selection shared by the hierarchy, viewport, cameras, and inspector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    Robot(String),
    Joint { robot: String, joint: String },
    Camera { robot: String, role: String },
}

impl Selection {
    pub(crate) fn robot_id(&self) -> &str {
        match self {
            Selection::Robot(robot)
            | Selection::Joint { robot, .. }
            | Selection::Camera { robot, .. } => robot,
        }
    }

    pub(crate) fn matches_robot(&self, robot: &str) -> bool {
        self.robot_id() == robot
    }

    pub(crate) fn matches_joint(&self, robot: &str, joint: &str) -> bool {
        matches!(
            self,
            Selection::Joint {
                robot: selected_robot,
                joint: selected_joint,
            } if selected_robot == robot && selected_joint == joint
        )
    }
}

/// Choose the first configured motor only when selection does not already name
/// a valid object on an available arm. Camera selection remains intentional;
/// selecting an arm itself, arriving with no selection, or losing a stale
/// motor selection opens that arm's first motor in the inspector.
trait DefaultRobotSelection {
    fn default_joint_selection(&self, selected: Option<&Selection>) -> Option<Selection>;
}

impl DefaultRobotSelection for [RobotSummary] {
    fn default_joint_selection(&self, selected: Option<&Selection>) -> Option<Selection> {
        let robot = selected
            .and_then(|selection| self.iter().find(|robot| robot.id() == selection.robot_id()))
            .or_else(|| self.first())?;

        match selected {
            Some(Selection::Camera {
                robot: selected, ..
            }) if selected == robot.id() => return None,
            Some(Selection::Joint {
                robot: selected,
                joint,
            }) if selected == robot.id()
                && robot.motors().iter().any(|motor| motor.name() == joint) =>
            {
                return None;
            }
            _ => {}
        }

        robot.motors().first().map(|motor| Selection::Joint {
            robot: robot.id().to_owned(),
            joint: motor.name().to_owned(),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RobotSelection {
    selected: Signal<Option<Selection>>,
    pointed: Signal<Option<Selection>>,
}
impl RobotSelection {
    pub(crate) fn use_selection(
        snapshot: impl Readable<Target = RobotSetupSnapshot> + Copy + 'static,
    ) -> Self {
        let selection = use_hook(|| Self {
            selected: Signal::new(None),
            pointed: Signal::new(None),
        });
        let mut selected = selection.selected;
        use_effect(move || {
            let next = snapshot
                .read()
                .robots()
                .default_joint_selection(selected.read().as_ref());
            if let Some(next) = next {
                selected.set(Some(next));
            }
        });
        selection
    }
    pub(crate) fn selected(self) -> Signal<Option<Selection>> {
        self.selected
    }
    pub(crate) fn pointed(self) -> Signal<Option<Selection>> {
        self.pointed
    }
    pub(crate) fn focused(self, robots: &[RobotSummary]) -> Option<RobotSummary> {
        self.selected
            .read()
            .as_ref()
            .and_then(|selection| {
                robots
                    .iter()
                    .find(|robot| robot.id() == selection.robot_id())
            })
            .or_else(|| robots.first())
            .cloned()
    }
}
#[cfg(test)]
mod tests {
    use spiral_robotics_common::robots::{
        ConnectionStatus, MotorSummary, RobotAvailability, RobotSummary,
    };

    use super::{DefaultRobotSelection, Selection};

    fn robot(id: &str, motors: &[&str]) -> RobotSummary {
        RobotSummary::new(
            id,
            None,
            RobotAvailability::new(false, None, ConnectionStatus::default()),
        )
        .with_motors(
            motors
                .iter()
                .enumerate()
                .map(|(index, name)| MotorSummary::new(*name, index as u32 + 1))
                .collect(),
        )
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn canonical_selection_keeps_robot_context() {
        let joint = Selection::Joint {
            robot: "arm-a".into(),
            joint: "elbow".into(),
        };
        let camera = Selection::Camera {
            robot: "arm-a".into(),
            role: "wrist".into(),
        };

        assert_eq!(joint.robot_id(), "arm-a");
        assert!(joint.matches_joint("arm-a", "elbow"));
        assert_eq!(camera.robot_id(), "arm-a");
        assert!(!camera.matches_robot("arm-b"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_first_motor_is_the_default_object_on_the_focused_arm() {
        let robots = [
            robot("follower", &["shoulder_pan", "elbow_flex"]),
            robot("leader", &["shoulder_lift"]),
        ];

        assert_eq!(
            robots.default_joint_selection(None),
            Some(Selection::Joint {
                robot: "follower".into(),
                joint: "shoulder_pan".into(),
            })
        );
        assert_eq!(
            robots.default_joint_selection(Some(&Selection::Robot("leader".into()))),
            Some(Selection::Joint {
                robot: "leader".into(),
                joint: "shoulder_lift".into(),
            })
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_intentional_camera_or_valid_motor_selection_is_preserved() {
        let robots = [robot("follower", &["shoulder_pan"])];
        let camera = Selection::Camera {
            robot: "follower".into(),
            role: "top".into(),
        };
        let joint = Selection::Joint {
            robot: "follower".into(),
            joint: "shoulder_pan".into(),
        };

        assert_eq!(robots.default_joint_selection(Some(&camera)), None);
        assert_eq!(robots.default_joint_selection(Some(&joint)), None);
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct SelectionHarness(Rc<RefCell<SelectionOwners>>);
    #[derive(Default)]
    struct SelectionOwners {
        mounted: Option<Signal<bool>>,
        selections: Vec<RobotSelection>,
    }
    impl SelectionHarness {
        fn render(self) -> Element {
            let mounted = use_signal(|| true);
            let snapshot = use_signal(RobotSetupSnapshot::default);
            self.0.borrow_mut().mounted = Some(mounted);
            rsx! {
                if mounted() {
                    for name in ["live", "settings"] {
                        SelectionOwner {
                            key: "{name}",
                            snapshot,
                            on_created: { let owners = self.0.clone(); move |selection| owners.borrow_mut().selections.push(selection) },
                        }
                    }
                }
            }
        }
    }
    #[component]
    fn SelectionOwner(
        snapshot: Signal<RobotSetupSnapshot>,
        on_created: EventHandler<RobotSelection>,
    ) -> Element {
        let selection = RobotSelection::use_selection(snapshot.map(|value| value));
        use_hook(move || on_created.call(selection));
        rsx! {}
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn views_have_independent_selections_that_are_disposed_with_their_owner() {
        let harness = SelectionHarness::default();
        let mut dom = VirtualDom::new_with_props(SelectionHarness::render, harness.clone());
        dom.rebuild_in_place();
        let (live, settings, mut mounted) = {
            let owners = harness.0.borrow();
            (
                owners.selections[0],
                owners.selections[1],
                owners.mounted.unwrap(),
            )
        };
        dom.in_runtime(|| {
            live.selected()
                .set(Some(Selection::Robot("follower".into())));
            settings
                .selected()
                .set(Some(Selection::Robot("leader".into())));
            live.pointed().set(Some(Selection::Joint {
                robot: "follower".into(),
                joint: "elbow".into(),
            }));
        });
        dom.render_immediate_to_vec();
        assert_eq!(
            *live.selected().peek(),
            Some(Selection::Robot("follower".into()))
        );
        assert_eq!(
            *settings.selected().peek(),
            Some(Selection::Robot("leader".into()))
        );
        assert!(settings.pointed().peek().is_none());
        dom.in_runtime(|| mounted.set(false));
        dom.render_immediate_to_vec();
        assert!(live.selected().try_read().is_err());
        assert!(settings.selected().try_read().is_err());
    }
}
