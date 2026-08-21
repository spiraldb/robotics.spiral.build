use super::data::{
    application_data_settings, application_query_result, contract_data_settings,
    contract_query_result,
};
use super::devices::{build_camera_object, multipart_part};
use super::recordings::{ByteRange, parse_byte_range};
use super::training_rollout::{
    application_rollout_snapshot, application_training_run, contract_rollout_snapshot,
    contract_training_run,
};
use super::*;

#[test]
fn bootstrap_lifecycle_controls_catalog_admission() {
    let state = BootstrapState::new();
    assert_eq!(state.current().state, BackendReadinessState::Bootstrapping);
    assert!(state.catalog_unavailable().is_some());

    state.ready();
    assert_eq!(state.current().state, BackendReadinessState::Ready);
    assert!(state.catalog_unavailable().is_none());

    state.degraded(false, "authentication expired");
    assert_eq!(state.current().state, BackendReadinessState::Degraded);
    assert_eq!(
        state.catalog_unavailable().as_deref(),
        Some("authentication expired")
    );
}

#[test]
fn temporary_unavailability_includes_a_retry_hint() {
    let response = Failure::unavailable("still warming").into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::RETRY_AFTER),
        Some(&axum::http::HeaderValue::from_static("1"))
    );
}

#[test]
fn feature_routers_preserve_every_public_path_exactly_once() {
    let sources = [
        include_str!("../routes.rs"),
        include_str!("data.rs"),
        include_str!("devices.rs"),
        include_str!("recordings.rs"),
        include_str!("training_rollout.rs"),
    ]
    .join("\n");
    let paths = [
        "/state",
        "/events",
        "/ports",
        "/ports/identify",
        "/cameras",
        "/cameras/refresh",
        "/cameras/stream.mjpeg",
        "/cameras/snapshot.jpg",
        "/robots",
        "/robots/import",
        "/robots/{id}",
        "/robots/{id}/configuration",
        "/robots/{id}/connect",
        "/robots/{id}/disconnect",
        "/robots/{id}/hold",
        "/robots/{id}/release",
        "/robots/{id}/goal",
        "/robots/{id}/calibration/start",
        "/robots/{id}/calibration/finish",
        "/robots/{id}/calibration/cancel",
        "/robots/{id}/calibration/reapply",
        "/mode",
        "/episodes",
        "/episodes/record",
        "/episodes/record/pause",
        "/episodes/record/stop",
        "/episodes/replay",
        "/episodes/replay/pause",
        "/episodes/export",
        "/episodes/import",
        "/episodes/{id}",
        "/episodes/{id}/inspection",
        "/episodes/{id}/judge",
        "/episodes/{id}/trace",
        "/episodes/{id}/timeline",
        "/episodes/{id}/timeline/traces",
        "/episodes/{id}/timeline/sample",
        "/episodes/{id}/seek",
        "/episodes/{id}/videos/{stream}",
        "/settings/data",
        "/settings/data/test/spiral",
        "/spiral/query",
        "/spiral/replay",
        "/spiral/video.mp4",
        "/project/export",
        "/project/import",
        "/training/runs",
        "/training/runs/{id}",
        "/training/runs/{id}/stop",
        "/rollout",
        "/rollout/stop",
    ];
    for path in paths {
        assert_eq!(
            sources.matches(&format!("\"{path}\"")).count(),
            1,
            "route path {path:?} must be registered exactly once"
        );
    }
}

#[test]
fn imports_distinguish_unavailable_storage_from_invalid_files() {
    let unavailable = Failure::import(
        "SpiralDB is unavailable at http://127.0.0.1:4279. Start the service or update Settings → SpiralDB, then try again.",
    );
    assert_eq!(unavailable.0, StatusCode::SERVICE_UNAVAILABLE);

    let invalid = Failure::import("episode bundle has the wrong format");
    assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
}

fn status(id: &str) -> CameraStatus {
    CameraStatus {
        id: id.to_owned(),
        name: "Test camera".to_owned(),
        width: 1280,
        height: 720,
        fps: 30,
        streaming: false,
        frames: 0,
        error: None,
    }
}

fn binding(name: &str, id: &str) -> CameraAssignment {
    CameraAssignment {
        role: name.to_owned(),
        device_id: id.to_owned(),
    }
}

