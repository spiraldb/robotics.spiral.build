//! Historical physical-seek application action.
//!
//! Delivery supplies plain input. The action admits the motion, chooses the
//! recorded source arm, validates the sampled pose, and orders ticketing,
//! sampling, and the atomic current-ticket controller handoff.

use std::collections::BTreeMap;

use super::episode_arms::select_recorded_arm;
use super::error::ApplicationError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeekEpisodeCommand {
    pub episode: String,
    pub robot: String,
    pub source_arm: Option<String>,
    pub seconds: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeekControlMode {
    Idle,
    Replay,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveRecording {
    pub episode: String,
    pub paused: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeekActivity {
    pub recording: Option<ActiveRecording>,
    pub mode: SeekControlMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeekTarget {
    pub id: String,
    pub robot_type: Option<String>,
    pub connected: bool,
    pub calibrated: bool,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeekableEpisode {
    pub id: String,
    pub arms: Vec<String>,
    pub sample_hz: f64,
    pub problem: Option<String>,
    pub unfinished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SampledArm {
    pub id: String,
    pub normalized: BTreeMap<String, Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoricalSample {
    pub seconds: f64,
    pub frame: u64,
    pub arms: Vec<SampledArm>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhysicalSeekPlan {
    pub ticket: u64,
    pub robot: String,
    pub episode: String,
    pub pose: BTreeMap<String, f64>,
    pub sample_hz: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoricalSeekState {
    Seeking,
    Coalesced,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoricalSeekResult {
    pub state: HistoricalSeekState,
    pub episode: String,
    pub robot: String,
    pub source_arm: String,
    pub requested_seconds: f64,
    pub resolved_seconds: f64,
    pub frame: u64,
}

pub(crate) trait PhysicalSeekPort {
    fn activity(&self) -> SeekActivity;
    fn target(&self, robot: &str) -> Result<Option<SeekTarget>, ApplicationError>;
    fn episode(&self, episode: &str) -> Result<SeekableEpisode, ApplicationError>;
    fn reserve_seek(&self, robot: &str) -> u64;

    async fn sample_at(
        &self,
        episode: &str,
        seconds: f64,
    ) -> Result<HistoricalSample, ApplicationError>;

    /// Under the seek serialization gate, command the pose only if `ticket`
    /// is still current. Returning `false` means a newer scrub superseded it.
    async fn commit_if_current(&self, plan: PhysicalSeekPlan) -> Result<bool, ApplicationError>;
}

pub(crate) async fn seek<P: PhysicalSeekPort + ?Sized>(
    port: &P,
    command: SeekEpisodeCommand,
) -> Result<HistoricalSeekResult, ApplicationError> {
    if !command.seconds.is_finite() || command.seconds < 0.0 {
        return Err(ApplicationError::InvalidInput(
            "historical seek time must be a finite, non-negative number of seconds".to_owned(),
        ));
    }

    let activity = port.activity();
    let paused_current = activity
        .recording
        .as_ref()
        .is_some_and(|recording| recording.paused && recording.episode == command.episode);
    if let Some(reason) = recording_blocker(activity.recording.as_ref(), &command.episode) {
        return Err(ApplicationError::Conflict(reason));
    }
    if !matches!(
        activity.mode,
        SeekControlMode::Idle | SeekControlMode::Replay
    ) {
        return Err(ApplicationError::Conflict(
            "stop the active control mode before moving a physical arm through history".to_owned(),
        ));
    }

    let target = port.target(&command.robot)?.ok_or_else(|| {
        ApplicationError::InvalidInput(format!("no robot named {:?}", command.robot))
    })?;
    if !is_follower(target.robot_type.as_deref()) {
        return Err(ApplicationError::InvalidInput(format!(
            "historical seek only drives an explicitly configured follower; robot {:?} is {:?}",
            command.robot, target.robot_type
        )));
    }
    if !target.connected {
        return Err(ApplicationError::Conflict(format!(
            "connect follower {:?} before seeking",
            command.robot
        )));
    }
    if !target.calibrated {
        return Err(ApplicationError::Conflict(format!(
            "calibrate follower {:?} before seeking",
            command.robot
        )));
    }
    if let Some(problem) = target.problem {
        return Err(ApplicationError::Conflict(format!(
            "follower {:?} cannot be driven: {problem}",
            command.robot
        )));
    }

    let episode = port.episode(&command.episode)?;
    if let Some(problem) = &episode.problem {
        let paused_live_partial =
            paused_current && episode.unfinished && problem == "recording did not finish";
        if !paused_live_partial {
            return Err(ApplicationError::InvalidInput(format!(
                "take {:?} can be inspected but cannot drive hardware: {problem}",
                command.episode
            )));
        }
    }
    if !episode.sample_hz.is_finite() || episode.sample_hz <= 0.0 {
        return Err(ApplicationError::InvalidInput(format!(
            "take {:?} has an unusable sample rate of {} Hz",
            command.episode, episode.sample_hz
        )));
    }
    let source = select_recorded_arm(
        &episode.id,
        &episode.arms,
        &command.robot,
        command.source_arm.as_deref(),
    )?;

    // Tickets are reserved before slow sample I/O. The adapter makes the last
    // ticket check and hardware handoff atomic under its serialization gate.
    let ticket = port.reserve_seek(&command.robot);
    let sample = port.sample_at(&command.episode, command.seconds).await?;
    if !sample.seconds.is_finite() || sample.seconds < 0.0 {
        return Err(ApplicationError::InvalidInput(format!(
            "take {:?} returned an invalid historical sample time",
            command.episode
        )));
    }
    let arm = sample
        .arms
        .iter()
        .find(|arm| arm.id == source)
        .ok_or_else(|| {
            ApplicationError::InvalidInput(format!(
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
        return Err(ApplicationError::InvalidInput(format!(
            "take {:?} has no calibrated positions for {source:?} at {:.3}s",
            command.episode, sample.seconds
        )));
    }
    if pose.values().any(|value| !value.is_finite()) {
        return Err(ApplicationError::InvalidInput(format!(
            "take {:?} has a non-finite calibrated position for {source:?} at {:.3}s",
            command.episode, sample.seconds
        )));
    }

    let committed = port
        .commit_if_current(PhysicalSeekPlan {
            ticket,
            robot: command.robot.clone(),
            episode: command.episode.clone(),
            pose,
            sample_hz: episode.sample_hz,
        })
        .await?;

    Ok(HistoricalSeekResult {
        state: if committed {
            HistoricalSeekState::Seeking
        } else {
            HistoricalSeekState::Coalesced
        },
        episode: command.episode,
        robot: command.robot,
        source_arm: source,
        requested_seconds: command.seconds,
        resolved_seconds: sample.seconds,
        frame: sample.frame,
    })
}

fn recording_blocker(recording: Option<&ActiveRecording>, episode: &str) -> Option<String> {
    let recording = recording?;
    if !recording.paused {
        return Some(
            "pause the active recording before moving a physical arm through history".to_owned(),
        );
    }
    (recording.episode != episode).then(|| {
        format!(
            "paused take {:?} is the only episode available for physical scrubbing",
            recording.episode
        )
    })
}

fn is_follower(robot_type: Option<&str>) -> bool {
    robot_type.is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
}

#[cfg(test)]
mod tests;
