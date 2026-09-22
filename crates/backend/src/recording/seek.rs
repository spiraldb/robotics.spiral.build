//! Recorded physical-seek capability action.
//!
//! Delivery supplies plain input. The action admits the motion, chooses the
//! recorded source arm, validates the sampled pose, and orders ticketing,
//! sampling, and the atomic current-ticket controller handoff.

use std::collections::BTreeMap;

use super::episode_arms::RecordedArms;
use crate::OperationError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeekEpisodeCommand {
    episode: String,
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeekControlMode {
    Idle,
    Replay,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveRecording {
    episode: String,
    paused: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeekActivity {
    recording: Option<ActiveRecording>,
    mode: SeekControlMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeekTarget {
    id: String,
    robot_type: Option<String>,
    connected: bool,
    calibrated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeekableEpisode {
    id: String,
    arms: Vec<String>,
    sample_hz: f64,
    problem: Option<String>,
    unfinished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SampledArm {
    id: String,
    normalized: BTreeMap<String, Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordedSample {
    seconds: f64,
    frame: u64,
    arms: Vec<SampledArm>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhysicalSeekPlan {
    ticket: u64,
    robot: String,
    episode: String,
    pose: BTreeMap<String, f64>,
    sample_hz: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordedSeekState {
    Seeking,
    Coalesced,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordedSeekResult {
    state: RecordedSeekState,
    episode: String,
    robot: String,
    source_arm: String,
    requested_seconds: f64,
    resolved_seconds: f64,
    frame: u64,
}

impl SeekEpisodeCommand {
    pub(super) fn new(episode: impl Into<String>, robot: impl Into<String>, seconds: f64) -> Self {
        Self {
            episode: episode.into(),
            robot: robot.into(),
            source_arm: None,
            seconds,
        }
    }

    pub(super) fn with_source_arm(mut self, source_arm: Option<String>) -> Self {
        self.source_arm = source_arm;
        self
    }
}

impl ActiveRecording {
    pub(super) fn new(episode: impl Into<String>, paused: bool) -> Self {
        Self {
            episode: episode.into(),
            paused,
        }
    }

    fn seek_blocker(&self, episode: &str) -> Option<String> {
        if !self.paused {
            return Some(
                "pause the active recording before moving a physical arm through history"
                    .to_owned(),
            );
        }
        (self.episode != episode).then(|| {
            format!(
                "paused take {:?} is the only episode available for physical scrubbing",
                self.episode
            )
        })
    }
}

impl SeekActivity {
    pub(super) fn new(recording: Option<ActiveRecording>, mode: SeekControlMode) -> Self {
        Self { recording, mode }
    }
}

impl SeekTarget {
    pub(super) fn new(id: impl Into<String>, robot_type: Option<String>, connected: bool) -> Self {
        Self {
            id: id.into(),
            robot_type,
            connected,
            calibrated: false,
        }
    }

    pub(super) const fn with_calibration(mut self, calibrated: bool) -> Self {
        self.calibrated = calibrated;
        self
    }

    fn is_follower(&self) -> bool {
        self.robot_type
            .as_deref()
            .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
    }
}

impl SeekableEpisode {
    pub(super) fn new(id: impl Into<String>, arms: Vec<String>, sample_hz: f64) -> Self {
        Self {
            id: id.into(),
            arms,
            sample_hz,
            problem: None,
            unfinished: false,
        }
    }

    pub(super) fn with_state(mut self, problem: Option<String>, unfinished: bool) -> Self {
        self.problem = problem;
        self.unfinished = unfinished;
        self
    }
}

impl SampledArm {
    pub(super) fn new(id: impl Into<String>, normalized: BTreeMap<String, Option<f64>>) -> Self {
        Self {
            id: id.into(),
            normalized,
        }
    }
}

impl RecordedSample {
    pub(super) fn new(seconds: f64, frame: u64, arms: Vec<SampledArm>) -> Self {
        Self {
            seconds,
            frame,
            arms,
        }
    }
}

impl PhysicalSeekPlan {
    fn new(ticket: u64, robot: impl Into<String>, episode: impl Into<String>) -> Self {
        Self {
            ticket,
            robot: robot.into(),
            episode: episode.into(),
            pose: BTreeMap::new(),
            sample_hz: 0.0,
        }
    }

    fn with_trajectory(mut self, pose: BTreeMap<String, f64>, sample_hz: f64) -> Self {
        self.pose = pose;
        self.sample_hz = sample_hz;
        self
    }

    pub(super) fn into_parts(self) -> (u64, String, String, BTreeMap<String, f64>, f64) {
        (
            self.ticket,
            self.robot,
            self.episode,
            self.pose,
            self.sample_hz,
        )
    }
}

impl RecordedSeekResult {
    fn new(state: RecordedSeekState, episode: impl Into<String>, robot: impl Into<String>) -> Self {
        Self {
            state,
            episode: episode.into(),
            robot: robot.into(),
            source_arm: String::new(),
            requested_seconds: 0.0,
            resolved_seconds: 0.0,
            frame: 0,
        }
    }

    fn with_source_arm(mut self, source_arm: impl Into<String>) -> Self {
        self.source_arm = source_arm.into();
        self
    }

    fn with_times(mut self, requested_seconds: f64, resolved_seconds: f64) -> Self {
        self.requested_seconds = requested_seconds;
        self.resolved_seconds = resolved_seconds;
        self
    }

    fn with_frame(mut self, frame: u64) -> Self {
        self.frame = frame;
        self
    }
}

pub(crate) trait PhysicalSeekPort {
    fn activity(&self) -> SeekActivity;
    async fn target(&self, robot: &str) -> Result<Option<SeekTarget>, OperationError>;
    async fn episode(&self, episode: &str) -> Result<SeekableEpisode, OperationError>;
    fn reserve_seek(&self, robot: &str) -> u64;

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<RecordedSample, OperationError>;

    /// Under the seek serialization gate, command the pose only if `ticket`
    /// is still current. Returning `false` means a newer scrub superseded it.
    async fn commit_if_current(&self, plan: PhysicalSeekPlan) -> Result<bool, OperationError>;
}

pub(crate) struct PhysicalSeekActions<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: PhysicalSeekPort + ?Sized> PhysicalSeekActions<'a, P> {
    pub(super) fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub(super) async fn seek(
        &self,
        command: SeekEpisodeCommand,
    ) -> Result<RecordedSeekResult, OperationError> {
        let port = self.port;
        if !command.seconds.is_finite() || command.seconds < 0.0 {
            return Err(OperationError::InvalidInput(
                "recorded seek time must be a finite, non-negative number of seconds".to_owned(),
            ));
        }

        let activity = port.activity();
        let paused_current = activity
            .recording
            .as_ref()
            .is_some_and(|recording| recording.paused && recording.episode == command.episode);
        if let Some(reason) = activity
            .recording
            .as_ref()
            .and_then(|recording| recording.seek_blocker(&command.episode))
        {
            return Err(OperationError::Conflict(reason));
        }
        if !matches!(
            activity.mode,
            SeekControlMode::Idle | SeekControlMode::Replay
        ) {
            return Err(OperationError::Conflict(
                "stop the active control mode before moving a physical arm through history"
                    .to_owned(),
            ));
        }

        let target = port.target(&command.robot).await?.ok_or_else(|| {
            OperationError::InvalidInput(format!("no robot named {:?}", command.robot))
        })?;
        if !target.is_follower() {
            return Err(OperationError::InvalidInput(format!(
                "recorded seek only drives an explicitly configured follower; robot {:?} is {:?}",
                command.robot, target.robot_type
            )));
        }
        if !target.connected {
            return Err(OperationError::Conflict(format!(
                "connect follower {:?} before seeking",
                command.robot
            )));
        }
        if !target.calibrated {
            return Err(OperationError::Conflict(format!(
                "calibrate follower {:?} before seeking",
                command.robot
            )));
        }

        let episode = port.episode(&command.episode).await?;
        if let Some(problem) = &episode.problem {
            let paused_live_partial =
                paused_current && episode.unfinished && problem == "recording did not finish";
            if !paused_live_partial {
                return Err(OperationError::InvalidInput(format!(
                    "take {:?} can be inspected but cannot drive hardware: {problem}",
                    command.episode
                )));
            }
        }
        if !episode.sample_hz.is_finite() || episode.sample_hz <= 0.0 {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} has an unusable sample rate of {} Hz",
                command.episode, episode.sample_hz
            )));
        }
        let source = RecordedArms::new(&episode.id, &episode.arms, &command.robot)
            .select(command.source_arm.as_deref())?;

        // Tickets are reserved before slow sample I/O. The adapter makes the last
        // ticket check and hardware handoff atomic under its serialization gate.
        let ticket = port.reserve_seek(&command.robot);
        let sample = port.sample_at(&command.episode, command.seconds).await?;
        if !sample.seconds.is_finite() || sample.seconds < 0.0 {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} returned an invalid recorded sample time",
                command.episode
            )));
        }
        let arm = sample
            .arms
            .iter()
            .find(|arm| arm.id == source)
            .ok_or_else(|| {
                OperationError::InvalidInput(format!(
                    "take {:?} did not record an arm named {source:?}",
                    command.episode
                ))
            })?;
        let pose = arm
            .normalized
            .iter()
            .filter_map(|(joint, value)| value.map(|value| (joint.clone(), value)))
            .collect::<BTreeMap<_, _>>();
        if pose.is_empty() {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} has no calibrated positions for {source:?} at {:.3}s",
                command.episode, sample.seconds
            )));
        }
        if pose.values().any(|value| !value.is_finite()) {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} has a non-finite calibrated position for {source:?} at {:.3}s",
                command.episode, sample.seconds
            )));
        }

        let committed = port
            .commit_if_current(
                PhysicalSeekPlan::new(ticket, command.robot.clone(), command.episode.clone())
                    .with_trajectory(pose, episode.sample_hz),
            )
            .await?;

        Ok(RecordedSeekResult::new(
            if committed {
                RecordedSeekState::Seeking
            } else {
                RecordedSeekState::Coalesced
            },
            command.episode,
            command.robot,
        )
        .with_source_arm(source)
        .with_times(command.seconds, sample.seconds)
        .with_frame(sample.frame))
    }
}

#[cfg(test)]
mod tests;
