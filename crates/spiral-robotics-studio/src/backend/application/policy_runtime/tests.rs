use super::*;

fn data() -> PolicyDataContract {
    PolicyDataContract {
        value_space: "centered_normalized".to_owned(),
        robot_type: "so101_follower".to_owned(),
        joint_names: vec!["shoulder".to_owned(), "elbow".to_owned()],
        camera_streams: vec!["follower.camera.wrist".to_owned()],
        sample_hz: 60.0,
        image_layout: "chw".to_owned(),
        image_normalization: "imagenet".to_owned(),
        frame_alignment: "fixed_fps".to_owned(),
        schema_version: 6,
        supported_schema_version: 6,
    }
}

fn model() -> PolicyModelContract {
    PolicyModelContract {
        state_dimensions: 2,
        action_dimensions: 2,
        action_chunk_size: 100,
        recommended_action_steps: 25,
        image_streams: vec!["follower.camera.wrist".to_owned()],
    }
}

fn target() -> TargetRobotContract {
    TargetRobotContract {
        id: "follower".to_owned(),
        robot_type: Some("so101_follower".to_owned()),
        joint_names: vec!["elbow".to_owned(), "shoulder".to_owned()],
        camera_bindings: vec![CameraBindingContract {
            role: "wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
        }],
        connected: true,
        calibrated: true,
        problem: None,
    }
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
    assert!(validate_artifact_identity(&expected, "run-1", 10).is_ok());

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
        assert!(validate_artifact_identity(&invalid, "run-1", 10).is_err());
    }
}

#[test]
fn valid_contract_resolves_camera_roles_and_preserves_checkpoint_joint_order() {
    let result = validate_rollout_contract(data(), &model(), &target()).unwrap();
    assert_eq!(result.joint_names, ["shoulder", "elbow"]);
    assert_eq!(result.action_steps, 25);
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
fn artifact_data_must_have_unique_names_and_a_safe_rate() {
    for contract in [
        PolicyDataContract {
            joint_names: vec![],
            ..data()
        },
        PolicyDataContract {
            joint_names: vec!["shoulder".to_owned(), "shoulder".to_owned()],
            ..data()
        },
        PolicyDataContract {
            camera_streams: vec!["wrist".to_owned(), "wrist".to_owned()],
            ..data()
        },
        PolicyDataContract {
            sample_hz: f64::NAN,
            ..data()
        },
        PolicyDataContract {
            sample_hz: 101.0,
            ..data()
        },
        PolicyDataContract {
            value_space: "raw_ticks".to_owned(),
            ..data()
        },
        PolicyDataContract {
            image_layout: "hwc".to_owned(),
            ..data()
        },
        PolicyDataContract {
            schema_version: 5,
            ..data()
        },
    ] {
        assert!(validate_rollout_contract(contract, &model(), &target()).is_err());
    }
}

#[test]
fn action_output_must_have_exact_shape_and_finite_values() {
    let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
    assert_eq!(
        prepare_action_goals(&joints, &[1.0, -1.0]).unwrap().goals,
        BTreeMap::from([("elbow".to_owned(), -1.0), ("shoulder".to_owned(), 1.0)])
    );
    assert!(prepare_action_goals(&joints, &[0.25]).is_err());
    assert!(prepare_action_goals(&joints, &[0.25, 0.5, 0.75]).is_err());
    assert!(prepare_action_goals(&joints, &[0.25, f32::NAN]).is_err());
}

#[test]
fn finite_action_overshoots_are_clamped_and_reported() {
    let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
    let prepared = prepare_action_goals(&joints, &[1.049_266_2, -1.005_372_2]).unwrap();

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
    let joints = vec!["shoulder".to_owned(), "elbow".to_owned()];
    let observations = vec![
        JointObservation {
            joint: "elbow".to_owned(),
            normalized: Some(-0.5),
        },
        JointObservation {
            joint: "shoulder".to_owned(),
            normalized: Some(0.25),
        },
    ];
    assert_eq!(
        prepare_observation_state(&joints, &observations).unwrap(),
        [0.25, -0.5]
    );

    for invalid in [
        vec![observations[0].clone()],
        vec![
            observations[0].clone(),
            JointObservation {
                joint: "shoulder".to_owned(),
                normalized: None,
            },
        ],
        vec![
            observations[0].clone(),
            JointObservation {
                joint: "shoulder".to_owned(),
                normalized: Some(f64::NAN),
            },
        ],
        vec![observations[0].clone(), observations[0].clone()],
    ] {
        assert!(prepare_observation_state(&joints, &invalid).is_err());
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
        assert!(validate_rollout_contract(data(), &variant, &target()).is_err());
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
            CameraBindingContract {
                role: "wrist".to_owned(),
                camera_id: "camera-1".to_owned(),
            },
            CameraBindingContract {
                role: "wrist".to_owned(),
                camera_id: "camera-2".to_owned(),
            },
        ],
        ..target()
    });

    for variant in variants {
        assert!(validate_rollout_contract(data(), &model(), &variant).is_err());
    }
}

#[test]
fn observations_must_exist_and_be_within_the_shared_freshness_window() {
    let maximum = Duration::from_millis(350);
    assert!(
        require_fresh_observations(
            &[ObservationFreshness {
                source: "wrist".to_owned(),
                age: Some(maximum),
            }],
            maximum,
        )
        .is_ok()
    );
    assert!(
        require_fresh_observations(
            &[ObservationFreshness {
                source: "wrist".to_owned(),
                age: None,
            }],
            maximum,
        )
        .is_err()
    );
    assert!(
        require_fresh_observations(
            &[ObservationFreshness {
                source: "wrist".to_owned(),
                age: Some(maximum + Duration::from_nanos(1)),
            }],
            maximum,
        )
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
        assess_policy_ownership(&owner, None, "follower", "session-1").unwrap(),
        PolicyOwnership::Retained
    );
    assert!(matches!(
        assess_policy_ownership(
            &ControlOwner::Other("Idle".to_owned()),
            None,
            "follower",
            "session-1"
        )
        .unwrap(),
        PolicyOwnership::Replaced(_)
    ));
    assert!(matches!(
        assess_policy_ownership(
            &ControlOwner::Other("Idle".to_owned()),
            Some("motor bus stopped responding"),
            "follower",
            "session-1"
        ),
        Err(ApplicationError::Conflict(_))
    ));
}
