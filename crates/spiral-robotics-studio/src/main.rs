//! Platform entrypoint for the single-crate Dioxus fullstack Studio.

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    spiral_robotics_studio::backend::run()
}

#[cfg(all(not(feature = "server"), feature = "web"))]
fn main() {
    dioxus::launch(spiral_robotics_studio::App);
}

#[cfg(not(any(feature = "server", feature = "web")))]
fn main() -> std::process::ExitCode {
    eprintln!("Spiral Robotics Studio is a Dioxus fullstack app; run it with `dx serve`");
    std::process::ExitCode::FAILURE
}
