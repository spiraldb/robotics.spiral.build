//! Keep product composition in activities and rendering contracts independent of navigation.

use std::{fs, path::PathBuf};

#[test]
fn shared_visualizations_do_not_dispatch_on_activity() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for component in [
        "panels/camera.rs",
        "panels/camera_panel.rs",
        "panels/scene.rs",
        "panels/timeline/panel.rs",
        "panels/timeline/playback.rs",
        "panels/timeline/state.rs",
        "panels/timeline/subject.rs",
    ] {
        let text = fs::read_to_string(source.join(component)).unwrap();
        assert!(
            !text.contains("Activity::") && !text.contains("shell::activity"),
            "{component} must consume explicit inputs instead of activity identity"
        );
    }
    assert!(
        !source.join("shell/workspace.rs").exists(),
        "screen assembly belongs to activities"
    );
}

#[test]
fn rollout_history_is_owned_above_all_of_its_panels() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/activities/rollout");
    let screen = fs::read_to_string(source.join("screen.rs")).unwrap();
    let workbench = fs::read_to_string(source.join("workspace.rs")).unwrap();
    assert!(
        screen.contains("TrainingHistory::use_history"),
        "camera selection and checkpoint controls need the same route-scoped history"
    );
    assert!(
        !workbench.contains("TrainingHistory::use_history"),
        "a workbench-scoped provider is invisible to its sibling panels"
    );
}
