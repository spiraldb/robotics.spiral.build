//! Target-aware Studio development supervisor.
//!
//! Dioxus 0.7 does not include server-only Cargo dep-info in its watcher. Run
//! the browser dev server separately and rebuild the native server here so a
//! backend save never pays for wasm-bindgen or wasm-opt.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

const FRONTEND_PORT: u16 = 57_722;
const BACKEND_PORT: u16 = 57_723;
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

type Snapshot = BTreeMap<PathBuf, FileStamp>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("studio dev supervisor: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let root = workspace_root()?;
    let backend_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let rust_dylib_path = rust_toolchain_library_path()?;

    build_client(&root, &rust_dylib_path)?;
    build_server(&root)?;

    let mut client = spawn_client(&root, &rust_dylib_path)?;
    let mut server = spawn_server(&root, &backend_args)?;
    let mut snapshot = native_snapshot(&root)?;

    println!("Watching native Studio sources; backend saves rebuild only the server target");
    loop {
        thread::sleep(POLL_INTERVAL);
        if let Some(status) = client
            .try_wait()
            .map_err(|error| format!("checking Dioxus dev server: {error}"))?
        {
            stop_child(&mut server);
            return Err(format!("Dioxus dev server exited with {status}"));
        }

        let next = native_snapshot(&root)?;
        if next == snapshot {
            continue;
        }
        snapshot = settle_snapshot(&root, next)?;
        let started = std::time::Instant::now();
        println!("Native source changed; rebuilding Studio server…");
        if let Err(error) = build_server(&root) {
            eprintln!("Native rebuild failed; keeping the previous server running: {error}");
            continue;
        }

        stop_child(&mut server);
        server = spawn_server(&root, &backend_args)?;
        println!(
            "Native rebuild and restart dispatched in {:.2?}",
            started.elapsed()
        );
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve the Cargo workspace root".to_owned())
}

fn rust_toolchain_library_path() -> Result<PathBuf, String> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map_err(|error| format!("locating the Rust toolchain: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "locating the Rust toolchain exited with {}",
            output.status
        ));
    }
    let sysroot = String::from_utf8(output.stdout)
        .map_err(|error| format!("decoding the Rust toolchain path: {error}"))?;
    Ok(PathBuf::from(sysroot.trim()).join("lib"))
}

#[cfg(target_os = "macos")]
const RUST_DYLIB_PATH_VAR: &str = "DYLD_FALLBACK_LIBRARY_PATH";
#[cfg(all(unix, not(target_os = "macos")))]
const RUST_DYLIB_PATH_VAR: &str = "LD_LIBRARY_PATH";
#[cfg(windows)]
const RUST_DYLIB_PATH_VAR: &str = "PATH";

fn build_client(root: &Path, rust_dylib_path: &Path) -> Result<(), String> {
    run_command(
        Command::new("dx")
            .current_dir(root.join("crates/spiral-robotics-studio"))
            .env(RUST_DYLIB_PATH_VAR, rust_dylib_path)
            .args([
                "build",
                "--release",
                "--web",
                "--no-default-features",
                "--features",
                "web",
            ]),
        "building the browser client",
    )
}

fn build_server(root: &Path) -> Result<(), String> {
    run_command(
        Command::new("cargo").current_dir(root).args([
            "build",
            "--profile",
            "server-release",
            "--package",
            "spiral-robotics-studio",
            "--bin",
            "spiral-robotics-studio",
            "--no-default-features",
            "--features",
            "server",
        ]),
        "building the native server",
    )
}

fn run_command(command: &mut Command, action: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{action}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{action} exited with {status}"))
    }
}

fn spawn_client(root: &Path, rust_dylib_path: &Path) -> Result<Child, String> {
    Command::new("dx")
        .current_dir(root.join("crates/spiral-robotics-studio"))
        .env(RUST_DYLIB_PATH_VAR, rust_dylib_path)
        .args([
            "serve",
            "--release",
            "--web",
            "--no-default-features",
            "--features",
            "web",
            "--port",
            &FRONTEND_PORT.to_string(),
            "--open",
            "false",
            "--interactive",
            "false",
        ])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("starting the Dioxus browser dev server: {error}"))
}

fn spawn_server(root: &Path, args: &[OsString]) -> Result<Child, String> {
    let executable = root.join("target/server-release").join(format!(
        "spiral-robotics-studio{}",
        std::env::consts::EXE_SUFFIX
    ));
    let public = root.join("target/dx/spiral-robotics-studio/release/web/public");
    let public_url = format!("http://127.0.0.1:{FRONTEND_PORT}");
    Command::new(&executable)
        .current_dir(root)
        .env("DIOXUS_PUBLIC_PATH", public)
        .env("SPIRAL_ROBOTICS_PUBLIC_URL", public_url)
        .arg("--bind")
        .arg(format!("127.0.0.1:{BACKEND_PORT}"))
        .args(args)
        .spawn()
        .map_err(|error| format!("starting {}: {error}", executable.display()))
}

fn native_snapshot(root: &Path) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot::new();
    for path in [
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("src"),
        root.join("crates"),
        root.join("training/contracts"),
        root.join("training/http-client"),
    ] {
        collect_native_files(&path, &mut snapshot)?;
    }
    Ok(snapshot)
}

fn collect_native_files(path: &Path, snapshot: &mut Snapshot) -> Result<(), String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
    };
    if metadata.is_file() {
        let watched = path.file_name().is_some_and(|name| name == "Cargo.toml")
            || path.extension().is_some_and(|extension| extension == "rs")
            || path.file_name().is_some_and(|name| name == "Cargo.lock");
        if watched {
            snapshot.insert(
                path.to_path_buf(),
                FileStamp {
                    modified: metadata.modified().ok(),
                    len: metadata.len(),
                },
            );
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let entries =
        fs::read_dir(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("reading {}: {error}", path.display()))?;
        collect_native_files(&entry.path(), snapshot)?;
    }
    Ok(())
}

fn settle_snapshot(root: &Path, mut current: Snapshot) -> Result<Snapshot, String> {
    loop {
        thread::sleep(POLL_INTERVAL);
        let next = native_snapshot(root)?;
        if next == current {
            return Ok(next);
        }
        current = next;
    }
}

fn stop_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status();
        let started = std::time::Instant::now();
        while started.elapsed() < SHUTDOWN_TIMEOUT {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
