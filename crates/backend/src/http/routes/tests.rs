use crate::OperationError;
use crate::project_session::ProjectReadiness;
use crate::recording::video_http::ByteRange;
use crate::robotics::cameras::Frame;
use crate::robots::commands::CameraConfigurationObject;
use crate::robots::service::CameraAssignment;
use bytes::Bytes;
use serde_json::Map;
use spiral_robotics_common::robots::CameraStatus;
use spiral_robotics_common::status::BackendReadinessState;

#[test]
fn bootstrap_lifecycle_controls_catalog_admission() {
    let state = ProjectReadiness::new();
    assert_eq!(
        state.current().state(),
        BackendReadinessState::Bootstrapping
    );
    assert!(state.catalog_unavailable().is_some());
    assert!(state.storage_unavailable().is_some());

    state.ready();
    assert_eq!(state.current().state(), BackendReadinessState::Ready);
    assert!(state.current().storage_available());
    assert!(state.catalog_unavailable().is_none());

    state.degraded(false, "authentication expired");
    assert_eq!(state.current().state(), BackendReadinessState::Degraded);
    assert_eq!(
        state
            .catalog_unavailable()
            .as_ref()
            .map(OperationError::message),
        Some("authentication expired")
    );
}

fn status(id: &str) -> CameraStatus {
    CameraStatus::new(
        id,
        "Test camera",
        spiral_robotics_common::robots::CameraStreamFormat::new(1280, 720, 30),
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
fn readiness_preserves_compatibility_failures_for_all_project_reads() {
    use spiral_robotics_common::command_failure::CommandFailureCode;
    let readiness = ProjectReadiness::new();
    readiness.failed(
        false,
        &OperationError::IncompatibleWorkflow("Saved format unsupported".into()),
    );
    assert_eq!(
        readiness.current().failure_code(),
        Some(CommandFailureCode::IncompatibleWorkflow)
    );
    assert_eq!(
        readiness.catalog_unavailable().unwrap().code(),
        CommandFailureCode::IncompatibleWorkflow
    );
    assert_eq!(
        readiness.storage_unavailable().unwrap().code(),
        CommandFailureCode::IncompatibleWorkflow
    );
    readiness.failed(false, &OperationError::External("Connection lost".into()));
    assert_eq!(
        readiness.storage_unavailable().unwrap().code(),
        CommandFailureCode::ProjectUnavailable
    );
}
