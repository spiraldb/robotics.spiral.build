use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn collect_production_rust_sources(directory: &Path, sources: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory must exist") {
        let entry = entry.expect("source entry must be readable");
        let path = entry.path();
        if entry
            .file_type()
            .expect("source type must be readable")
            .is_dir()
        {
            collect_production_rust_sources(&path, sources);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs")
            && path.file_name().and_then(|value| value.to_str()) != Some("tests.rs")
        {
            sources.push(path);
        }
    }
}

fn declares_local_dependency(manifest: &str, dependency: &str) -> bool {
    let package = manifest
        .split_once("[package]")
        .map_or(manifest, |(_, package)| package);
    let workspace_key = format!("{dependency}.workspace");
    let table_key = format!("{dependency} =");
    package.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(&workspace_key) || line.starts_with(&table_key)
    })
}

#[test]
fn application_policy_does_not_import_delivery_or_adapter_details() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend/application");
    let forbidden = [
        "axum",
        "dioxus",
        "serde",
        "serde_json",
        "tokio",
        "bytes::",
        "arrow_",
        "adbc_",
        "parquet::",
        "zip::",
        "safetensors::",
        "spiral_robotics::",
        "spiral_robotics_inference",
        "spiral_robotics_contracts",
        "backend::http",
        "backend::storage",
        "backend::robotics",
        "backend::recording",
        "backend::robots",
        "std::fs",
        "std::env",
        "std::io",
        "std::net",
        "std::process",
        "std::path",
        "SystemTime",
        "PathBuf",
        "StatusCode",
        "checkpoint_relation",
        "checkpoint_table",
        "spiral_table",
    ];

    let mut sources = Vec::new();
    collect_production_rust_sources(&directory, &mut sources);
    assert!(!sources.is_empty(), "application policy sources must exist");
    for path in sources {
        let source = fs::read_to_string(&path).expect("application source must be readable");
        // Test adapters may use an async runtime. Only production policy is
        // part of this dependency assertion.
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for outward in forbidden {
            assert!(
                !production.contains(outward),
                "{} imports outward detail {outward:?}",
                path.display()
            );
        }
    }
}

#[test]
fn read_controllers_delegate_selection_and_admission_to_application_use_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend/http");
    let routes = fs::read_to_string(root.join("routes.rs")).expect("routes source");
    for seam in [
        "robots::list(&robot_adapter)",
        "episode_queries::list(&recordings::EpisodeHttpAdapter::catalog(state))",
        "device_discovery::list_cameras(&state.cameras)",
        "data_application::list_publications(&data::DataHttpAdapter::new(state))",
    ] {
        assert!(routes.contains(seam), "state snapshot bypasses {seam}");
    }

    let devices = fs::read_to_string(root.join("routes/devices.rs")).expect("device routes");
    for seam in [
        "device_discovery::list_serial_ports",
        "device_discovery::list_cameras",
        "robots::list(&adapter)",
        "robots::get(&RobotHttpAdapter::new(&state), id)",
    ] {
        assert!(
            devices.contains(seam),
            "device read controller bypasses {seam}"
        );
    }

    let recordings =
        fs::read_to_string(root.join("routes/recordings.rs")).expect("recording routes");
    for seam in [
        "episode_queries::list",
        "episode_queries::follow_error_trace",
        "episode_queries::timeline_traces",
        "episode_queries::timeline(",
        "episode_queries::timeline_sample",
        "episode_queries::authorize_asset",
    ] {
        assert!(
            recordings.contains(seam),
            "episode read controller bypasses {seam}"
        );
    }

    let data = fs::read_to_string(root.join("routes/data.rs")).expect("data routes");
    assert!(
        data.contains("data_application::video_asset"),
        "remote video identity and selection bypass the application use case"
    );
}

