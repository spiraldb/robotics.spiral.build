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
fn completed_episode_selection_belongs_to_the_data_controller() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = fs::read_to_string(frontend.join("src/activities/data/state.rs"))
        .expect("Data controller source should be readable");
    let studio = fs::read_to_string(frontend.join("src/shell/state.rs"))
        .expect("Studio state source should be readable");

    assert!(
        data.contains("selected_episode: Signal<Option<String>>"),
        "the Data controller must own completed-episode selection",
    );
    assert!(
        !studio.contains("selected_episode"),
        "completed-episode selection must not drift back into Studio-wide state",
    );
}

#[test]
fn episode_catalog_and_full_data_belong_to_the_data_controller() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = fs::read_to_string(frontend.join("src/activities/data/state.rs"))
        .expect("Data controller source should be readable");
    let studio = fs::read_to_string(frontend.join("src/shell/state.rs"))
        .expect("Studio state source should be readable");
    let episodes = fs::read_to_string(frontend.join("src/activities/data/workbench.rs"))
        .expect("Episodes panel source should be readable");
    let workspace = fs::read_to_string(frontend.join("src/shell/workspace.rs"))
        .expect("Episode workspace source should be readable");
    let preview = fs::read_to_string(frontend.join("src/activities/data/episode_cache.rs"))
        .expect("Episode preview source should be readable");

    for ownership in [
        "episode_catalog: Signal<EpisodeCatalogState>",
        "episode_loader: Resource<Option<EpisodeLoadResult>>",
        "summary: EpisodeListItem",
        "full_data: Option<Arc<EpisodeData>>",
        "Ready(Arc<EpisodeData>)",
    ] {
        assert!(
            data.contains(ownership),
            "the Data controller is missing episode ownership {ownership:?}",
        );
    }
    assert!(
        !data.contains("full_data: EpisodeDataState"),
        "catalog entries must cache successful data rather than duplicate resource state",
    );
    assert!(
        !studio.contains("episode_catalog"),
        "the episode catalog must not drift back into Studio-wide state",
    );
    assert!(
        !episodes.contains("data.load_episodes("),
        "the panel must not own a second catalog loader"
    );
    assert!(data.contains("fn use_polling"));
    assert!(data.contains("QueryDomain::Data"));
    assert!(
        workspace.contains("match data.selected_episode_data()"),
        "Data renderers must derive directly from the selected resource",
    );
    assert!(
        !preview.contains("data: Option<DataController>") && !preview.contains("with_data"),
        "the non-Data preview cache must not mirror DataController results",
    );
}

#[test]
fn workflow_backend_state_has_one_frontend_owner() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let studio = fs::read_to_string(frontend.join("src/shell/state.rs"))
        .expect("Studio state source should be readable");
    let workflow = fs::read_to_string(frontend.join("src/activities/workflow/state.rs"))
        .expect("Workflow controller source should be readable");
    let frame = fs::read_to_string(frontend.join("src/shell/frame.rs"))
        .expect("Status bar source should be readable");

    assert!(!studio.contains("workflow_state: Signal"));
    assert!(!studio.contains("workflow_agent_state: Signal"));
    assert!(studio.contains("self.workflow_controller().use_polling(self)"));
    assert!(workflow.contains("fn accept_fetched_state"));
    assert!(workflow.contains("fn accept_fetched_agent"));
    assert!(!workflow.contains("reconcile_state_command"));
    assert!(!workflow.contains("accept_streamed"));
    assert!(workflow.contains("pub(crate) fn accepted_state"));
    assert!(frame.contains("studio.workflow_controller().accepted_state()"));
}

