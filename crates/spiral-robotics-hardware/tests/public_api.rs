use std::collections::BTreeMap;
use std::fmt::Debug;

use spiral_robotics_hardware::{
    CameraRuntime, ControlSettings, HardwareInputError, HardwareState, JointGoals, MotorSpec,
    NormalizedPosition, PortSettings, ReplayRequest, ReplayTrajectory, RobotSpec,
};

#[test]
fn a_caller_can_compose_valid_hardware_inputs_from_the_flat_boundary() {
    let port = PortSettings::new("/dev/tty.usbmodem-arm", 1_000_000).expect("valid port");
    let robot = RobotSpec::new("follower".to_owned(), port, MotorSpec::so101_arm())
        .expect("valid robot definition");
    let goals = JointGoals::new(BTreeMap::from([(
        "gripper".to_owned(),
        NormalizedPosition::try_from(0.25).expect("normalized position"),
    )]))
    .expect("nonempty goals");
    let trajectory =
        ReplayTrajectory::single_pose(goals.as_ref().clone(), 60.0).expect("valid trajectory");
    let replay = ReplayRequest::timeline(
        "follower".to_owned(),
        "episode_000001".to_owned(),
        trajectory,
    )
    .expect("valid replay")
    .starting_at(0, 0.0)
    .expect("valid start");

    assert!(format!("{robot:?}").contains("follower"));
    assert!(format!("{replay:?}").contains("episode_000001"));
}

#[test]
fn malformed_values_are_unrepresentable_at_the_actor_boundary() {
    assert!(PortSettings::new("", 1_000_000).is_err());
    assert!(PortSettings::new("/dev/tty.arm", 0).is_err());
    assert!(
        RobotSpec::new(
            "bad robot".to_owned(),
            PortSettings::new("/dev/tty.arm", 1_000_000).unwrap(),
            MotorSpec::so101_arm(),
        )
        .is_err()
    );
    assert!(JointGoals::try_from(BTreeMap::new()).is_err());
    assert!(JointGoals::try_from(BTreeMap::from([("gripper".to_owned(), f64::NAN,)])).is_err());
    assert!(JointGoals::try_from(BTreeMap::from([("gripper".to_owned(), 1.1,)])).is_err());
    assert!(ReplayTrajectory::new(Vec::new(), 60.0).is_err());
    assert!(
        ReplayTrajectory::single_pose(BTreeMap::from([("gripper".to_owned(), 0.0)]), 0.0,).is_err()
    );
    assert!(
        ReplayTrajectory::new(
            vec![
                BTreeMap::from([("gripper".to_owned(), 0.0)]),
                BTreeMap::from([("wrist_roll".to_owned(), 0.0)]),
            ],
            60.0,
        )
        .is_err()
    );

    let trajectory =
        ReplayTrajectory::single_pose(BTreeMap::from([("gripper".to_owned(), 0.0)]), 60.0).unwrap();
    let replay = ReplayRequest::timeline(
        "follower".to_owned(),
        "episode_000001".to_owned(),
        trajectory,
    )
    .unwrap();
    assert!(replay.starting_at(1, 0.0).is_err());
}

#[test]
fn boundary_values_support_the_traits_the_app_needs() {
    fn clone_debug_eq<T: Clone + Debug + PartialEq>() {}
    fn copy_debug_eq<T: Copy + Debug + Eq>() {}
    fn error<T: std::error::Error + Send + Sync + 'static>() {}

    clone_debug_eq::<HardwareState>();
    copy_debug_eq::<ControlSettings>();
    error::<HardwareInputError>();

    let normalized = NormalizedPosition::try_from(-0.5).unwrap();
    assert_eq!(f64::from(normalized), -0.5);
    assert!(
        ControlSettings::default()
            .with_motion_limits(20, 40)
            .is_ok()
    );
    assert!(
        ControlSettings::default()
            .with_motion_limits(0, 40)
            .is_err()
    );

    assert_eq!(std::mem::size_of::<CameraRuntime>(), 0);
}
