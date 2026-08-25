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
fn product_crates_follow_functional_boundaries() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    for present in [
        "spiral-robotics-api",
        "spiral-robotics-backend",
        "spiral-robotics-frontend",
        "spiral-robotics-hardware",
        "spiral-robotics-storage",
        "spiral-robotics-studio",
    ] {
        assert!(
            crates.join(present).join("Cargo.toml").is_file(),
            "functional crate {present:?} must exist"
        );
    }
    for retired in [
        "spiral-robotics-boundary-build",
        "spiral-robotics-contracts",
        "spiral-robotics-studio-application",
        "spiral-robotics-studio-backend",
        "spiral-robotics-training-http",
        "spiral-robotics-ui",
    ] {
        assert!(
            !crates.join(retired).exists(),
            "engineering-convenience crate {retired:?} must stay removed"
        );
    }
    let workspace = crates
        .parent()
        .expect("crates directory lives in the workspace");
    assert!(
        !workspace.join("contracts").exists(),
        "generic root contracts directory must stay split between functional owners"
    );
    for metadata in [
        "spiral-robotics-storage/schema/version.json",
        "spiral-robotics-training/protocol/version.json",
    ] {
        assert!(
            crates.join(metadata).is_file(),
            "owned compatibility metadata {metadata:?} must exist"
        );
    }

    let backend_library = fs::read_to_string(backend.join("src/lib.rs")).expect("backend library");
    for capability in [
        "data",
        "recording",
        "robots",
        "rollout",
        "training",
        "workflow",
    ] {
        assert!(
            backend_library.contains(&format!("pub mod {capability};")),
            "backend capability {capability:?} must be visible at the crate root"
        );
    }
}

#[test]
fn physical_io_and_control_share_one_compile_time_boundary() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    let hardware = crates.join("spiral-robotics-hardware");
    assert!(hardware.join("src/lib.rs").is_file());
    for retired in ["spiral-robotics", "spiral-robotics-studio-hardware"] {
        assert!(
            !crates.join(retired).exists(),
            "retired hardware boundary {retired:?} must not return"
        );
    }
    assert!(
        !backend.join("src/robotics/hardware.rs").exists(),
        "physical I/O and fixed-rate control must not drift back into the delivery crate"
    );

    let manifest = fs::read_to_string(hardware.join("Cargo.toml")).expect("hardware manifest");
    for forbidden in [
        "spiral-robotics-backend",
        "axum",
        "dioxus",
        "adbc",
        "arrow",
        "spiral-robotics-inference",
        "spiral-robotics-training",
    ] {
        assert!(
            !declares_local_dependency(&manifest, forbidden),
            "hardware crate depends on outward detail {forbidden:?}"
        );
    }

    let backend_manifest =
        fs::read_to_string(backend.join("Cargo.toml")).expect("backend manifest");
    assert!(
        declares_local_dependency(&backend_manifest, "spiral-robotics-hardware"),
        "the backend must compose the unified hardware crate through Cargo"
    );
}

#[test]
fn read_controllers_delegate_selection_and_admission_to_backend_capabilities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http");
    let routes = fs::read_to_string(root.join("routes.rs")).expect("routes source");
    for seam in [
        "robots::list(&robot_adapter)",
        "devices::CameraDiscoveryAdapter(state.cameras.clone())",
    ] {
        assert!(routes.contains(seam), "state snapshot bypasses {seam}");
    }

    let devices = fs::read_to_string(root.join("routes/devices.rs")).expect("device routes");
    for seam in [
        ".serial_ports()",
        ".cameras()",
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
        "EpisodeQueries::new",
        ".follow_error_trace(",
        ".timeline_traces(",
        ".timeline(id)",
        ".timeline_sample(",
        ".authorize_asset(",
    ] {
        assert!(
            recordings.contains(seam),
            "episode read controller bypasses {seam}"
        );
    }

    let data = fs::read_to_string(root.join("routes/data.rs")).expect("data routes");
    assert!(
        data.contains(".load(&DataHttpAdapter"),
        "remote video identity and selection bypass the Data capability"
    );
}

