use super::*;

fn data() -> PolicyDataContract {
    PolicyDataContract::new(
        PolicyObservationContract::new(
            "centered_normalized",
            "so101_follower",
            vec!["shoulder".to_owned(), "elbow".to_owned()],
        )
        .with_capture(vec!["follower.camera.wrist".to_owned()], 60.0),
        PolicyImageContract::new("chw", "imagenet", "fixed_fps"),
        PolicySchemaContract::new(6, 6),
    )
}

fn model() -> PolicyModelContract {
    PolicyModelContract::new(2, 2, vec!["follower.camera.wrist".to_owned()])
        .with_action_shape(100, 25)
}

fn target() -> TargetRobotContract {
    TargetRobotContract::new(
        "follower",
        Some("so101_follower".to_owned()),
        vec!["elbow".to_owned(), "shoulder".to_owned()],
    )
    .with_cameras(vec![CameraBindingContract::new("wrist", "camera-1")])
    .with_availability(true, true)
}

#[test]
fn artifact_identity_binds_format_purpose_namespace_run_and_step() {
    let expected = ArtifactIdentity {
        format: ACT_ARTIFACT_FORMAT.to_owned(),
        purpose: "inference".to_owned(),
        run_id: "run-1".to_owned(),
        step: 10,
        tensor_namespace: ACT_TENSOR_NAMESPACE.to_owned(),
    };
    assert!(expected.validate_for("run-1", 10).is_ok());

    for invalid in [
        ArtifactIdentity {
            format: "foreign".to_owned(),
            ..expected.clone()
        },
        ArtifactIdentity {
            purpose: "training".to_owned(),
            ..expected.clone()
        },
        ArtifactIdentity {
            run_id: "run-2".to_owned(),
            ..expected.clone()
        },
        ArtifactIdentity {
            step: 11,
            ..expected.clone()
        },
        ArtifactIdentity {
            tensor_namespace: "foreign".to_owned(),
            ..expected.clone()
        },
    ] {
        assert!(invalid.validate_for("run-1", 10).is_err());
    }
}

#[test]
fn valid_contract_resolves_camera_roles_and_preserves_checkpoint_joint_order() {
    let result = data().validated_with(&model(), &target()).unwrap();
    assert_eq!(result.joint_names, ["shoulder", "elbow"]);
    assert_eq!(result.action_steps, MAX_ROLLOUT_ACTION_STEPS);
    assert_eq!(result.sample_hz, 60.0);
    assert_eq!(result.sample_period, Duration::from_secs_f64(1.0 / 60.0));
    assert_eq!(
        result.camera_bindings,
        [ResolvedCameraBinding {
            stream: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
        }]
    );
}

#[test]
fn rollout_preserves_a_checkpoint_prefix_shorter_than_the_replanning_cap() {
    let short_model = PolicyModelContract::new(2, 2, vec!["follower.camera.wrist".to_owned()])
        .with_action_shape(10, 10);

    let result = data().validated_with(&short_model, &target()).unwrap();

    assert_eq!(result.action_steps, 10);
}

#[test]
fn artifact_data_must_have_unique_names_and_a_safe_rate() {
    let mut empty_joints = data();
    empty_joints.observations.joint_names.clear();
    let mut duplicate_joints = data();
    duplicate_joints.observations.joint_names = vec!["shoulder".to_owned(), "shoulder".to_owned()];
    let mut duplicate_cameras = data();
    duplicate_cameras.observations.camera_streams = vec!["wrist".to_owned(), "wrist".to_owned()];
    let mut nan_rate = data();
    nan_rate.observations.sample_hz = f64::NAN;
    let mut excessive_rate = data();
    excessive_rate.observations.sample_hz = 101.0;
    let mut raw_ticks = data();
    raw_ticks.observations.value_space = "raw_ticks".to_owned();
    let mut wrong_layout = data();
    wrong_layout.images.layout = "hwc".to_owned();
    let mut old_schema = data();
    old_schema.schemas.artifact = 5;
    for contract in [
        empty_joints,
        duplicate_joints,
        duplicate_cameras,
        nan_rate,
        excessive_rate,
        raw_ticks,
        wrong_layout,
        old_schema,
    ] {
        assert!(contract.validated_with(&model(), &target()).is_err());
    }
}

#[test]
fn action_output_must_have_exact_shape_and_finite_values() {
    let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
    assert_eq!(
        PreparedActionGoals::from_action(&joints, &[1.0, -1.0])
            .unwrap()
            .goals,
        BTreeMap::from([("elbow".to_owned(), -1.0), ("shoulder".to_owned(), 1.0)])
    );
    assert!(PreparedActionGoals::from_action(&joints, &[0.25]).is_err());
    assert!(PreparedActionGoals::from_action(&joints, &[0.25, 0.5, 0.75]).is_err());
    assert!(PreparedActionGoals::from_action(&joints, &[0.25, f32::NAN]).is_err());
}

