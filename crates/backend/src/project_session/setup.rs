//! Setup projection from project catalog, hardware, and camera observations.
use super::context::ProjectContext;
use crate::robots::service::RobotCatalogActions;
use spiral_robotics_common::status::StudioSnapshot;
pub(crate) struct ProjectSetup<'a> {
    project: &'a ProjectContext,
}
impl<'a> ProjectSetup<'a> {
    pub(crate) fn new(project: &'a ProjectContext) -> Self {
        Self { project }
    }
    /// Assemble the current setup-level view of the studio.
    pub(crate) async fn snapshot(&self) -> StudioSnapshot {
        let state = self.project;
        let hardware = state.hardware().state();
        let readiness = state.bootstrap().current();
        let robots = if readiness.catalog_available() {
            let robot_adapter = crate::robots::commands::RobotAdapter::prefetched(state).await;
            let mut summaries = Vec::new();
            for saved in RobotCatalogActions::new(&robot_adapter).list().await {
                let connection = hardware
                    .connections()
                    .get(saved.id())
                    .cloned()
                    .unwrap_or_default();
                let summary = robot_adapter.summary(saved.id(), connection).await.ok();
                let Some(mut summary) = summary else {
                    continue;
                };
                // Limits come from the motors, not the saved configuration, so they
                // are folded in here after loading the SpiralDB row.
                if let Some(limits) = hardware.limits().get(saved.id()) {
                    let motors = summary
                        .motors()
                        .iter()
                        .cloned()
                        .map(|mut motor| {
                            if let Some(limits) = limits.get(motor.name()) {
                                motor.set_limits(*limits);
                            }
                            motor
                        })
                        .collect();
                    summary = summary.with_motors(motors);
                }
                summaries.push(summary);
            }
            summaries
        } else {
            Vec::new()
        };

        let mut issues = if readiness.catalog_available() {
            state.workspace().issues()
        } else {
            Vec::new()
        };
        issues.extend(state.cameras().issues());
        // A take that stopped itself has to be visible after the counters that were
        // describing it have gone.
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }

        let cameras = state.cameras().statuses();
        StudioSnapshot::new(readiness, "SpiralDB", hardware.mode_status().clone())
            .with_robots(robots)
            .with_cameras(cameras)
            .with_issues(issues)
    }
}
