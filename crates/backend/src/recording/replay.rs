//! Driving a follower through a recorded take.
use spiral_robotics_hardware::{ReplayTrajectory, RobotPlaybackRequest};
use spiral_robotics_storage::episodes::RobotPlaybackRequest as RecordedPlaybackRequest;

use crate::OperationError;
use crate::project_session::context::ProjectContext;
use crate::recording::episode_arms::RecordedArms;

pub(super) struct StartReplayCommand {
    episode: String,
    robot: String,
    source_arm: Option<String>,
    start_seconds: f64,
}

impl StartReplayCommand {
    pub(super) fn new(
        (episode, robot): (String, String),
        source_arm: Option<String>,
        start_seconds: f64,
    ) -> Self {
        Self {
            episode,
            robot,
            source_arm,
            start_seconds,
        }
    }

    /// Resolve the recorded arm, load its trajectory from `start_seconds`, and
    /// admit the replay. The hardware request validates the trajectory before
    /// any motor moves.
    pub(super) async fn run(self, state: &ProjectContext) -> Result<(), OperationError> {
        if self.episode.trim().is_empty() || self.robot.trim().is_empty() {
            return Err(OperationError::InvalidInput(
                "replay requires an episode and target robot".to_owned(),
            ));
        }
        if !self.start_seconds.is_finite() || self.start_seconds < 0.0 {
            return Err(OperationError::InvalidInput(
                "replay start time must be a finite, non-negative number of seconds".to_owned(),
            ));
        }
        let summary = state.recording_buffer().get(&self.episode)?;
        let arms = summary
            .arms()
            .iter()
            .map(|arm| arm.id().to_owned())
            .collect::<Vec<_>>();
        let source = RecordedArms::new(summary.id(), &arms, &self.robot)
            .select(self.source_arm.as_deref())?;
        let trajectory = state.recording_buffer().trajectory_at(
            &self.episode,
            RecordedPlaybackRequest::new(&source, self.start_seconds),
        )?;
        let (start_frame, resolved_start_seconds) = (
            trajectory.start_frame(),
            trajectory.resolved_start_seconds(),
        );
        let request = ReplayTrajectory::new(trajectory.into_frames(), summary.sample_hz())
            .and_then(|trajectory| {
                RobotPlaybackRequest::timeline(self.robot, self.episode, trajectory)
            })
            .and_then(|request| request.starting_at(start_frame, resolved_start_seconds))
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        state
            .hardware()
            .start_replay(request)
            .await
            .map_err(OperationError::conflict)
    }
}