#[test]
fn finite_action_overshoots_are_clamped_and_reported() {
    let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
    let prepared = PreparedActionGoals::from_action(&joints, &[1.049_266_2, -1.005_372_2]).unwrap();

    assert_eq!(
        prepared.goals,
        BTreeMap::from([("elbow".to_owned(), -1.0), ("shoulder".to_owned(), 1.0)])
    );
    assert_eq!(prepared.clamps.len(), 2);
    assert_eq!(prepared.clamps[0].index, 0);
    assert_eq!(prepared.clamps[0].joint, "shoulder");
    assert_eq!(prepared.clamps[0].requested, 1.049_266_2);
    assert_eq!(prepared.clamps[0].applied, 1.0);
    assert_eq!(prepared.clamps[1].index, 1);
    assert_eq!(prepared.clamps[1].joint, "elbow");
    assert_eq!(prepared.clamps[1].requested, -1.005_372_2);
    assert_eq!(prepared.clamps[1].applied, -1.0);
}

#[test]
fn live_joint_state_is_complete_finite_unique_and_checkpoint_ordered() {
    let joints = ["shoulder".to_owned(), "elbow".to_owned()];
    let observations = [
        JointObservation::new("elbow", Some(-0.5)),
        JointObservation::new("shoulder", Some(0.25)),
    ];
    assert_eq!(observations.ordered_state(&joints).unwrap(), [0.25, -0.5]);

    for invalid in [
        vec![observations[0].clone()],
        vec![
            observations[0].clone(),
            JointObservation::new("shoulder", None),
        ],
        vec![
            observations[0].clone(),
            JointObservation::new("shoulder", Some(f64::NAN)),
        ],
        vec![observations[0].clone(), observations[0].clone()],
    ] {
        assert!(invalid.ordered_state(&joints).is_err());
    }
}

#[test]
fn loaded_model_must_match_dimensions_camera_order_and_chunk_contract() {
    let variants = [
        PolicyModelContract {
            state_dimensions: 1,
            ..model()
        },
        PolicyModelContract {
            action_dimensions: 3,
            ..model()
        },
        PolicyModelContract {
            recommended_action_steps: 0,
            ..model()
        },
        PolicyModelContract {
            recommended_action_steps: 101,
            ..model()
        },
        PolicyModelContract {
            image_streams: vec!["other".to_owned()],
            ..model()
        },
    ];
    for variant in variants {
        assert!(data().validated_with(&variant, &target()).is_err());
    }
}

#[test]
fn target_must_be_healthy_exactly_typed_and_joint_compatible() {
    let mut variants = Vec::new();
    variants.push(TargetRobotContract {
        connected: false,
        ..target()
    });
    variants.push(TargetRobotContract {
        calibrated: false,
        ..target()
    });
    variants.push(TargetRobotContract {
        problem: Some("invalid calibration".to_owned()),
        ..target()
    });
    variants.push(TargetRobotContract {
        robot_type: Some("so101_leader".to_owned()),
        ..target()
    });
    variants.push(TargetRobotContract {
        joint_names: vec!["shoulder".to_owned()],
        ..target()
    });
    variants.push(TargetRobotContract {
        camera_bindings: vec![],
        ..target()
    });
    variants.push(TargetRobotContract {
        camera_bindings: vec![
            CameraBindingContract::new("wrist", "camera-1"),
            CameraBindingContract::new("wrist", "camera-2"),
        ],
        ..target()
    });

    for variant in variants {
        assert!(data().validated_with(&model(), &variant).is_err());
    }
}

#[test]
fn observations_must_exist_and_be_within_the_shared_freshness_window() {
    let maximum = Duration::from_millis(350);
    assert!(
        [ObservationFreshness {
            source: "wrist".to_owned(),
            age: Some(maximum),
        }]
        .require_fresh(maximum)
        .is_ok()
    );
    assert!(
        [ObservationFreshness {
            source: "wrist".to_owned(),
            age: None,
        }]
        .require_fresh(maximum)
        .is_err()
    );
    assert!(
        [ObservationFreshness {
            source: "wrist".to_owned(),
            age: Some(maximum + Duration::from_nanos(1)),
        }]
        .require_fresh(maximum)
        .is_err()
    );
}

#[test]
fn exact_policy_session_owns_control_and_replacements_are_classified() {
    let owner = ControlOwner::Policy {
        robot: "follower".to_owned(),
        session: "session-1".to_owned(),
    };
    assert_eq!(
        owner
            .assess(None, ExpectedPolicyOwner::new("follower", "session-1"))
            .unwrap(),
        PolicyOwnership::Retained
    );
    assert!(matches!(
        ControlOwner::Other("Idle".to_owned())
            .assess(None, ExpectedPolicyOwner::new("follower", "session-1"))
            .unwrap(),
        PolicyOwnership::Replaced(_)
    ));
    assert!(matches!(
        ControlOwner::Other("Idle".to_owned()).assess(
            Some("motor bus stopped responding"),
            ExpectedPolicyOwner::new("follower", "session-1"),
        ),
        Err(OperationError::Conflict(_))
    ));
}
