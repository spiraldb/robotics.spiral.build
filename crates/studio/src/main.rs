//! Browser entrypoint and independent native Studio server launcher.

#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn main() -> std::process::ExitCode {
    spiral_robotics_backend::StudioServer::run()
}

#[cfg(all(target_arch = "wasm32", feature = "web"))]
fn main() {
    dioxus::launch(spiral_robotics_frontend::App);
}

#[cfg(not(any(
    all(not(target_arch = "wasm32"), feature = "server"),
    all(target_arch = "wasm32", feature = "web"),
)))]
fn main() -> std::process::ExitCode {
    eprintln!("Run `dx serve --fullstack` to build and start Studio.");
    std::process::ExitCode::FAILURE
}