#[test]
fn episode_catalog_is_loaded_explicitly_from_spiral() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = backend.parent().expect("workspace crates directory");
    let contracts = fs::read_to_string(crates.join("spiral-robotics-api/src/status.rs"))
        .expect("contracts source");
    let snapshot = contracts
        .split("pub struct StudioSnapshot")
        .nth(1)
        .and_then(|source| source.split("/// Lifecycle of the backend services").next())
        .expect("StudioSnapshot declaration");
    assert!(
        !snapshot.contains("pub episodes:"),
        "episode catalog must not ride the live Studio snapshot"
    );
    let routes = fs::read_to_string(backend.join("src/http/routes.rs")).expect("routes source");
    for catalog_data in ["state.episodes.issues()", "list_publications"] {
        assert!(
            !routes.contains(catalog_data),
            "episode catalog data {catalog_data:?} must not ride the live Studio snapshot"
        );
    }

    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    assert!(
        !server.contains("episodes.reload_if_changed()"),
        "the sole-writer episode directory must not be polled"
    );

    let frontend_state =
        fs::read_to_string(crates.join("spiral-robotics-frontend/src/shell/state.rs"))
            .expect("frontend state source");
    assert!(
        frontend_state.contains("api::get::<EpisodeCatalogResponse>(\"/api/episodes\")"),
        "the browser must load episodes as one explicit catalog"
    );
    assert!(
        !backend.join("src/recording/dataset.rs").exists(),
        "the filesystem episode store must stay removed"
    );
    let episode_store = fs::read_to_string(crates.join("spiral-robotics-storage/src/episodes.rs"))
        .expect("Spiral episode store source");
    for retired in [
        "struct FileStamp",
        "fn stamps(&self)",
        "std::fs",
        "tempfile",
    ] {
        assert!(
            !episode_store.contains(retired),
            "the Spiral episode store must not retain filesystem detail {retired:?}"
        );
    }

    let data_state =
        fs::read_to_string(crates.join("spiral-robotics-frontend/src/activities/data/state.rs"))
            .expect("data controller source");
    assert!(
        !data_state.contains("TimeoutFuture::new(1_000)"),
        "publication status must not use the retired one-second poll"
    );
    assert!(
        !data_state.contains("/api/publications"),
        "publication status must travel with the explicit episode catalog"
    );
}

#[test]
fn listener_binding_precedes_concurrent_external_bootstrap() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    let bind = server
        .find("TcpListener::bind(bind).await")
        .expect("server must bind its listener");
    let bootstrap = server
        .find("tokio::spawn(bootstrap_services")
        .expect("server must start background bootstrap");
    assert!(
        bind < bootstrap,
        "external bootstrap must follow listener bind"
    );
    let critical_path = &server[..bind];
    for blocking_startup in [
        "load_studio_preferences()",
        "TrainingManager::open(",
        "refresh_blocking()).await",
        "auto_connect_saved_robots",
    ] {
        assert!(
            !critical_path.contains(blocking_startup),
            "listener startup performs external work {blocking_startup:?}"
        );
    }
    assert!(
        server.contains("tokio::join!(catalog, camera)")
            && server.contains(
                "tokio::join!(preferences, training, workflow, workflow_agent, rollout)",
            ),
        "independent startup I/O must progress concurrently"
    );

    let training = fs::read_to_string(backend.join("src/training/manager.rs"))
        .expect("training manager source");
    assert!(
        training.contains("tokio::join!(backend_probe, spiral_probe)"),
        "training readiness probes must run concurrently"
    );
}