#[test]
fn application_associated_types_are_only_closed_loop_capability_tokens() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend/application");
    let allowed = BTreeSet::from([
        ("datasets.rs".to_owned(), "PreparedExport".to_owned()),
        ("datasets.rs".to_owned(), "StagedImport".to_owned()),
        ("recording.rs".to_owned(), "MediaCapture".to_owned()),
        ("recording.rs".to_owned(), "MediaOutput".to_owned()),
        ("recording.rs".to_owned(), "RecordingDraft".to_owned()),
        ("robots.rs".to_owned(), "CameraConfiguration".to_owned()),
        (
            "training_execution.rs".to_owned(),
            "CheckpointEnvelope".to_owned(),
        ),
        (
            "training_execution.rs".to_owned(),
            "SourceHandle".to_owned(),
        ),
    ]);

    let mut declared = BTreeSet::new();
    let mut sources = Vec::new();
    collect_production_rust_sources(&directory, &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path).expect("application source must be readable");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("application source has a UTF-8 file name");
        for line in production.lines() {
            let line = line.trim();
            if let Some(name) = line
                .strip_prefix("type ")
                .and_then(|declaration| declaration.split(';').next())
                .map(str::trim)
                .filter(|declaration| !declaration.contains('='))
            {
                declared.insert((file.to_owned(), name.to_owned()));
            }
            for (owner, token) in &allowed {
                if line.contains(&format!("::{token}")) {
                    assert!(
                        file == owner
                            && (line.contains(&format!("Self::{token}"))
                                || line.contains(&format!("P::{token}"))),
                        "{} lets associated capability {token} escape its declaring use-case boundary: {line}",
                        path.display()
                    );
                }
            }
        }
    }

    assert_eq!(
        declared, allowed,
        "application associated types must be an explicitly reviewed, adapter-minted and adapter-consumed capability"
    );
}

#[test]
fn workspace_crate_dependencies_point_toward_policy() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manifest = |relative: &str| {
        fs::read_to_string(workspace.join(relative))
            .unwrap_or_else(|error| panic!("could not read {relative}: {error}"))
    };

    let crates = [
        (
            "spiral-robotics-core",
            "crates/spiral-robotics-core/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-contracts",
            "crates/spiral-robotics-contracts/Cargo.toml",
            &["spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics",
            "Cargo.toml",
            &["spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics-inference",
            "crates/spiral-robotics-inference/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-inference-pytorch",
            "crates/spiral-robotics-inference-pytorch/Cargo.toml",
            &["spiral-robotics-inference"][..],
        ),
        (
            "spiral-robotics-panels",
            "crates/spiral-robotics-panels/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-training",
            "crates/spiral-robotics-training/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-training-http",
            "crates/spiral-robotics-training-http/Cargo.toml",
            &["spiral-robotics-inference", "spiral-robotics-training"][..],
        ),
        (
            "spiral-robotics-training-pytorch",
            "crates/spiral-robotics-training-pytorch/Cargo.toml",
            &[
                "spiral-robotics-inference-pytorch",
                "spiral-robotics-training",
            ][..],
        ),
        (
            "spiral-robotics-training-runtime",
            "crates/spiral-robotics-training-runtime/Cargo.toml",
            &[
                "spiral-robotics-inference",
                "spiral-robotics-inference-pytorch",
                "spiral-robotics-training-http",
                "spiral-robotics-training-pytorch",
            ][..],
        ),
        (
            "spiral-robotics-viewport",
            "crates/spiral-robotics-viewport/Cargo.toml",
            &["spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics-studio",
            "crates/spiral-robotics-studio/Cargo.toml",
            &[
                "spiral-robotics-contracts",
                "spiral-robotics-core",
                "spiral-robotics-panels",
                "spiral-robotics-studio-backend",
                "spiral-robotics-viewport",
            ][..],
        ),
        (
            "spiral-robotics-studio-backend",
            "crates/spiral-robotics-studio-backend/Cargo.toml",
            &[
                "spiral-robotics",
                "spiral-robotics-contracts",
                "spiral-robotics-core",
                "spiral-robotics-inference",
                "spiral-robotics-training",
                "spiral-robotics-training-http",
            ][..],
        ),
    ];
    let local_names: Vec<_> = crates.iter().map(|(name, _, _)| *name).collect();

    for (name, path, allowed_dependencies) in crates {
        let source = manifest(path);
        for dependency in &local_names {
            if dependency == &name {
                continue;
            }
            assert_eq!(
                declares_local_dependency(&source, dependency),
                allowed_dependencies.contains(dependency),
                "{name} has an unexpected workspace dependency relationship with {dependency}"
            );
        }
    }
}

#[test]
fn studio_training_owns_only_the_backend_trait_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manager = fs::read_to_string(
        root.join("crates/spiral-robotics-studio-backend/src/backend/training/manager.rs"),
    )
    .expect("training manager source");
    assert!(
        manager.contains("training_backend: Arc<dyn TrainingBackend>")
            && manager.contains(".create_session")
            && manager.contains("trainer: &mut dyn TrainingSession"),
        "Studio must own only the backend-neutral training trait objects"
    );
    for detail in [
        "NativeActTrainer",
        "VisionActTrainingConfig",
        "VisionActBatch",
        "tch::",
        "torch_sys",
    ] {
        assert!(
            !manager.contains(detail),
            "Studio training manager imports backend detail {detail:?}"
        );
    }

    let adapter =
        fs::read_to_string(root.join("crates/spiral-robotics-training-pytorch/src/lib.rs"))
            .expect("PyTorch training adapter source");
    assert!(
        adapter.contains("impl TrainingBackend for PyTorchTrainingBackend"),
        "PyTorch must enter through the public training boundary"
    );
}

#[test]
fn studio_rollout_does_not_enable_or_call_the_pytorch_backend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manifest =
        fs::read_to_string(root.join("crates/spiral-robotics-studio-backend/Cargo.toml"))
            .expect("Studio backend manifest");
    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "spiral-robotics-inference.workspace = true"),
        "Studio must consume only the featureless inference contract"
    );
    assert!(
        !manifest.contains("spiral-robotics-inference-pytorch"),
        "Studio must not depend on the PyTorch inference implementation"
    );
    let inference_manifest =
        fs::read_to_string(root.join("crates/spiral-robotics-inference/Cargo.toml"))
            .expect("framework-neutral inference manifest");
    for native_dependency in ["tch", "torch-sys", "pytorch"] {
        assert!(
            !inference_manifest
                .to_ascii_lowercase()
                .contains(native_dependency),
            "framework-neutral inference contains native dependency {native_dependency:?}"
        );
    }
    for obsolete_build_script in [
        "crates/spiral-robotics-studio/build.rs",
        "crates/spiral-robotics-studio-backend/build.rs",
    ] {
        assert!(
            !root.join(obsolete_build_script).exists(),
            "Studio must not inject tensor-runtime linker settings"
        );
    }

    let rollout = fs::read_to_string(
        root.join("crates/spiral-robotics-studio-backend/src/backend/rollout/manager.rs"),
    )
    .expect("rollout manager source");
    assert!(
        rollout.contains("HttpPolicyLoader"),
        "Studio rollout must load policy sessions through the remote runtime"
    );
    for local_backend in [
        "spiral_robotics_inference::act",
        "load_cpu_artifact",
        "load_metal_artifact",
        "LibTorch",
    ] {
        assert!(
            !rollout.contains(local_backend),
            "Studio rollout contains local tensor backend detail {local_backend:?}"
        );
    }
}

