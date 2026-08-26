use super::data::DataSettingsOverrides;
use super::devices::CameraConfigurationObject;
use super::recordings::ByteRange;
use super::*;

#[test]
fn bootstrap_lifecycle_controls_catalog_admission() {
    let state = BootstrapState::new();
    assert_eq!(
        state.current().state(),
        BackendReadinessState::Bootstrapping
    );
    assert!(state.catalog_unavailable().is_some());

    state.ready();
    assert_eq!(state.current().state(), BackendReadinessState::Ready);
    assert!(state.catalog_unavailable().is_none());

    state.degraded(false, "authentication expired");
    assert_eq!(state.current().state(), BackendReadinessState::Degraded);
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
        include_str!("workflow.rs"),
    ]
    .join("\n");
    let paths = [
        "/events",
        "/ports",
        "/ports/identify",
        "/cameras/refresh",
        "/cameras/stream.mjpeg",
        "/robots",
        "/robots/import",
        "/robots/{id}",
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
        "/episodes/import",
        "/episodes/{id}",
        "/episodes/{id}/inspection",
        "/episodes/{id}/seek",
        "/episodes/{id}/videos/{stream}",
        "/settings/data",
        "/settings/data/test/spiral",
        "/spiral/query",
        "/project/export",
        "/project/import",
        "/training/runs",
        "/training/runs/{id}",
        "/training/runs/{id}/stop",
        "/rollout",
        "/rollout/stop",
        "/workflow/relations",
        "/workflow",
        "/workflow/command",
        "/workflow/agent",
        "/workflow/agent/command",
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
fn workflow_uses_named_events_on_the_single_reconnecting_stream() {
    let source = include_str!("../routes.rs");
    assert!(source.contains("event(\"workflow_snapshot\")"));
    assert!(source.contains("event(\"workflow_update\")"));
    assert!(source.contains("event(\"workflow_agent\")"));
    assert!(
        source.contains("let snapshot = state.workflow().state();"),
        "Workflow broadcasts are wakeups so an old queued scope cannot regress SSE"
    );
    assert_eq!(source.matches("route(\"/events\"").count(), 1);
}

#[test]
fn workflow_agent_sse_cursor_coalesces_duplicate_wakeups_and_accepts_new_scopes() {
    let mut cursor = AgentCursor {
        scope_id: "scope-a".to_owned(),
        revision: 8,
    };
    assert!(!cursor.advance_to(AgentCursor {
        scope_id: "scope-a".to_owned(),
        revision: 8,
    }));
    assert!(cursor.advance_to(AgentCursor {
        scope_id: "scope-a".to_owned(),
        revision: 9,
    }));
    assert!(cursor.advance_to(AgentCursor {
        scope_id: "scope-b".to_owned(),
        revision: 1,
    }));
    assert_eq!(cursor.scope_id, "scope-b");
    assert_eq!(cursor.revision, 1);
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
    CameraStatus::new(
        id,
        "Test camera",
        spiral_robotics_api::robots::CameraStreamFormat::new(1280, 720, 30),
    )
}

fn binding(name: &str, id: &str) -> CameraAssignment {
    CameraAssignment::new(name, id)
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

    let cameras = existing
        .with_camera_assignments(&[binding("wrist", "/dev/video0")], &[status("/dev/video0")]);

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

    let cameras = existing.with_camera_assignments(&[binding("wrist", "/dev/video9")], &[]);

    assert_eq!(cameras["wrist"]["id"], "/dev/video9");
    assert_eq!(cameras["wrist"]["mount"]["frame"], "gripper_link");
    assert_eq!(cameras["wrist"]["mount"]["xyz"][2], 0.04);
}

#[test]
fn an_unknown_device_still_records_the_assignment() {
    let cameras = Map::new().with_camera_assignments(&[binding("top", "/dev/video7")], &[]);

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

    let cameras = existing.with_camera_assignments(&[binding("top", "/dev/video1")], &[]);

    assert_eq!(cameras.len(), 1);
    assert!(cameras.contains_key("top"));
}

#[test]
fn multipart_parts_carry_an_accurate_length() {
    let frame = Frame::new(
        Bytes::from_static(&[0xff, 0xd8, 0x00, 0xff, 0xd9]),
        1,
        std::time::Instant::now(),
    );

    let part = frame.multipart_part();
    let text = String::from_utf8_lossy(&part);

    assert!(text.starts_with("--spiral-robotics-frame\r\nContent-Type: image/jpeg\r\n"));
    assert!(text.contains("Content-Length: 5\r\n\r\n"));
    assert!(part.ends_with(b"\r\n"));
    assert_eq!(part.len(), 5 + part.len() - 5);
}

#[test]
fn spiral_video_ranges_cover_browser_seek_forms() {
    assert_eq!(
        ByteRange::parse("bytes=10-19", 100),
        Some(ByteRange::new(10, 19))
    );
    assert_eq!(
        ByteRange::parse("bytes=90-", 100),
        Some(ByteRange::new(90, 99))
    );
    assert_eq!(
        ByteRange::parse("bytes=-10", 100),
        Some(ByteRange::new(90, 99))
    );
    assert_eq!(
        ByteRange::parse("bytes=90-200", 100),
        Some(ByteRange::new(90, 99))
    );
    assert_eq!(ByteRange::parse("bytes=100-", 100), None);
    assert_eq!(ByteRange::parse("bytes=20-10", 100), None);
    assert_eq!(ByteRange::parse("bytes=0-1,4-5", 100), None);
    assert_eq!(ByteRange::parse("items=0-1", 100), None);
    assert_eq!(ByteRange::parse("bytes=0-1", 0), None);
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
    let settings = DataSettings::new(
        ContractSpiralSettings::new(
            "https://api.spiraldb.dev",
            "https://spfs.spiraldb.dev",
            "project",
        )
        .with_namespace("dataset"),
        spiral_robotics_api::settings::TrainingRuntimeSettings::default(),
        VideoSettings::new(true),
    );
    assert_eq!(
        DataSettings::from(DataSettingsOverrides::new(None, None).apply(settings.clone())),
        settings
    );

    let pending =
        DataSettingsOverrides::new(Some("http://127.0.0.1:4279"), Some("http://127.0.0.1:4295"))
            .with_identity(Some("local-project"), Some("client-id"))
            .with_credentials(true)
            .apply(settings);
    assert_eq!(pending.spiral().endpoint(), "http://127.0.0.1:4279");
    assert_eq!(pending.spiral().spfs_endpoint(), "http://127.0.0.1:4295");
    assert_eq!(pending.spiral().project_id(), "local-project");
    assert!(!pending.spiral().endpoint_restart_required());
    assert_eq!(pending.spiral().client_id(), "client-id");
    assert!(pending.spiral().client_secret_configured());

    let query = SpiralQueryResponse::new(
        vec![
            spiral_robotics_api::relations::QueryColumn::new("payload", "json", true)
                .with_semantic_type(Some("spql.test.Payload".to_owned())),
        ],
        vec![serde_json::Map::from_iter([(
            "payload".to_owned(),
            serde_json::json!({"signed": -2, "unsigned": u64::MAX, "nested": [true, null]}),
        )])],
        true,
    )
    .with_elapsed_ms(9);
    assert_eq!(
        SpiralQueryResponse::from(CapabilityQueryResult::from(query.clone())),
        query
    );
}