#[test]
fn studio_has_one_spiral_persistence_boundary() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    for retired in ["src/recording/dataset.rs", "src/application/datasets.rs"] {
        assert!(
            !backend.join(retired).exists(),
            "retired filesystem storage module {retired:?} must stay removed"
        );
    }

    let server =
        fs::read_to_string(backend.join("src/http/server.rs")).expect("backend server source");
    for retired_argument in ["workspace: PathBuf", "episodes: PathBuf", "simulate: bool"] {
        assert!(
            !server.contains(retired_argument),
            "Studio must not restore process-local storage/simulation option {retired_argument:?}"
        );
    }
    assert!(
        server.contains("ProjectStore::new")
            && server.contains("RobotWorkspace::new(")
            && server.contains("EpisodeStore::new(")
            && server.contains("StorageManager::new("),
        "robots, episodes, and Studio state must share one Spiral project store"
    );
    for retired in ["spiral_uri:", "StoredSpiralSettings", "DataSettingsStore"] {
        assert!(
            !server.contains(retired),
            "the Spiral project selector must not restore {retired:?}"
        );
    }
    assert!(
        server.contains("BackendStateStore::open_default")
            && !server.contains("Spiral project is required"),
        "Studio must start without a project and recover its bootstrap pointer from backend state"
    );

    let storage = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage");
    let settings =
        fs::read_to_string(storage.join("src/settings.rs")).expect("storage settings source");
    let bootstrap = settings
        .split_once("struct BackendState {")
        .and_then(|(_, source)| source.split_once("pub struct BackendStateStore"))
        .map(|(state, _)| state)
        .expect("backend state must remain an explicit narrow document");
    assert!(bootstrap.contains("project_id: String"));
    assert!(bootstrap.contains("spiral_endpoint: String"));
    assert!(bootstrap.contains("spiral_spfs_endpoint: String"));
    for forbidden in ["episodes:", "robots:", "training:", "rollout:", "video:"] {
        assert!(
            !bootstrap.contains(forbidden),
            "backend bootstrap state must not grow a project-data field {forbidden:?}"
        );
    }

    let project_store =
        fs::read_to_string(storage.join("src/adbc.rs")).expect("Spiral project-store source");
    for retired in [
        "spiral:memory:",
        "memory: Arc",
        "settings: StoredSpiralSettings",
    ] {
        assert!(
            !project_store.contains(retired),
            "the project store must not retain alternate destination plumbing {retired:?}"
        );
    }

    let checkpoints = fs::read_to_string(
        backend
            .parent()
            .expect("workspace crates directory")
            .join("spiral-robotics-storage/src/checkpoints.rs"),
    )
    .expect("checkpoint boundary source");
    for retired in ["CheckpointSource", "register_run_source"] {
        assert!(
            !checkpoints.contains(retired),
            "checkpoints must inherit the selected Spiral destination instead of retaining {retired:?}"
        );
    }
}

#[test]
fn robot_mutations_invalidate_pooled_read_snapshots() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = fs::read_to_string(backend.join("src/robots/workspace.rs"))
        .expect("robot workspace source");
    for (operation, next) in [
        ("pub(crate) fn delete", "fn try_list"),
        ("fn persist", "fn port_value"),
    ] {
        let implementation = workspace
            .split_once(operation)
            .and_then(|(_, source)| source.split_once(next))
            .map(|(implementation, _)| implementation)
            .unwrap_or_else(|| panic!("robot workspace must define {operation}"));
        assert!(
            implementation.contains("run_invalidating_blocking"),
            "robot {operation} must make the next Studio snapshot read fresh state"
        );
    }

    let storage = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage");
    let project_store =
        fs::read_to_string(storage.join("src/adbc.rs")).expect("Spiral project-store source");
    let invalidating = project_store
        .split_once("pub fn run_invalidating_blocking")
        .and_then(|(_, source)| source.split_once("/// Run a namespace replacement"))
        .map(|(implementation, _)| implementation)
        .expect("invalidating project-store mutation implementation");
    assert!(
        invalidating.contains("write(&self.operation_gate)")
            && invalidating.contains("lock(&self.hosted).available.clear()")
            && !invalidating.contains("checkin(connection.into_inner())"),
        "a robot write must drain readers and leave no pre-write or writer connection in the read pool"
    );
}