#[test]
fn project_lifetime_contexts_wrap_the_session_owned_feature_controllers() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_root = frontend.join("src");
    let studio = fs::read_to_string(source_root.join("shell/state.rs"))
        .expect("Studio state source should be readable");
    let providers = files_with_extension(&source_root, "rs")
        .into_iter()
        .flat_map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
            source
                .split("#[cfg(test)]")
                .next()
                .unwrap()
                .match_indices("use_context_provider")
                .map(move |(offset, _)| (path.clone(), offset))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let allowed = [
        "src/shell/state.rs",
        "src/shell/project.rs",
        "src/shell/routes.rs",
        "src/client/queries.rs",
    ];
    assert_eq!(providers.len(), allowed.len());
    for (path, _) in &providers {
        assert!(
            allowed.iter().any(|allowed| path.ends_with(allowed)),
            "unexpected context owner: {}",
            path.display()
        );
    }
    for controller in [
        "data: DataController",
        "workflow: WorkflowController",
        "training: TrainingController",
        "rollout: RolloutController",
        "timeline: TimelineController",
    ] {
        assert!(
            studio.contains(controller),
            "Studio is missing session controller {controller:?}",
        );
    }
    assert!(studio.contains("settings: Resource<Result<DataSettings, String>>"));
    assert!(studio.contains("configuration: Memo<StudioConfiguration>"));
}

#[test]
fn serial_port_catalog_is_owned_once_by_the_robots_view() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let robots = fs::read_to_string(frontend.join("src/settings/robots/mod.rs"))
        .expect("Robot settings source should be readable");
    let robot_panel = robots
        .split_once("pub(super) fn RobotPanel")
        .expect("RobotPanel should exist")
        .1
        .split_once("fn RobotEntry")
        .expect("RobotEntry should follow RobotPanel")
        .0;
    let robot_setup = robots
        .split_once("fn RobotSetup")
        .expect("RobotSetup should exist")
        .1
        .split_once("trait RobotTypePresentation")
        .expect("RobotSetup should precede presentation helpers")
        .0;

    assert!(robot_panel.contains("use_resource"));
    assert!(robot_panel.contains("PortCatalogState::new"));
    assert!(!robot_setup.contains("use_resource"));
}

#[test]
fn relation_schemas_belong_to_studio_while_exact_counts_are_data_view_scoped() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let studio = fs::read_to_string(frontend.join("src/shell/state.rs"))
        .expect("Studio state source should be readable");
    let routes = fs::read_to_string(frontend.join("src/shell/routes.rs"))
        .expect("Studio routes source should be readable");
    let data_state = fs::read_to_string(frontend.join("src/activities/data/state.rs"))
        .expect("Data controller source should be readable");
    let relations = fs::read_to_string(frontend.join("src/activities/data/relations.rs"))
        .expect("Data relations source should be readable");
    let query = fs::read_to_string(frontend.join("src/activities/data/workbench.rs"))
        .expect("Data Query source should be readable");
    let workflow = fs::read_to_string(frontend.join("src/activities/workflow/state.rs"))
        .expect("Workflow controller source should be readable");

    assert!(studio.contains("relation_catalog: Resource<Result<RelationCatalog, String>>"));
    assert_eq!(studio.matches("relation_catalog()").count(), 1);
    assert!(routes.contains("Studio::use_studio"));
    assert!(!frontend.join("src/shell/catalog.rs").exists());
    assert!(!query.contains("get::<RelationCatalog>"));
    assert!(!workflow.contains("get::<RelationCatalog>"));
    assert!(!data_state.contains("relation_generation"));
    assert!(relations.contains("statements.join(\"\\nUNION ALL\\n\")"));
    assert!(relations.contains("COUNT(*) AS row_count"));
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
    let viewport_gestures = fs::read_to_string(frontend.join("viewport/src/gesture.rs"))
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
        "BrowserFocus::after_render(\"project-import-input\".to_owned());",
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
            .join("backend/src/workflow/agent.rs"),
    )
    .expect("Workflow agent source should be readable");
    let data = fs::read_to_string(frontend.join("src/activities/data/query_result.rs"))
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
        "AssistantDocument::parse",
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