#[test]
fn camera_assignments_keep_unknown_fields_and_refresh_resolution() {
    let existing = Map::from_iter([(
        "wrist".to_owned(),
        serde_json::json!({
            "type": "opencv",
            "id": "/dev/video9",
            "width": 320,
            "rotation": 180
        }),
    )]);

    let cameras = build_camera_object(
        &existing,
        &[binding("wrist", "/dev/video0")],
        &[status("/dev/video0")],
    );

    assert_eq!(cameras["wrist"]["id"], "/dev/video0");
    assert_eq!(cameras["wrist"]["width"], 1280);
    assert_eq!(cameras["wrist"]["height"], 720);
    assert_eq!(cameras["wrist"]["fps"], 30);
    assert_eq!(cameras["wrist"]["rotation"], 180, "unknown fields survive");
}

#[test]
fn a_camera_mount_survives_being_reassigned() {
    // Where a camera is bolted is written by hand into the robot file; the
    // studio has no control that sets it, so every camera edit the studio
    // does make has to leave it exactly where it was.
    let existing = Map::from_iter([(
        "wrist".to_owned(),
        serde_json::json!({
            "id": "/dev/video0",
            "mount": { "frame": "gripper_link", "xyz": [0.02, 0.0, 0.04] }
        }),
    )]);

    let cameras = build_camera_object(&existing, &[binding("wrist", "/dev/video9")], &[]);

    assert_eq!(cameras["wrist"]["id"], "/dev/video9");
    assert_eq!(cameras["wrist"]["mount"]["frame"], "gripper_link");
    assert_eq!(cameras["wrist"]["mount"]["xyz"][2], 0.04);
}

#[test]
fn an_unknown_device_still_records_the_assignment() {
    let cameras = build_camera_object(&Map::new(), &[binding("top", "/dev/video7")], &[]);

    assert_eq!(cameras["top"]["id"], "/dev/video7");
    assert_eq!(cameras["top"]["type"], "opencv");
    assert!(cameras["top"].get("width").is_none());
}

#[test]
fn removing_a_camera_drops_it_from_the_stored_object() {
    let existing = Map::from_iter([
        ("wrist".to_owned(), serde_json::json!({"id": "/dev/video0"})),
        ("top".to_owned(), serde_json::json!({"id": "/dev/video1"})),
    ]);

    let cameras = build_camera_object(&existing, &[binding("top", "/dev/video1")], &[]);

    assert_eq!(cameras.len(), 1);
    assert!(cameras.contains_key("top"));
}

#[test]
fn multipart_parts_carry_an_accurate_length() {
    let frame = Frame {
        bytes: Bytes::from_static(&[0xff, 0xd8, 0x00, 0xff, 0xd9]),
        sequence: 1,
        captured_at: std::time::Instant::now(),
    };

    let part = multipart_part(&frame);
    let text = String::from_utf8_lossy(&part);

    assert!(text.starts_with("--spiral-robotics-frame\r\nContent-Type: image/jpeg\r\n"));
    assert!(text.contains("Content-Length: 5\r\n\r\n"));
    assert!(part.ends_with(b"\r\n"));
    assert_eq!(part.len(), 5 + part.len() - 5);
}

#[test]
fn spiral_video_ranges_cover_browser_seek_forms() {
    assert_eq!(
        parse_byte_range("bytes=10-19", 100),
        Some(ByteRange { start: 10, end: 19 })
    );
    assert_eq!(
        parse_byte_range("bytes=90-", 100),
        Some(ByteRange { start: 90, end: 99 })
    );
    assert_eq!(
        parse_byte_range("bytes=-10", 100),
        Some(ByteRange { start: 90, end: 99 })
    );
    assert_eq!(
        parse_byte_range("bytes=90-200", 100),
        Some(ByteRange { start: 90, end: 99 })
    );
    assert_eq!(parse_byte_range("bytes=100-", 100), None);
    assert_eq!(parse_byte_range("bytes=20-10", 100), None);
    assert_eq!(parse_byte_range("bytes=0-1,4-5", 100), None);
    assert_eq!(parse_byte_range("items=0-1", 100), None);
    assert_eq!(parse_byte_range("bytes=0-1", 0), None);
}