#[test]
fn rollout_action_scheduler_contains_no_inline_inference_work() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend/rollout/manager.rs"),
    )
    .expect("rollout manager source");
    let scheduler = source
        .split_once("async fn run_session(")
        .and_then(|(_, body)| body.split_once("    fn classify_control("))
        .map(|(body, _)| body)
        .expect("rollout session scheduler boundaries");

    assert!(
        scheduler.contains("spawn_inference(next_policy, observation)"),
        "the running action scheduler must hand inference to its worker"
    );
    assert!(
        scheduler.contains("actions.merge(completed.requested_timestep, &completed.chunk)")
            && scheduler.contains("let action = actions.pop()?"),
        "async rollout must merge timed chunks into the non-blocking action queue"
    );
    let dispatch = scheduler
        .find(".set_policy_goals(")
        .expect("the scheduler dispatches a physical action");
    let observe = scheduler
        .find("pending_observation.replace(observation)")
        .expect("the scheduler captures a latest-only observation");
    assert!(
        dispatch < observe
            && scheduler.contains("let latest_action = actions.latest_action_timestep()?")
            && scheduler.contains("action_refill_threshold(initial.chunk.steps())"),
        "LeRobot parity requires execute-then-observe timestamps and the default half-chunk refill threshold"
    );
    assert!(
        !scheduler.contains("policy.infer(")
            && !scheduler.contains("decode_images(")
            && !scheduler.contains("block_in_place"),
        "camera decoding and model execution must never run inline with physical action dispatch"
    );
}
