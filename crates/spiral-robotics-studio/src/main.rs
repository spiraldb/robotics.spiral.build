//! Platform entrypoint for the Dioxus fullstack Studio.

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    spiral_robotics_backend::StudioServer::run(spiral_robotics_frontend::App)
}

#[cfg(all(not(feature = "server"), feature = "web"))]
fn main() {
    dioxus::launch(spiral_robotics_frontend::App);
}

#[cfg(not(any(feature = "server", feature = "web")))]
fn main() -> std::process::ExitCode {
    eprintln!("Spiral Robotics Studio is a Dioxus fullstack app; run it with `dx serve`");
    std::process::ExitCode::FAILURE
}
