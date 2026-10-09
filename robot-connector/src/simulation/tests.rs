use std::sync::Arc;

use spiral_robotics_common::robots::config::{MotorAccess, SimulatedMotors};
use spiral_robotics_hardware::{ControlSettings, HardwareHandle, NoSerialLines};

use super::world::ParsedModels;
use super::{DemoBench, SimulatedWorld};
use crate::robotics::cameras::CameraDevice;
use crate::robots::models::RobotModelStore;
use crate::robots::workspace::RobotWorkspace;

#[tokio::test]
async fn the_demo_bench_is_a_guided_leader_and_a_filmed_follower() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = Arc::new(
        RobotWorkspace::open(
            directory.path().join("robots.json"),
            RobotModelStore::new(directory.path().join("models")),
        )
        .unwrap(),
    );
    // A second start installs over the first.
    DemoBench::install(&workspace).await.unwrap();
    DemoBench::install(&workspace).await.unwrap();

    let records = workspace.records();
    let arms = records
        .iter()
        .map(|record| {
            (
                record.id(),
                record.config().motor_access(),
                record.config().calibrated(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        arms,
        [
            (
                "sim_follower",
                Some(MotorAccess::Simulated(SimulatedMotors::new(false))),
                true
            ),
            (
                "sim_leader",
                Some(MotorAccess::Simulated(SimulatedMotors::new(true))),
                true
            ),
        ]
    );
    assert_eq!(records[0].revision(), 2);

    let world = SimulatedWorld::new(
        workspace.clone(),
        Arc::new(HardwareHandle::spawn(
            ControlSettings::default(),
            Arc::new(NoSerialLines),
        )),
    );
    let cameras = world.cameras();
    let ids = cameras
        .iter()
        .map(|camera| camera.status().id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "simulated/sim_follower/front",
            "simulated/sim_follower/wrist"
        ]
    );
    // Both models parse and both mounts name links that exist.
    world.frame(&mut ParsedModels::default()).unwrap();
}