#[test]
fn synchronous_adbc_is_confined_to_one_bounded_adapter() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = backend
        .parent()
        .expect("workspace crates directory")
        .join("spiral-robotics-storage/src");
    let adbc_path = source_root.join("adbc.rs");
    let mut sources = Vec::new();
    collect_production_rust_sources(&source_root, &mut sources);
    for path in sources {
        if path == adbc_path {
            continue;
        }
        let source = fs::read_to_string(&path).expect("backend source must be readable");
        let names_spiral_connection = source
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .any(|token| token == "SpiralConnection");
        assert!(
            !source.contains("adbc_core")
                && !source.contains("adbc_driver_manager")
                && !source.contains("adbc_driver_spiraldb")
                && !names_spiral_connection,
            "{} bypasses the storage-owned ADBC adapter",
            path.display()
        );
    }

    let adbc = fs::read_to_string(adbc_path).expect("Spiral ADBC source must be readable");
    assert!(
        adbc.contains("use adbc_driver_spiraldb")
            && !adbc.contains("adbc_driver_manager")
            && !adbc.contains("load_dynamic")
            && adbc.contains("struct ConnectionPool")
            && adbc.contains("database: Option<SpiralDatabase>")
            && adbc.contains("available: Vec<SpiralConnection>")
            && adbc.contains("let mut driver = SpiralDriver")
            && adbc.contains("if self.database.is_none()")
            && adbc.contains("Semaphore::new(HOSTED_ADBC_CONNECTIONS)"),
        "the adapter must own one native database/engine and bound its connection pool"
    );
    let server = fs::read_to_string(backend.join("src/http/server.rs"))
        .expect("backend server source must be readable");
    assert!(
        !server.contains("spiral-adbc-driver") && !server.contains("SPIRALDB_ADBC_DRIVER"),
        "Studio must not accept a runtime path for its linked SpiralDB driver"
    );
    for operation in [
        "pub async fn run",
        "pub fn run_blocking",
        "pub fn run_invalidating_blocking",
        "pub fn run_exclusive_blocking",
    ] {
        let implementation = adbc
            .split_once(operation)
            .map(|(_, implementation)| implementation)
            .expect("project-store operation must exist");
        let next_operation = implementation
            .find("pub fn ")
            .into_iter()
            .chain(implementation.find("pub async fn "))
            .min()
            .unwrap_or(implementation.len());
        let implementation = &implementation[..next_operation];
        assert!(
            implementation.contains("hosted_slots"),
            "{operation} must consume the shared pool capacity gate"
        );
    }
}

