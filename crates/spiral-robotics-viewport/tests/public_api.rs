use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;

use spiral_robotics_viewport::{
    ArmPose, ArmPresentation, CameraAttachment, CameraMount, Command, Condition, Frame, FrameError,
    Interaction, Joint, Target, ViewportError,
};

#[test]
fn a_caller_can_compose_one_valid_frame_without_renderer_types() {
    let pose = ArmPose::from_degrees(
        &BTreeMap::from([
            ("shoulder_pan".to_owned(), 12.0),
            ("elbow_flex".to_owned(), -8.0),
        ]),
        -0.5,
    );
    let elbow = Target::Joint {
        robot: "follower".to_owned(),
        joint: Joint::ElbowFlex.name().to_owned(),
    };
    let mut frame = Frame::new(true);

    frame
        .add_arm(
            "follower",
            ArmPresentation::new([0.29, 0.56, 0.85, 1.0], Some(pose)),
        )
        .unwrap()
        .mount_camera(
            "follower",
            CameraMount::new(
                "wrist",
                CameraAttachment::new(Some("wrist_link"), [0.0, 0.0, 0.04], [0.0, 0.0, 0.0]),
                62.0,
            ),
        )
        .unwrap();

    assert!(frame.set_interaction(elbow.clone(), Interaction::Selected));
    assert!(frame.set_condition(elbow, Condition::Caution));
    assert_eq!(pose.degrees(Joint::ShoulderPan), Some(12.0));
    assert_eq!(pose.gripper_opening(), 0.75);
    assert!(
        pose.tool_position()
            .is_some_and(|position| position.into_iter().all(f32::is_finite))
    );
}

#[test]
fn boundary_values_support_the_traits_the_app_needs() {
    fn copy_debug<T: Copy + Debug>() {}
    fn semantic_key<T: Clone + Debug + Eq + Hash + Ord>() {}
    fn error<T: std::error::Error + Send + Sync + 'static>() {}

    copy_debug::<ArmPose>();
    copy_debug::<Command>();
    copy_debug::<Condition>();
    copy_debug::<Interaction>();
    copy_debug::<Joint>();
    semantic_key::<Target>();
    error::<FrameError>();
    error::<ViewportError>();

    assert_eq!(Joint::try_from("wrist_roll"), Ok(Joint::WristRoll));
    assert_eq!(Joint::WristRoll.to_string(), "wrist_roll");
}
