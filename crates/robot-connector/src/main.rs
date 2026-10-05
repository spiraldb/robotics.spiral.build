//! Native entry point for the Spiral Robotics robot connector.

fn main() -> std::process::ExitCode {
    spiral_robotics_robot_connector::RobotConnectorServer::run()
}
