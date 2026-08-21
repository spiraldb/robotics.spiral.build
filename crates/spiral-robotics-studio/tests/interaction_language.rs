use std::fs;
use std::path::{Path, PathBuf};

fn files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
        {
            let path = entry.expect("directory entry should be readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|value| value == extension) {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn product_surfaces_cannot_bypass_the_interaction_language() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = studio.parent().expect("Studio should live in crates/");
    let roots = [
        studio.join("src/frontend"),
        crates.join("spiral-robotics-panels/src"),
    ];
    let forbidden = [
        ("button {", "raw Dioxus button"),
        ("r#type: \"file\"", "raw file picker"),
        ("confirm_with_message", "native browser confirmation"),
        ("window.confirm", "native browser confirmation"),
        (
            "variant: ActionVariant::Quiet",
            "an explicit default action variant",
        ),
    ];

    let mut violations = Vec::new();
    for root in roots {
        for path in files_with_extension(&root, "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
            for (line_index, line) in source.lines().enumerate() {
                if line.contains("fn ") && line.contains("Button(") {
                    violations.push(format!(
                        "{}:{} names a component after its presentation instead of its action",
                        path.display(),
                        line_index + 1,
                    ));
                }
                for (pattern, description) in forbidden {
                    if line.contains(pattern) {
                        violations.push(format!(
                            "{}:{} uses {description}",
                            path.display(),
                            line_index + 1,
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Use spiral-robotics-ui primitives instead:\n{}",
        violations.join("\n"),
    );
}

#[test]
fn legacy_action_styles_cannot_return() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = studio.parent().expect("Studio should live in crates/");
    let styles = [
        studio.join("tailwind.css"),
        crates.join("spiral-robotics-panels/assets/panels.css"),
    ];
    let forbidden = [
        ".studio-button",
        ".icon-button",
        ".btn {",
        ".btn-primary",
        ".btn-danger",
        ".inline-safety-confirm",
        "-button",
    ];

    let mut violations = Vec::new();
    for path in styles {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{} contains {pattern}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Legacy action styling bypasses the shared state system:\n{}",
        violations.join("\n"),
    );
}

#[test]
fn status_bar_keeps_messages_and_live_rates_in_stable_lanes() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shell = fs::read_to_string(studio.join("src/frontend/app/shell.rs"))
        .expect("status-bar source should be readable");
    let styles = fs::read_to_string(studio.join("tailwind.css"))
        .expect("status-bar styles should be readable");

    assert!(
        shell.contains("status-bar-item status-bar-rate"),
        "the changing loop rate needs its reserved measurement class",
    );
    assert!(
        styles.contains("grid-template-columns: minmax(0, 1fr) minmax(0, 32rem) max-content"),
        "the status message must own a lane between context and operational state",
    );
    for declaration in [
        ".status-bar-rate",
        "@apply justify-end font-mono tabular-nums",
        "min-width: 10ch",
        ".status-bar-message",
        "grid-column: 2",
    ] {
        assert!(
            styles.contains(declaration),
            "status-bar stability is missing {declaration:?}",
        );
    }
}

#[test]
fn studio_is_a_strict_desktop_pointer_application() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = studio.parent().expect("Studio should live in crates/");
    let studio_styles =
        fs::read_to_string(studio.join("tailwind.css")).expect("Studio styles should be readable");
    let ui_styles = fs::read_to_string(crates.join("spiral-robotics-ui/assets/ui.css"))
        .expect("shared UI styles should be readable");
    let panel_styles = fs::read_to_string(crates.join("spiral-robotics-panels/assets/panels.css"))
        .expect("panel styles should be readable");
    let panel_workspace =
        fs::read_to_string(crates.join("spiral-robotics-panels/src/workspace.rs"))
            .expect("panel workspace source should be readable");
    let viewport_gestures =
        fs::read_to_string(crates.join("spiral-robotics-viewport/src/gesture.rs"))
            .expect("viewport gesture source should be readable");
    let scene = fs::read_to_string(studio.join("src/frontend/panels/scene.rs"))
        .expect("scene source should be readable");

    assert!(
        studio_styles.contains(".studio-shell-v2 {\n    min-width: 64rem;"),
        "the supported Studio viewport must begin at 1024 CSS pixels",
    );
    for (source, pattern) in [
        (&studio_styles, "@media (max-width:"),
        (&ui_styles, "@media (max-width:"),
        (&ui_styles, "pointer: coarse"),
        (&ui_styles, "pointer: fine"),
        (&ui_styles, "target-touch"),
        (&panel_styles, "@media (max-width:"),
        (&panel_styles, "touch-action"),
        (&panel_workspace, "rp-mobile"),
        (&panel_workspace, "mobile_panel"),
        (&scene, "pointer_type() == \"touch\""),
        (&scene, "Pinch"),
        (&viewport_gestures, "Pinch"),
    ] {
        assert!(
            !source.contains(pattern),
            "desktop-only Studio code must not specialize {pattern:?}",
        );
    }
}

#[test]
fn failed_project_import_releases_the_blocking_confirmation() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let settings = fs::read_to_string(studio.join("src/frontend/screens/settings/panel.rs"))
        .expect("settings panel source should be readable");

    for behavior in [
        "Project import failed. {error}",
        "pending_project_file.set(None);",
        "project_transfer_error.set(Some(error.clone()));",
        "focus_after_render(\"project-import-input\".to_owned());",
    ] {
        assert!(
            settings.contains(behavior),
            "a failed project import must preserve recovery behavior {behavior:?}",
        );
    }
}

#[test]
fn activity_rail_actions_center_their_vertical_content() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let styles = fs::read_to_string(studio.join("tailwind.css"))
        .expect("activity-rail styles should be readable");
    let rail_action = styles
        .split(".studio-activity-rail .activity-rail-item {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("the unlayered activity-rail action override should exist");

    assert!(
        rail_action.contains("justify-content: center"),
        "row actions need an explicit vertical center in the column rail",
    );
    assert!(
        rail_action.contains("text-align: center"),
        "activity labels should remain centered under their icons",
    );
}

#[test]
fn workflow_language_stays_relation_first_and_truthful() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shell = fs::read_to_string(studio.join("src/frontend/app/shell.rs"))
        .expect("activity rail source should be readable");
    let workflow = fs::read_to_string(studio.join("src/frontend/screens/workflow/workspace.rs"))
        .expect("workflow source should be readable");
    let workflow = workflow
        .split("#[cfg(test)]")
        .next()
        .expect("production Workflow source should precede its tests");

    let data = shell
        .find("onclick: move |_| on_activity.call(Activity::Data)")
        .expect("Data should be registered in the rail");
    let workflow_position = shell
        .find("onclick: move |_| on_activity.call(Activity::Workflow)")
        .expect("Workflow should be registered in the rail");
    let training = shell
        .find("onclick: move |_| on_activity.call(Activity::Training)")
        .expect("Training should be registered in the rail");
    assert!(
        data < workflow_position && workflow_position < training,
        "Workflow belongs beside Data and before Training",
    );

    for expected in [
        "Assistant is not connected",
        "Agent turns arrive in a later stage.",
        "manual Workflow nodes run now",
        "Workflow reads relations already materialized in the active project.",
        "configure SigLIP scoring",
    ] {
        assert!(
            workflow.contains(expected),
            "the Workflow scaffold should state {expected:?}",
        );
    }
    for external_source_node in ["Hugging Face", "MCAP node", "Source node"] {
        assert!(
            !workflow.contains(external_source_node),
            "external import producers do not belong on the Workflow canvas: {external_source_node}",
        );
    }
}

#[test]
fn workflow_model_warning_and_detail_states_have_structured_styles() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let styles = fs::read_to_string(studio.join("tailwind.css"))
        .expect("Workflow styles should be readable");
    for class in [
        ".workflow-node-status-dot.workflow-node-state-warning",
        ".workflow-inspector-warning",
        ".workflow-model-column-map",
        ".workflow-quarantine-examples",
    ] {
        assert!(
            styles.contains(class),
            "Workflow Model state is missing structured style {class:?}",
        );
    }
}
