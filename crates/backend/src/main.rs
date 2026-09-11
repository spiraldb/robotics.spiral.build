//! Native server entry point for Spiral Robotics Studio.

fn main() -> std::process::ExitCode {
    spiral_robotics_backend::StudioServer::run()
}
