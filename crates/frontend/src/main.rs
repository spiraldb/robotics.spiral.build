//! Browser entry point for Spiral Robotics Studio.

fn main() {
    // A panic aborts the module, which the browser reports only as an
    // unreachable trap; this logs its message, location, and stack first.
    console_error_panic_hook::set_once();
    dioxus::launch(spiral_robotics_frontend::App);
}