#[test]
fn backend_associated_types_are_only_closed_loop_capability_tokens() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = BTreeSet::from([
        ("recording.rs".to_owned(), "MediaCapture".to_owned()),
        ("recording.rs".to_owned(), "MediaOutput".to_owned()),
        ("recording.rs".to_owned(), "RecordingDraft".to_owned()),
        ("service.rs".to_owned(), "CameraConfiguration".to_owned()),
    ]);

    let mut declared = BTreeSet::new();
    let mut sources = Vec::new();
    collect_production_rust_sources(&directory, &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path).expect("backend source must be readable");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("backend source has a UTF-8 file name");
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
            for (_owner, token) in &allowed {
                if line.contains(&format!("::{token}")) {
                    assert!(
                        line.contains(&format!("Self::{token}"))
                            || line.contains(&format!("P::{token}")),
                        "{} lets associated capability {token} escape its declaring use-case boundary: {line}",
                        path.display()
                    );
                }
            }
        }
    }

    assert_eq!(
        declared, allowed,
        "backend associated types must be an explicitly reviewed, adapter-minted and adapter-consumed capability"
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
            "spiral-robotics-api",
            "crates/spiral-robotics-api/Cargo.toml",
            &["spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics-hardware",
            "crates/spiral-robotics-hardware/Cargo.toml",
            &["spiral-robotics-api", "spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics-inference",
            "crates/spiral-robotics-inference/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-training",
            "crates/spiral-robotics-training/Cargo.toml",
            &[][..],
        ),
        (
            "spiral-robotics-viewport",
            "crates/spiral-robotics-viewport/Cargo.toml",
            &["spiral-robotics-core"][..],
        ),
        (
            "spiral-robotics-studio",
            "crates/spiral-robotics-studio/Cargo.toml",
            &["spiral-robotics-backend", "spiral-robotics-frontend"][..],
        ),
        (
            "spiral-robotics-frontend",
            "crates/spiral-robotics-frontend/Cargo.toml",
            &[
                "spiral-robotics-api",
                "spiral-robotics-core",
                "spiral-robotics-viewport",
            ][..],
        ),
        (
            "spiral-robotics-storage",
            "crates/spiral-robotics-storage/Cargo.toml",
            &[
                "spiral-robotics-api",
                "spiral-robotics-core",
                "spiral-robotics-hardware",
            ][..],
        ),
        (
            "spiral-robotics-backend",
            "crates/spiral-robotics-backend/Cargo.toml",
            &[
                "spiral-robotics-api",
                "spiral-robotics-core",
                "spiral-robotics-hardware",
                "spiral-robotics-inference",
                "spiral-robotics-storage",
                "spiral-robotics-training",
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
fn studio_training_owns_only_the_remote_backend_trait_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manager =
        fs::read_to_string(root.join("crates/spiral-robotics-backend/src/training/manager.rs"))
            .expect("training manager source");
    let job_loop = manager
        .split_once("async fn run_backend")
        .and_then(|(_, source)| source.split_once("fn apply_remote_job_snapshot"))
        .map(|(source, _)| source)
        .expect("training manager must have one explicit worker-job loop");
    assert!(
        manager.contains("training_backend: Arc<dyn TrainingBackend>")
            && job_loop.contains("create_job(job_request)")
            && job_loop.contains("backend.job_snapshot(&next_run_id, cursor)")
            && job_loop.contains("backend.stop_job(&next_run_id, cursor)"),
        "Studio must orchestrate a backend-neutral worker job"
    );
    for forbidden_data_plane in [
        "run_training_adbc",
        "SpiralActLoader",
        "create_session",
        "buffer_training_batch",
        "TrainingBatch",
    ] {
        assert!(
            !job_loop.contains(forbidden_data_plane),
            "the active Studio job loop must not retain training data-plane detail {forbidden_data_plane:?}"
        );
    }
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

    let adapter = fs::read_to_string(root.join("training/src/spiral_training_runtime/act.py"))
        .expect("Python PyTorch training adapter source");
    assert!(
        adapter.contains("ACTPolicy") && adapter.contains("torch.optim.AdamW"),
        "the remote implementation must use the real PyTorch ACT policy and optimizer"
    );
    let spiral = fs::read_to_string(root.join("training/src/spiral_training_runtime/spiral.py"))
        .expect("Python Spiral training adapter source");
    for required in [
        "spiral.api_url",
        "spiral.spfs_url",
        "username",
        "password",
        "spql.video.frames",
        "spql.video.decode",
        "training_checkpoints",
    ] {
        assert!(
            spiral.contains(required),
            "the worker must own Spiral training operation {required:?}"
        );
    }
    for removed_native_crate in [
        "crates/spiral-robotics-inference-pytorch",
        "crates/spiral-robotics-training-pytorch",
        "crates/spiral-robotics-training-runtime",
    ] {
        assert!(
            !root.join(removed_native_crate).exists(),
            "obsolete native training crate {removed_native_crate:?} must stay removed"
        );
    }
}

#[test]
fn studio_rollout_does_not_enable_or_call_the_pytorch_backend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Studio lives under the workspace crates directory");
    let manifest = fs::read_to_string(root.join("crates/spiral-robotics-backend/Cargo.toml"))
        .expect("Studio backend manifest");
    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "spiral-robotics-inference.workspace = true"),
        "Studio must consume only the featureless inference contract"
    );
    assert!(
        !manifest.contains("spiral-robotics-inference-pytorch"),
        "Studio must not depend on a native PyTorch inference implementation"
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
        "crates/spiral-robotics-backend/build.rs",
    ] {
        assert!(
            !root.join(obsolete_build_script).exists(),
            "Studio must not inject tensor-runtime linker settings"
        );
    }

    let rollout =
        fs::read_to_string(root.join("crates/spiral-robotics-backend/src/rollout/manager.rs"))
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
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rollout/manager.rs"))
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
