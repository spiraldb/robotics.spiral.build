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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let roots = [frontend.join("src")];
    let interaction_primitives = frontend.join("src/components/interaction");
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
            if path.starts_with(&interaction_primitives) {
                continue;
            }
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
        "Use the frontend interaction primitives instead:\n{}",
        violations.join("\n"),
    );
}

#[test]
fn legacy_action_styles_cannot_return() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let styles = [frontend.join("tailwind.css")];
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shell = fs::read_to_string(frontend.join("src/shell/frame.rs"))
        .expect("status-bar source should be readable");
    let styles = fs::read_to_string(frontend.join("tailwind.css"))
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let studio_styles = fs::read_to_string(frontend.join("tailwind.css"))
        .expect("Studio styles should be readable");
    let ui_styles = fs::read_to_string(frontend.join("assets/ui.css"))
        .expect("shared UI styles should be readable");
    let crates = frontend.parent().expect("Frontend should live in crates/");
    let viewport_gestures =
        fs::read_to_string(crates.join("spiral-robotics-viewport/src/gesture.rs"))
            .expect("viewport gesture source should be readable");
    let scene = fs::read_to_string(frontend.join("src/panels/scene.rs"))
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let settings = fs::read_to_string(frontend.join("src/settings/panel.rs"))
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let styles = fs::read_to_string(frontend.join("tailwind.css"))
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shell = fs::read_to_string(frontend.join("src/shell/frame.rs"))
        .expect("activity rail source should be readable");
    let workflow = fs::read_to_string(frontend.join("src/activities/workflow/workspace.rs"))
        .expect("workflow source should be readable");
    let workflow = workflow
        .split("#[cfg(test)]")
        .next()
        .expect("production Workflow source should precede its tests");
    let workflow_state = fs::read_to_string(frontend.join("src/activities/workflow/state.rs"))
        .expect("workflow controller source should be readable");
    let workflow_language = format!("{workflow}\n{workflow_state}");

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
        "Workflow Assistant",
        "What should we find?",
        "Enter to send · Shift+Enter for a new line.",
        "Find moments when the gripper holds an object",
        "Jump to latest",
        "Retry last turn",
        "Conversation",
        "New Assistant conversation",
        "Configure or reconnect Spiral before changing conversations.",
        "Stop the active Assistant turn before changing conversations.",
        "Previous conversations are kept",
        "New workflow",
        "Empty workflow",
        "Apply to start fresh",
        "Its saved nodes and connections will appear here.",
        "Ask the Assistant to build this workflow before running.",
        "No workflow yet",
        "Results will appear here after it runs.",
    ] {
        assert!(
            workflow_language.contains(expected),
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
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let styles = fs::read_to_string(frontend.join("tailwind.css"))
        .expect("Workflow styles should be readable");
    for class in [
        ".workflow-node-status-dot.workflow-node-state-warning",
        ".workflow-node-card.workflow-node-state-running",
        ".df-edge.workflow-edge-running",
        ".workflow-node-progress",
        ".workflow-flow-empty",
        ".workflow-inspector-warning",
        ".workflow-feature-definition",
        ".workflow-quarantine-examples",
    ] {
        assert!(
            styles.contains(class),
            "Workflow Model state is missing structured style {class:?}",
        );
    }
    for copy in [
        "Feature name",
        "Description",
        "Model concept",
        "Candidate image field",
        "Streams (optional)",
        "Add feature",
    ] {
        assert!(
            fs::read_to_string(frontend.join("src/activities/workflow/workspace.rs"))
                .expect("Workflow workspace should be readable")
                .contains(copy),
            "Workflow Model definition editor is missing {copy:?}",
        );
    }
}

#[test]
fn workflow_assistant_receipts_results_and_run_states_are_explicit() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = fs::read_to_string(frontend.join("src/activities/workflow/workspace.rs"))
        .expect("Workflow source should be readable");
    let backend = fs::read_to_string(
        frontend
            .parent()
            .expect("Frontend should live in crates/")
            .join("spiral-robotics-backend/src/workflow/agent.rs"),
    )
    .expect("Workflow agent source should be readable");
    let data = fs::read_to_string(frontend.join("src/activities/data/workbench.rs"))
        .expect("query result source should be readable");
    let styles = fs::read_to_string(frontend.join("tailwind.css"))
        .expect("Workflow styles should be readable");

    for receipt in ["View graph", "View run", "View results"] {
        assert!(
            backend.contains(receipt),
            "the execution trace needs the contextual receipt {receipt:?}",
        );
    }
    for expectation in [
        "workflow-edit-badge",
        "workflow-run-badge",
        "Run cancelled",
        "Uncommitted feature work was discarded",
        "any cache publication that completed before cancellation remains authoritative",
        "Run again",
        "render_assistant_text",
        "role: \"status\"",
        "\"aria-live\": \"polite\"",
    ] {
        assert!(
            workspace.contains(expectation),
            "Workflow interaction language is missing {expectation:?}",
        );
    }
    for expectation in [
        "query-media-cell",
        "query-image-preview",
        "fn image_alt",
        "query-result-count",
        "Query results",
    ] {
        assert!(
            data.contains(expectation),
            "shared inline query results are missing {expectation:?}",
        );
    }
    for selector in [
        ".workflow-agent-message-body",
        ".workflow-agent-receipt-action",
        ".workflow-run-badge.is-complete",
        ".query-media-cell",
        ".query-image-preview",
        ".query-column-type",
        ".query-value-number",
        ".query-value-null",
    ] {
        assert!(
            styles.contains(selector),
            "Workflow repair styling is missing {selector:?}",
        );
    }
}
