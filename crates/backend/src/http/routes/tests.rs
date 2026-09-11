use crate::OperationError;
use crate::project_session::ProjectReadiness;
use crate::robotics::cameras::Frame;
use bytes::Bytes;
use spiral_robotics_common::status::BackendReadinessState;

#[test]
fn bootstrap_lifecycle_controls_catalog_admission() {
    let state = ProjectReadiness::new();
    assert_eq!(
        state.current().state(),
        BackendReadinessState::Bootstrapping
    );
    assert!(state.catalog_unavailable().is_some());

    state.ready();
    assert_eq!(state.current().state(), BackendReadinessState::Ready);
    assert!(state.current().catalog_available());
    assert!(state.catalog_unavailable().is_none());

    state.failed(
        false,
        &crate::OperationError::External("authentication expired".to_owned()),
    );
    assert_eq!(state.current().state(), BackendReadinessState::Degraded);
    assert_eq!(
        state
            .catalog_unavailable()
            .as_ref()
            .map(OperationError::message),
        Some("authentication expired")
    );
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
    readiness.failed(false, &OperationError::External("Connection lost".into()));
    assert_eq!(
        readiness.catalog_unavailable().unwrap().code(),
        CommandFailureCode::ProjectUnavailable
    );
}