#[test]
fn episode_errors_map_to_the_right_status_codes() {
    let cases = [
        (
            EpisodeError::InvalidId("../x".to_owned()),
            StatusCode::BAD_REQUEST,
        ),
        (
            EpisodeError::NotFound("episode_000001".to_owned()),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(Failure::from(error).0, expected);
    }
}

#[test]
fn workspace_errors_map_to_the_right_status_codes() {
    let cases = [
        (
            WorkspaceError::InvalidId("../x".to_owned()),
            StatusCode::BAD_REQUEST,
        ),
        (
            WorkspaceError::NotFound("arm".to_owned()),
            StatusCode::NOT_FOUND,
        ),
        (
            WorkspaceError::AlreadyExists("arm".to_owned()),
            StatusCode::CONFLICT,
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(Failure::from(error).0, expected);
    }
    // A control-thread refusal is a conflict, not a crash.
    assert_eq!(
        Failure::from("robot is not connected".to_owned()).0,
        StatusCode::CONFLICT
    );
}

#[test]
fn data_presenters_round_trip_transport_values_without_leaking_them_to_policy() {
    let settings = DataSettings {
        spiral: ContractSpiralSettings {
            endpoint: "https://api.spiraldb.dev".to_owned(),
            spfs_endpoint: "https://spfs.spiraldb.dev".to_owned(),
            project_id: "project".to_owned(),
            client_id: String::new(),
            client_secret_configured: false,
            namespace: "dataset".to_owned(),
            endpoint_restart_required: false,
        },
        training: spiral_robotics_contracts::api::TrainingRuntimeSettings::default(),
        video: VideoSettings { enabled: true },
    };
    assert_eq!(
        contract_data_settings(application_data_settings(
            settings.clone(),
            None,
            None,
            None,
            None,
            false,
        )),
        settings
    );

    let pending = application_data_settings(
        settings,
        Some("http://127.0.0.1:4279"),
        Some("http://127.0.0.1:4295"),
        Some("local-project"),
        Some("client-id"),
        true,
    );
    assert_eq!(pending.spiral.endpoint, "http://127.0.0.1:4279");
    assert_eq!(pending.spiral.spfs_endpoint, "http://127.0.0.1:4295");
    assert_eq!(pending.spiral.project_id, "local-project");
    assert!(!pending.spiral.endpoint_restart_required);
    assert_eq!(pending.spiral.client_id, "client-id");
    assert!(pending.spiral.client_secret_configured);

    let query = SpiralQueryResponse {
        columns: vec![spiral_robotics_contracts::api::QueryColumn {
            name: "payload".to_owned(),
            data_type: "json".to_owned(),
            nullable: true,
            semantic_type: Some("spql.test.Payload".to_owned()),
        }],
        rows: vec![serde_json::Map::from_iter([(
            "payload".to_owned(),
            serde_json::json!({"signed": -2, "unsigned": u64::MAX, "nested": [true, null]}),
        )])],
        truncated: true,
        elapsed_ms: 9,
    };
    assert_eq!(
        contract_query_result(application_query_result(query.clone())),
        query
    );
}

#[test]
fn training_and_rollout_presenters_round_trip_semantic_state() {
    let run = TrainingRun {
        id: "run-1".to_owned(),
        config: StartTrainingRequest::default(),
        source: spiral_robotics_contracts::api::TrainingSource {
            dataset_id: "default".to_owned(),
        },
        phase: spiral_robotics_contracts::api::TrainingRunPhase::Running,
        current_step: 4,
        started_ms: 1,
        finished_ms: None,
        message: Some("running".to_owned()),
        metrics: vec![spiral_robotics_contracts::api::TrainingMetricPoint {
            step: 4,
            loader_ms: Some(790.0),
            arrow_encode_ms: Some(24.0),
            upload_ms: Some(2_960.0),
            device_copy_ms: Some(6.0),
            model_ms: Some(149.0),
            ..Default::default()
        }],
        checkpoints: Vec::new(),
        logs: vec!["ready".to_owned()],
    };
    assert_eq!(
        contract_training_run(application_training_run(run.clone())),
        run
    );

    let rollout = RolloutSnapshot {
        phase: spiral_robotics_contracts::api::RolloutPhase::Running,
        session_id: Some("session-1".to_owned()),
        run_id: Some("run-1".to_owned()),
        checkpoint_step: Some(4),
        robot: Some("arm".to_owned()),
        duration_seconds: Some(30),
        started_ms: Some(1),
        finished_ms: None,
        message: None,
        warning: Some("clamped wrist".to_owned()),
        metrics: spiral_robotics_contracts::api::RolloutMetrics {
            inference_count: 2,
            command_count: 1,
            ..Default::default()
        },
    };
    assert_eq!(
        contract_rollout_snapshot(application_rollout_snapshot(rollout.clone())),
        rollout
    );
}
