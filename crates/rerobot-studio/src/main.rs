//! Platform entrypoint for the single-crate Dioxus fullstack Studio.

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    rerobot_studio::backend::run()
}

#[cfg(all(not(feature = "server"), feature = "web"))]
fn main() {
    dioxus::launch(rerobot_studio::App);
}

#[cfg(not(any(feature = "server", feature = "web")))]
fn main() -> std::process::ExitCode {
    eprintln!("rerobot Studio is a Dioxus fullstack app; run it with `dx serve`");
    std::process::ExitCode::FAILURE
}
