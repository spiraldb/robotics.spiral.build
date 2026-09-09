//! The single thread that owns every open motor bus.
//!
//! Serial traffic is blocking and a motor bus is exclusive, so all of it lives
//! on one dedicated OS thread running a fixed-rate loop: drain commands, read
//! every arm, let the active mode choose targets, write, publish. The HTTP
//! layer only ever sends a command and awaits a reply, so no request handler
//! can stall the control rate.
//!
//! Two safety rules are enforced here rather than in the UI, because the UI is
//! not the only thing that can send a request:
//!
//! * Joints are only ever commanded in calibrated, normalized units, so a
//!   command can never leave the travel the operator actually measured.
//! * Commanded positions ramp toward their target at a bounded rate, so no
//!   single request — and no teleoperation jump — can snap an arm across its
//!   workspace.

use spiral_robotics_common::recording_signals::{
    RecordingCursor, RecordingSignals, RecordingWindow,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(test)]
use crate::bus::SimulatedBus;
use crate::bus::{
    MotorBus, MotorFault as BusMotorFault, MotorFaults as BusMotorFaults,
    MotorLimits as BusMotorLimits, MotorSample,
};
use crate::feetech::FeetechBus;

use crate::HardwareInputError;
#[cfg(test)]
use crate::calibration::MotorDirection;
use crate::calibration::{
    Calibration, CalibrationError, CalibrationInput, MotorRangeTracker, MotorSpec,
    MotorSpecifications,
};
use crate::control_policy::{
    CalibrationAdmission, ControlMode, ControlState, Decision as ControlDecision,
    Effect as ControlEffect, Event as ControlEvent, Intent as ControlIntent,
    ReplayDecision as ReplayPolicyDecision, ReplayEvent as ReplayPolicyEvent,
    ReplayState as ReplayPolicyState, RobotAdmission,
};
use crate::state::CompletedRecording;
use spiral_robotics_common::recording::{
    EpisodeArm, RecordingClock as RecordingProgressClock, RecordingGap, RecordingIdentity,
    RecordingProgress, RecordingState, ReplayIdentity, ReplayPhase, ReplayProgress, ReplayPurpose,
    ReplayTimeline, ReplayTransport,
};
use spiral_robotics_common::robots::{
    CalibrationJointProgress, CalibrationProgress, ConnectionStatus, JointDiagnostics,
    JointIdentity, JointKinematics, JointRange, JointTelemetry, ModeSelection, ModeStatus,
    MotorElectricalReadings, MotorFault, MotorLimits, PortSettings, RobotTelemetry, TelemetryFrame,
};
use tokio::sync::{oneshot, watch};

/// Follow error at which an arm is no longer considered settled.
const CAUTION_FOLLOW_ERROR_DEGREES: f64 = 5.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmConditionSummary {
    arm: String,
    peak_temperature_celsius: Vec<Option<f64>>,
    min_voltage_volts: Vec<Option<f64>>,
    peak_load: Vec<Option<f64>>,
    peak_current_amps: Vec<Option<f64>>,
}

impl ArmConditionSummary {
    fn new(arm: String, joint_count: usize) -> Self {
        Self {
            arm,
            peak_temperature_celsius: vec![None; joint_count],
            min_voltage_volts: vec![None; joint_count],
            peak_load: vec![None; joint_count],
            peak_current_amps: vec![None; joint_count],
        }
    }

    pub fn arm(&self) -> &str {
        &self.arm
    }

    pub fn min_voltage_volts(&self) -> &[Option<f64>] {
        &self.min_voltage_volts
    }

    pub fn peak_temperature_celsius(&self) -> &[Option<f64>] {
        &self.peak_temperature_celsius
    }

    pub fn peak_load(&self) -> &[Option<f64>] {
        &self.peak_load
    }

    pub fn peak_current_amps(&self) -> &[Option<f64>] {
        &self.peak_current_amps
    }

    fn observe_motor(&mut self, slot: usize, track: &MotorTrack) {
        Self::retain_maximum(
            &mut self.peak_temperature_celsius,
            slot,
            track.sample.electrical().temperature_celsius(),
        );
        Self::retain_minimum(
            &mut self.min_voltage_volts,
            slot,
            track.sample.electrical().voltage_volts(),
        );
        Self::retain_maximum(&mut self.peak_load, slot, track.load.map(f64::abs));
        Self::retain_maximum(&mut self.peak_current_amps, slot, track.current);
    }

    /// Keep the more extreme of a slot's running value and a new reading.
    fn retain_maximum(slots: &mut [Option<f64>], slot: usize, reading: Option<f64>) {
        let Some(reading) = reading else { return };
        let Some(held) = slots.get_mut(slot) else {
            return;
        };
        *held = Some(held.map_or(reading, |held| held.max(reading)));
    }

    fn retain_minimum(slots: &mut [Option<f64>], slot: usize, reading: Option<f64>) {
        let Some(reading) = reading else { return };
        let Some(held) = slots.get_mut(slot) else {
            return;
        };
        *held = Some(held.map_or(reading, |held| held.min(reading)));
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    seconds: f64,
    normalized: Vec<Vec<Option<f64>>>,
    raw: Vec<Vec<i64>>,
    follow_error_degrees: Option<f64>,
    velocity: Option<Vec<Vec<Option<f64>>>>,
    effort: Option<Vec<Vec<Option<f64>>>>,
    action: Vec<Option<Vec<Option<f64>>>>,
}

impl Sample {
    pub fn new(seconds: f64, normalized: Vec<Vec<Option<f64>>>, raw: Vec<Vec<i64>>) -> Self {
        Self {
            seconds,
            normalized,
            raw,
            ..Self::default()
        }
    }

    pub fn with_follow_error_degrees(mut self, follow_error_degrees: Option<f64>) -> Self {
        self.follow_error_degrees = follow_error_degrees;
        self
    }

    pub fn with_velocity(mut self, velocity: Option<Vec<Vec<Option<f64>>>>) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn with_effort(mut self, effort: Option<Vec<Vec<Option<f64>>>>) -> Self {
        self.effort = effort;
        self
    }

    pub fn with_action(mut self, action: Vec<Option<Vec<Option<f64>>>>) -> Self {
        self.action = action;
        self
    }

    pub const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub fn normalized(&self) -> &[Vec<Option<f64>>] {
        &self.normalized
    }

    pub fn raw(&self) -> &[Vec<i64>] {
        &self.raw
    }

    pub const fn follow_error_degrees(&self) -> Option<f64> {
        self.follow_error_degrees
    }

    pub fn velocity(&self) -> Option<&[Vec<Option<f64>>]> {
        self.velocity.as_deref()
    }

    pub fn effort(&self) -> Option<&[Vec<Option<f64>>]> {
        self.effort.as_deref()
    }

    pub fn action(&self) -> &[Option<Vec<Option<f64>>>] {
        &self.action
    }

    pub fn replace_normalized_arm(
        &mut self,
        arm_index: usize,
        values: Vec<Option<f64>>,
    ) -> Result<(), Vec<Option<f64>>> {
        let Some(arm) = self.normalized.get_mut(arm_index) else {
            return Err(values);
        };
        *arm = values;
        Ok(())
    }
}

pub type EpisodeCheckpoint = oneshot::Receiver<Result<(), String>>;

/// Final timing and condition facts passed from hardware to episode storage.
pub struct RecordingCompletion {
    duration_seconds: f64,
    recording_gaps: Vec<RecordingGap>,
    max_follow_error_degrees: Option<f64>,
    condition: Vec<ArmConditionSummary>,
    discarded: bool,
}

impl RecordingCompletion {
    fn checkpoint(duration_seconds: f64, recording_gaps: Vec<RecordingGap>) -> Self {
        Self {
            duration_seconds,
            recording_gaps,
            max_follow_error_degrees: None,
            condition: Vec::new(),
            discarded: false,
        }
    }

    fn with_follow_error(mut self, max_follow_error_degrees: Option<f64>) -> Self {
        self.max_follow_error_degrees = max_follow_error_degrees;
        self
    }

    fn with_condition(mut self, condition: Vec<ArmConditionSummary>) -> Self {
        self.condition = condition;
        self
    }

    fn discard(mut self) -> Self {
        self.discarded = true;
        self
    }

    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    pub fn recording_gaps(&self) -> &[RecordingGap] {
        &self.recording_gaps
    }

    pub fn max_follow_error_degrees(&self) -> Option<f64> {
        self.max_follow_error_degrees
    }

    pub fn condition(&self) -> &[ArmConditionSummary] {
        &self.condition
    }

    pub fn discarded(&self) -> bool {
        self.discarded
    }
}

/// Recording boundary consumed by the fixed-rate hardware thread.
///
/// Persistence stays in the storage crate; the control runtime only owns the
/// sampling contract, which keeps edits on either side independently cached.
pub trait RecordingWriter: Send {
    fn update_clock(&self, _now: Instant, _elapsed: Option<Duration>) {}
    fn id(&self) -> &str;
    fn dataset_id(&self) -> &str;
    fn push(&self, sample: Sample) -> bool;
    fn checkpoint(&self, completion: RecordingCompletion) -> Result<EpisodeCheckpoint, String>;
    fn failure(&self) -> Option<String>;
    fn finish(
        self: Box<Self>,
        completion: RecordingCompletion,
    ) -> Result<CompletedRecording, String>;
}

/// How often the control loop runs.
pub const CONTROL_HZ: u64 = 60;
const CONTROL_PERIOD: Duration = Duration::from_nanos(1_000_000_000 / CONTROL_HZ);

/// Default ceiling on how far one joint may be commanded to move per cycle.
///
/// At the control rate above this is roughly 100 degrees per second, which is
/// brisk enough for teleoperation and slow enough to stay in front of.
pub const DEFAULT_MAX_STEP_COUNTS: i64 = 20;

/// Default teleoperation ceiling on follower movement per control cycle.
///
/// Manual slider moves stay deliberately gentle. A follower has to keep up
/// with ordinary hand motion, so it gets a faster, still-bounded ramp instead
/// of accumulating a large pose error behind the leader.
pub const DEFAULT_TELEOP_MAX_STEP_COUNTS: i64 = 40;

/// Consecutive failed transfers before a bus is considered lost.
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// How close every joint must get to a replay's requested frame before playback runs.
///
/// In normalized units, so this is a fraction of each joint's measured travel
/// rather than a count that means something different on every joint.
const REPLAY_SEEK_TOLERANCE: f64 = 0.02;

/// How long an arm may spend moving to a replay's requested pose before giving up.
///
/// A joint blocked by the bench, or by the operator's own hand, would otherwise
/// leave a replay waiting to start forever with an energized arm pushing
/// against whatever is in the way.
const REPLAY_SEEK_TIMEOUT: Duration = Duration::from_secs(20);

type Reply<T> = oneshot::Sender<Result<T, String>>;

/// Everything the control thread needs to open one arm.
#[derive(Clone, Debug)]
pub struct RobotSpec {
    id: String,
    port: PortSettings,
    motors: BTreeMap<String, MotorSpec>,
    calibration: Option<Calibration>,
}

impl RobotSpec {
    pub fn new(
        id: String,
        port: PortSettings,
        motors: BTreeMap<String, MotorSpec>,
    ) -> Result<Self, HardwareInputError> {
        HardwareInputError::validate_name("robot identifier", &id)?;
        if port.path().trim().is_empty() {
            return Err(HardwareInputError::new(
                "serial port path must not be empty",
            ));
        }
        if port.baud_rate() == 0 {
            return Err(HardwareInputError::new(
                "serial port baud rate must be greater than zero",
            ));
        }
        MotorSpec::validate_set(&motors)
            .map_err(|error| HardwareInputError::new(format!("invalid motor set: {error}")))?;
        Ok(Self {
            id,
            port,
            motors,
            calibration: None,
        })
    }

    pub fn with_calibration(
        mut self,
        calibration: Calibration,
    ) -> Result<Self, HardwareInputError> {
        calibration
            .validate_for_motor_specs(&self.motors)
            .map_err(|error| HardwareInputError::new(format!("invalid calibration: {error}")))?;
        self.calibration = Some(calibration);
        Ok(self)
    }
}

/// Tuning for the control loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSettings {
    period: Duration,
    /// Largest change in encoder counts applied to a joint in one cycle.
    max_step_counts: i64,
    /// Largest per-cycle change while an arm is the teleoperation follower.
    teleop_max_step_counts: i64,
}

impl ControlSettings {
    /// Override the ordinary and teleoperation motion limits while preserving
    /// the requirement that every control-cycle step is positive.
    pub fn with_motion_limits(
        mut self,
        max_step_counts: i64,
        teleop_max_step_counts: i64,
    ) -> Result<Self, HardwareInputError> {
        if max_step_counts <= 0 || teleop_max_step_counts <= 0 {
            return Err(HardwareInputError::new(
                "control-cycle motion limits must be greater than zero",
            ));
        }
        self.max_step_counts = max_step_counts;
        self.teleop_max_step_counts = teleop_max_step_counts;
        Ok(self)
    }
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            period: CONTROL_PERIOD,
            max_step_counts: DEFAULT_MAX_STEP_COUNTS,
            teleop_max_step_counts: DEFAULT_TELEOP_MAX_STEP_COUNTS,
        }
    }
}

/// A published sample of everything the control thread knows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HardwareState {
    telemetry: TelemetryFrame,
    /// Every robot the thread has been asked to connect, including ones whose
    /// bus has since been lost, so the reason stays visible.
    connections: BTreeMap<String, ConnectionStatus>,
    mode: ModeStatus,
    /// What each connected robot's motors said they will fault at.
    ///
    /// Keyed robot then motor. These ride the snapshot rather than every live
    /// joint sample because they do not change while an arm runs.
    limits: BTreeMap<String, BTreeMap<String, MotorLimits>>,
    /// Why the last take stopped on its own, kept until another one starts.
    recording_issue: Option<String>,
}

impl HardwareState {
    pub const fn sequence(&self) -> u64 {
        self.telemetry.sequence()
    }

    pub const fn loop_hz(&self) -> f64 {
        self.telemetry.loop_hz()
    }

    pub const fn target_hz(&self) -> f64 {
        self.telemetry.target_hz()
    }

    pub fn robots(&self) -> &[RobotTelemetry] {
        self.telemetry.robots()
    }

    pub const fn recording(&self) -> Option<&RecordingProgress> {
        self.telemetry.recording()
    }

    pub const fn replay(&self) -> Option<&ReplayProgress> {
        self.telemetry.replay()
    }

    pub const fn connections(&self) -> &BTreeMap<String, ConnectionStatus> {
        &self.connections
    }

    pub const fn mode(&self) -> &ModeSelection {
        self.mode.selection()
    }

    pub const fn mode_status(&self) -> &ModeStatus {
        &self.mode
    }

    pub const fn telemetry(&self) -> &TelemetryFrame {
        &self.telemetry
    }

    pub const fn mode_active(&self) -> bool {
        self.mode.active()
    }

    pub fn mode_error(&self) -> Option<&str> {
        self.mode.error()
    }

    pub const fn limits(&self) -> &BTreeMap<String, BTreeMap<String, MotorLimits>> {
        &self.limits
    }

    pub fn recording_issue(&self) -> Option<&str> {
        self.recording_issue.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointGoals(BTreeMap<String, f64>);

impl JointGoals {
    /// Compose a nonempty joint command from already validated positions.
    pub fn new(
        positions: BTreeMap<String, crate::NormalizedPosition>,
    ) -> Result<Self, HardwareInputError> {
        let values: BTreeMap<String, f64> = positions
            .into_iter()
            .map(|(joint, position)| (joint, position.get()))
            .collect();
        Self::try_from(values)
    }

    pub(crate) fn values(&self) -> &BTreeMap<String, f64> {
        &self.0
    }
}

impl AsRef<BTreeMap<String, f64>> for JointGoals {
    fn as_ref(&self) -> &BTreeMap<String, f64> {
        self.values()
    }
}

impl TryFrom<BTreeMap<String, f64>> for JointGoals {
    type Error = HardwareInputError;

    fn try_from(values: BTreeMap<String, f64>) -> Result<Self, Self::Error> {
        if values.is_empty() {
            return Err(HardwareInputError::new(
                "a joint command must contain at least one target",
            ));
        }
        for (joint, value) in &values {
            HardwareInputError::validate_name("joint name", joint)?;
            crate::NormalizedPosition::new(*value).map_err(|error| {
                HardwareInputError::new(format!("joint {joint:?} has an invalid target: {error}"))
            })?;
        }
        Ok(Self(values))
    }
}

#[derive(Debug, PartialEq)]
pub struct ReplayTrajectory {
    frames: Vec<BTreeMap<String, f64>>,
    sample_hz: f64,
}

impl ReplayTrajectory {
    pub fn new(
        frames: Vec<BTreeMap<String, f64>>,
        sample_hz: f64,
    ) -> Result<Self, HardwareInputError> {
        if frames.is_empty() {
            return Err(HardwareInputError::new(
                "a replay trajectory must contain at least one frame",
            ));
        }
        if !sample_hz.is_finite() || sample_hz <= 0.0 {
            return Err(HardwareInputError::new(
                "replay sample rate must be finite and greater than zero",
            ));
        }
        let expected_joints = frames[0]
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for (index, frame) in frames.iter().enumerate() {
            JointGoals::try_from(frame.clone())?;
            let actual_joints = frame.keys().map(String::as_str).collect::<BTreeSet<_>>();
            if actual_joints != expected_joints {
                return Err(HardwareInputError::new(format!(
                    "replay frame {index} changes the trajectory's joint set"
                )));
            }
        }
        Ok(Self { frames, sample_hz })
    }

    pub fn single_pose(
        pose: BTreeMap<String, f64>,
        sample_hz: f64,
    ) -> Result<Self, HardwareInputError> {
        Self::new(vec![pose], sample_hz)
    }
}

#[derive(Debug, PartialEq)]
pub struct RobotPlaybackRequest {
    robot: String,
    episode: String,
    trajectory: ReplayTrajectory,
    start_frame: usize,
    resolved_start_seconds: f64,
    purpose: ReplayPurpose,
}

/// One normalized command admitted under a specific policy session.
#[derive(Debug, PartialEq)]
pub struct PolicyGoals {
    robot: String,
    session: String,
    normalized: JointGoals,
}

impl PolicyGoals {
    pub fn new(
        robot: String,
        session: String,
        normalized: JointGoals,
    ) -> Result<Self, HardwareInputError> {
        HardwareInputError::validate_name("robot identifier", &robot)?;
        HardwareInputError::validate_name("policy session identifier", &session)?;
        Ok(Self {
            robot,
            session,
            normalized,
        })
    }
}

impl RobotPlaybackRequest {
    pub fn timeline(
        robot: String,
        episode: String,
        trajectory: ReplayTrajectory,
    ) -> Result<Self, HardwareInputError> {
        HardwareInputError::validate_name("robot identifier", &robot)?;
        HardwareInputError::validate_name("replay identifier", &episode)?;
        Ok(Self {
            robot,
            episode,
            trajectory,
            start_frame: 0,
            resolved_start_seconds: 0.0,
            purpose: ReplayPurpose::Timeline,
        })
    }

    pub fn physical_seek(
        robot: String,
        episode: String,
        trajectory: ReplayTrajectory,
    ) -> Result<Self, HardwareInputError> {
        Ok(Self {
            purpose: ReplayPurpose::PhysicalSeek,
            ..Self::timeline(robot, episode, trajectory)?
        })
    }

    pub fn starting_at(
        mut self,
        start_frame: usize,
        resolved_start_seconds: f64,
    ) -> Result<Self, HardwareInputError> {
        if start_frame >= self.trajectory.frames.len() {
            return Err(HardwareInputError::new(format!(
                "replay start frame {start_frame} is outside {} trajectory frames",
                self.trajectory.frames.len()
            )));
        }
        if !resolved_start_seconds.is_finite() || resolved_start_seconds < 0.0 {
            return Err(HardwareInputError::new(
                "replay start time must be finite and non-negative",
            ));
        }
        self.start_frame = start_frame;
        self.resolved_start_seconds = resolved_start_seconds;
        Ok(self)
    }
}

enum Command {
    Connect {
        spec: Box<RobotSpec>,
        reply: Reply<()>,
    },
    Disconnect {
        id: String,
        reply: Reply<()>,
    },
    Hold {
        id: String,
        reply: Reply<()>,
    },
    Release {
        id: String,
        reply: Reply<()>,
    },
    SetGoals {
        id: String,
        normalized: JointGoals,
        reply: Reply<()>,
    },
    SetPolicyGoals {
        goals: Box<PolicyGoals>,
        reply: Reply<()>,
    },
    StopPolicySession {
        session: String,
        reply: Reply<()>,
    },
    SetMode {
        mode: ModeSelection,
        reply: Reply<()>,
    },
    StartCalibration {
        id: String,
        reply: Reply<()>,
    },
    CalibrationInputFor {
        id: String,
        reply: Reply<CalibrationInput>,
    },
    Forget {
        id: String,
        reply: Reply<()>,
    },
    CancelCalibration {
        id: String,
        reply: Reply<()>,
    },
    ApplyCalibration {
        id: String,
        calibration: Box<Calibration>,
        reply: Reply<()>,
    },
    StartRecording {
        writer: Box<dyn RecordingWriter>,
        arms: Vec<EpisodeArm>,
        reply: Reply<()>,
    },
    StopRecording {
        keep: bool,
        reply: Reply<CompletedRecording>,
    },
    SetRecordingPaused {
        paused: bool,
        reply: Reply<(Option<EpisodeCheckpoint>, f64)>,
    },
    SetReplayPaused {
        paused: bool,
        reply: Reply<()>,
    },
    StartReplay {
        request: Box<RobotPlaybackRequest>,
        reply: Reply<()>,
    },
    Shutdown {
        reply: Reply<()>,
    },
}

/// A take being captured by the control loop.
struct Recording {
    writer: Box<dyn RecordingWriter>,
    /// Arms sampled, fixed when the take started so every frame has one shape.
    arms: Vec<EpisodeArm>,
    /// Control mode suspended for review and restored when capture resumes.
    resume_mode: ModeSelection,
    clock: RecordingClock,
    /// Samples handed to the writer.
    frames: u64,
    tracking: RecordingTrackingSummary,
    /// The worst each motor has seen so far, in the arms' joint order.
    ///
    /// Accumulated as the take runs so the footer can state it without a second
    /// pass over a take that may contain a hundred thousand samples.
    condition: Vec<ArmConditionSummary>,
}

/// Wall-clock accounting for an active recording, including deliberate gaps.
struct RecordingClock {
    started: Instant,
    /// Start of the current wall-clock omission, when capture is paused.
    paused_at: Option<Instant>,
    /// Completed pause time omitted from the active sample clock.
    paused_duration: Duration,
    /// Deliberate wall-clock omissions, persisted with the finished take.
    gaps: Vec<RecordingGap>,
}

/// Tracking-quality extrema accumulated while a recording is active.
#[derive(Default)]
struct RecordingTrackingSummary {
    /// Largest `Follow Δ` since the arm first caught up, in degrees.
    ///
    /// A recording almost always opens with the arm travelling — to a fresh
    /// manual goal, or to wherever the leader is standing. That transient is
    /// real, stays in the recorded samples, and says nothing about how well the take
    /// tracked. Summarising it would rank every take by how far its arm had to
    /// move before the work started, which is why measurement waits until the
    /// arm has been inside [`CAUTION_FOLLOW_ERROR_DEGREES`] at least once.
    max_follow_error: Option<f64>,
    /// True once the arm has caught up with what it was told, at least once.
    settled: bool,
    /// The worst reading from before that happened.
    ///
    /// Reported only if the arm never caught up at all: a take that never
    /// tracked has no settled period to measure, and saying nothing about it
    /// would make the worst takes look like the ones with no measurement.
    unsettled_peak: Option<f64>,
}

impl RecordingClock {
    fn started_now() -> Self {
        Self {
            started: Instant::now(),
            paused_at: None,
            paused_duration: Duration::ZERO,
            gaps: Vec::new(),
        }
    }

    /// Time represented by samples and media, excluding deliberate pauses.
    fn active_elapsed(&self, now: Instant) -> Duration {
        let current_pause = self
            .paused_at
            .map_or(Duration::ZERO, |paused_at| now.duration_since(paused_at));
        now.duration_since(self.started)
            .saturating_sub(self.paused_duration)
            .saturating_sub(current_pause)
    }

    /// All wall time deliberately omitted so far, including an active pause.
    fn paused_elapsed(&self, now: Instant) -> Duration {
        self.paused_duration
            + self
                .paused_at
                .map_or(Duration::ZERO, |paused_at| now.duration_since(paused_at))
    }

    /// Close an active pause and retain its active-time boundary and duration.
    fn close_pause(&mut self, now: Instant) {
        let Some(paused_at) = self.paused_at.take() else {
            return;
        };
        let duration = now.duration_since(paused_at);
        let after_seconds = now
            .duration_since(self.started)
            .saturating_sub(self.paused_duration)
            .saturating_sub(duration)
            .as_secs_f64();
        self.paused_duration += duration;
        self.gaps
            .push(RecordingGap::new(after_seconds, duration.as_secs_f64()));
    }

    fn pause(&mut self, now: Instant) {
        self.paused_at.get_or_insert(now);
    }

    fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    fn wall_elapsed(&self, now: Instant) -> Duration {
        now.duration_since(self.started)
    }
}

impl RecordingTrackingSummary {
    /// Fold one instant's tracking error into the take's summary.
    fn observe_follow_error(&mut self, error: f64) {
        if !self.settled && error < CAUTION_FOLLOW_ERROR_DEGREES {
            self.settled = true;
        }
        let slot = if self.settled {
            &mut self.max_follow_error
        } else {
            &mut self.unsettled_peak
        };
        *slot = Some(slot.map_or(error, |worst: f64| worst.max(error)));
    }

    fn follow_error(&self) -> Option<f64> {
        self.max_follow_error.or(self.unsettled_peak)
    }
}

impl Recording {
    fn active_elapsed(&self, now: Instant) -> Duration {
        self.clock.active_elapsed(now)
    }

    fn paused_elapsed(&self, now: Instant) -> Duration {
        self.clock.paused_elapsed(now)
    }

    fn close_pause(&mut self, now: Instant) {
        self.clock.close_pause(now);
    }

    fn follow_error(&self) -> Option<f64> {
        self.tracking.follow_error()
    }
}

/// An episode being played back onto one arm.
struct Replay {
    episode: String,
    robot: String,
    frames: Vec<BTreeMap<String, f64>>,
    /// Seconds between recorded frames, so playback runs at the rate it was
    /// captured at rather than at whatever the control loop happens to manage.
    frame_period: f64,
    purpose: ReplayPurpose,
    phase: ReplayPhase,
    timing: ReplayTiming,
}

/// Mutable playback clock and pause/seek bookkeeping for one replay.
struct ReplayTiming {
    /// Fractional index into `frames`, advanced by real elapsed time.
    position: f64,
    /// Frame position and episode time are not interchangeable after frames
    /// without commandable poses have been filtered out.
    start_position: f64,
    resolved_start_seconds: f64,
    /// A paused replay keeps its loaded trajectory and current position. The
    /// arm holds where it is until the same replay resumes.
    paused: bool,
    /// When the seek started, so a blocked arm gives up instead of pushing.
    seeking_since: Instant,
}

impl Replay {
    /// The pose this replay wants the arm at right now.
    fn pose(&self) -> Option<&BTreeMap<String, f64>> {
        self.frames.get(self.cursor())
    }

    fn cursor(&self) -> usize {
        self.timing.position as usize
    }

    fn finished(&self) -> bool {
        self.phase == ReplayPhase::Playing && self.cursor() >= self.frames.len()
    }

    fn duration_seconds(&self) -> f64 {
        self.frames.len() as f64 * self.frame_period
    }

    fn elapsed_seconds(&self) -> f64 {
        self.timing.resolved_start_seconds
            + (self.timing.position - self.timing.start_position).max(0.0) * self.frame_period
    }
}

/// The asynchronous side of the control thread.
#[derive(Debug)]
pub struct HardwareHandle {
    commands: Sender<Command>,
    state: watch::Receiver<HardwareState>,
    recording_signals: Arc<RwLock<RecordingSignals>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl HardwareHandle {
    /// Start the control thread.
    pub fn spawn(settings: ControlSettings) -> Self {
        let (commands, requests) = mpsc::channel();
        let (state, receiver) = watch::channel(HardwareState::default());
        let controller = Controller::new(settings, state);
        let recording_signals = controller.recording_signals.clone();
        let thread = thread::Builder::new()
            .name("spiral-robotics-control".to_owned())
            .spawn(move || controller.run(&requests))
            .expect("the control thread must start");

        Self {
            commands,
            state: receiver,
            recording_signals,
            thread: Mutex::new(Some(thread)),
        }
    }

    /// Read the control thread's most recent state.
    pub fn state(&self) -> HardwareState {
        self.state.borrow().clone()
    }

    pub fn recording_signals(&self, cursor: RecordingCursor) -> RecordingWindow {
        self.recording_signals
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .window(cursor)
    }
    pub fn clear_recording_signals(&self) {
        self.recording_signals
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    /// Observe authoritative states published after control-thread commands.
    pub fn subscribe(&self) -> watch::Receiver<HardwareState> {
        self.state.clone()
    }

    /// Open a robot's motor bus.
    pub async fn connect(&self, spec: RobotSpec) -> Result<(), String> {
        self.request(|reply| Command::Connect {
            spec: Box::new(spec),
            reply,
        })
        .await
    }

    /// Close a robot's motor bus.
    pub async fn disconnect(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Disconnect {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Enter manual mode and engage holding torque as one serialized action.
    pub async fn hold(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Hold {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Stop active motion and release the named robot as one serialized action.
    pub async fn release(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Release {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Command joint targets in normalized units.
    pub async fn set_goals(&self, id: &str, normalized: JointGoals) -> Result<(), String> {
        self.request(|reply| Command::SetGoals {
            id: id.to_owned(),
            normalized,
            reply,
        })
        .await
    }

    /// Command normalized targets for the currently admitted policy session.
    pub async fn set_policy_goals(&self, goals: PolicyGoals) -> Result<(), String> {
        self.request(|reply| Command::SetPolicyGoals {
            goals: Box::new(goals),
            reply,
        })
        .await
    }

    /// Leave policy mode only when the named session still owns it.
    pub async fn stop_policy_session(&self, session: &str) -> Result<(), String> {
        self.request(|reply| Command::StopPolicySession {
            session: session.to_owned(),
            reply,
        })
        .await
    }

    /// Select the operating mode.
    pub async fn set_mode(&self, mode: ModeSelection) -> Result<(), String> {
        self.request(|reply| Command::SetMode { mode, reply }).await
    }

    /// Disable torque and begin recording a joint-range sweep.
    pub async fn start_calibration(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::StartCalibration {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Read the ranges the sweep has recorded so far.
    ///
    /// This deliberately leaves the sweep running. The caller has to build,
    /// validate, and store a calibration from the result, and any of those can
    /// fail — discarding the operator's sweep before then would throw away
    /// minutes of work that is still perfectly good.
    pub async fn calibration_input(&self, id: &str) -> Result<CalibrationInput, String> {
        self.request(|reply| Command::CalibrationInputFor {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Drop every trace of a robot, including why its bus was lost.
    pub async fn forget(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Forget {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Abandon a sweep without producing a calibration.
    pub async fn cancel_calibration(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::CancelCalibration {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Begin sampling the named arms into an already-opened take.
    ///
    /// The writer is created by the caller at the Spiral storage boundary
    /// and handed over whole because only this thread runs at the rate a take
    /// has to be sampled at.
    pub async fn start_recording<W: RecordingWriter + 'static>(
        &self,
        writer: W,
        arms: Vec<EpisodeArm>,
    ) -> Result<(), String> {
        self.request(|reply| Command::StartRecording {
            writer: Box::new(writer),
            arms,
            reply,
        })
        .await
    }

    /// Finish the take in progress and return what was stored.
    pub async fn stop_recording(&self, keep: bool) -> Result<CompletedRecording, String> {
        self.request(|reply| Command::StopRecording { keep, reply })
            .await
    }

    /// Pause or resume sampling while keeping the same open take and ID.
    pub async fn set_recording_paused(&self, paused: bool) -> Result<f64, String> {
        let (checkpoint, elapsed_seconds) = self
            .request(|reply| Command::SetRecordingPaused { paused, reply })
            .await?;
        if let Some(checkpoint) = checkpoint {
            checkpoint.await.map_err(|_| {
                "the episode writer stopped before the pause was flushed".to_owned()
            })??;
        }
        Ok(elapsed_seconds)
    }

    /// Play or physically seek through a validated recorded trajectory.
    pub async fn start_replay(&self, request: RobotPlaybackRequest) -> Result<(), String> {
        self.request(|reply| Command::StartReplay {
            request: Box::new(request),
            reply,
        })
        .await
    }

    /// Pause or resume the loaded replay without returning to frame zero.
    pub async fn set_replay_paused(&self, paused: bool) -> Result<(), String> {
        self.request(|reply| Command::SetReplayPaused { paused, reply })
            .await
    }

    /// Adopt a calibration that has just been committed to SpiralDB.
    pub async fn apply_calibration(
        &self,
        id: &str,
        calibration: Calibration,
    ) -> Result<(), String> {
        self.request(|reply| Command::ApplyCalibration {
            id: id.to_owned(),
            calibration: Box::new(calibration),
            reply,
        })
        .await
    }

    /// Stop the control thread and wait for it to finish.
    pub async fn shutdown(&self) -> Result<(), String> {
        let cleanup = self.request(|reply| Command::Shutdown { reply }).await;
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|error| format!("hardware control thread join failed: {error}"))?
                .map_err(|_| "hardware control thread panicked during shutdown".to_owned())?;
        }
        cleanup
    }

    async fn request<T, F>(&self, build: F) -> Result<T, String>
    where
        F: FnOnce(Reply<T>) -> Command,
    {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(build(reply))
            .map_err(|_| "the control thread is not running".to_owned())?;
        response
            .await
            .map_err(|_| "the control thread stopped before answering".to_owned())?
    }
}

/// One arm the control thread currently holds open.
/// Weight a new sample carries when smoothing a continuously varying reading.
///
/// The same trick the loop-rate measurement uses above, and for the same
/// reason: load and speed are noisy sample to sample, and a readout that
/// flickers through three values a frame is a readout nobody can read.
const READING_SMOOTHING: f64 = 0.2;

/// How much of a held peak survives one control cycle.
///
/// The loop samples at [`CONTROL_HZ`] but publishes to the interface at a
/// third of that, so two samples in three never reach a client. Load and
/// current are exactly the readings where the sample that gets dropped is the
/// one worth seeing — a stall is over in tens of milliseconds. Holding the peak
/// and letting it fall away over about three cycles keeps a real transient
/// visible for one frame without pinning the readout at a spike forever.
const PEAK_HOLD_DECAY: f64 = 0.79;

/// How far back the temperature-rise reading looks.
///
/// The registers report whole degrees, so a difference taken between two frames
/// is quantization noise with a unit attached: one degree across a 50ms frame
/// reads as 1,200 degrees a minute. A minute of history buys a resolution of
/// one degree per minute, which is the number an operator can actually act on.
const RISE_WINDOW: Duration = Duration::from_secs(60);

/// Spacing between the temperature samples kept for the rise reading.
///
/// Thirteen samples per motor covers the window; keeping every cycle would be
/// three thousand six hundred of them to answer the same question.
const RISE_SAMPLE_SPACING: Duration = Duration::from_secs(5);

/// One motor's live readings, smoothed and peak-held for display.
#[derive(Clone, Debug, Default)]
struct MotorTrack {
    sample: MotorSample,
    speed: Option<f64>,
    load: Option<f64>,
    current: Option<f64>,
    /// Sparse temperature history, oldest first, spanning [`RISE_WINDOW`].
    history: VecDeque<(Instant, f64)>,
}

impl MotorTrack {
    fn observe(&mut self, sample: MotorSample, now: Instant) {
        let mechanical = sample.mechanical();
        let electrical = sample.electrical();
        self.speed = Self::smoothed_reading(self.speed, mechanical.speed_degrees_per_second());
        self.load = Self::held_peak(
            self.load,
            Self::smoothed_reading(self.load, mechanical.load()),
            f64::abs,
        );
        self.current = Self::held_peak(
            self.current,
            Self::smoothed_reading(self.current, electrical.current_amps()),
            |amps| amps,
        );
        if let Some(celsius) = electrical.temperature_celsius() {
            self.record_temperature(celsius, now);
        }
        self.sample = sample;
    }

    fn record_temperature(&mut self, celsius: f64, now: Instant) {
        let due = self
            .history
            .back()
            .is_none_or(|(when, _)| now.duration_since(*when) >= RISE_SAMPLE_SPACING);
        if !due {
            return;
        }
        self.history.push_back((now, celsius));
        // Keep one sample older than the window so the span it measures is the
        // whole window rather than whatever is left after the last eviction.
        while self
            .history
            .get(1)
            .is_some_and(|(when, _)| now.duration_since(*when) > RISE_WINDOW)
        {
            self.history.pop_front();
        }
    }

    /// Degrees per minute over the retained window, once it has filled.
    fn temperature_rise_per_minute(&self) -> Option<f64> {
        let (first, oldest) = self.history.front().copied()?;
        let (last, newest) = self.history.back().copied()?;
        let elapsed = last.duration_since(first);
        if elapsed < RISE_WINDOW {
            return None;
        }
        Some((newest - oldest) / (elapsed.as_secs_f64() / 60.0))
    }

    /// Fold a reading into its smoothed predecessor, starting from the reading.
    fn smoothed_reading(previous: Option<f64>, next: Option<f64>) -> Option<f64> {
        let next = next?;
        Some(match previous {
            Some(previous) if previous.is_finite() => {
                previous + (next - previous) * READING_SMOOTHING
            }
            _ => next,
        })
    }

    /// Keep the larger of a decaying held peak and the current reading.
    fn held_peak(
        held: Option<f64>,
        next: Option<f64>,
        magnitude: impl Fn(f64) -> f64,
    ) -> Option<f64> {
        let next = next?;
        match held {
            Some(held) if magnitude(held * PEAK_HOLD_DECAY) > magnitude(next) => {
                Some(held * PEAK_HOLD_DECAY)
            }
            _ => Some(next),
        }
    }
}

struct ConnectedRobot {
    spec: RobotSpec,
    bus: Box<dyn MotorBus>,
    motion: MotorMotion,
    /// What each motor reports about itself, smoothed for display.
    tracks: BTreeMap<String, MotorTrack>,
    /// What each motor said it will fault at, read once when it connected.
    limits: BTreeMap<String, MotorLimits>,
    health: BusHealth,
    sweep: Option<MotorRangeTracker>,
}

/// Restate a bus-level limit on the published wire boundary.
impl From<BusMotorLimits> for MotorLimits {
    fn from(limits: BusMotorLimits) -> Self {
        Self::default()
            .with_max_temperature_celsius(limits.max_temperature_celsius())
            .with_voltage_range(limits.min_voltage_volts(), limits.max_voltage_volts())
            .with_max_load(limits.max_load())
    }
}

impl From<BusMotorFaults> for Vec<MotorFault> {
    fn from(faults: BusMotorFaults) -> Self {
        let mut named = faults
            .known_faults()
            .into_iter()
            .map(|fault| match fault {
                BusMotorFault::Overheat => MotorFault::Overheat,
                BusMotorFault::Overload => MotorFault::Overload,
                BusMotorFault::Voltage => MotorFault::Voltage,
            })
            .collect::<Vec<_>>();
        if faults.unknown_bits() != 0 {
            named.push(MotorFault::Unrecognized);
        }
        named
    }
}

/// Measured and commanded motor state that must move through the loop together.
struct MotorMotion {
    torque: TorqueState,
    positions: BTreeMap<String, i64>,
    /// Where each joint should end up.
    target: BTreeMap<String, i64>,
    /// The last goal actually written, which ramps toward `target`.
    commanded: BTreeMap<String, i64>,
}

/// Communication failures accumulated for one open motor bus.
#[derive(Default)]
struct BusHealth {
    error: Option<String>,
    failures: u32,
}

/// What the controller has actually established about physical holding torque.
/// Unknown is deliberately distinct from released: neither may drive, but only
/// a successful torque-off transition is allowed to claim release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TorqueState {
    Released,
    Engaged,
    Unknown,
}

impl TorqueState {
    const fn is_engaged(self) -> bool {
        matches!(self, Self::Engaged)
    }
}

impl ConnectedRobot {
    /// Pin intent to measured reality, so energizing never causes a jump.
    fn follow_measured_positions(&mut self) {
        self.motion.target = self.motion.positions.clone();
        self.motion.commanded = self.motion.positions.clone();
    }

    /// Step every commanded goal toward its target and write what changed.
    fn drive(&mut self, max_step: i64) {
        if !self.motion.torque.is_engaged() || self.motion.target.is_empty() {
            return;
        }

        let mut writes = BTreeMap::new();
        for (name, target) in &self.motion.target {
            let current = self
                .motion
                .commanded
                .get(name)
                .or_else(|| self.motion.positions.get(name))
                .copied()
                .unwrap_or(*target);
            let step = target.saturating_sub(current).clamp(-max_step, max_step);
            writes.insert(name.clone(), current.saturating_add(step));
        }

        if writes == self.motion.commanded {
            return;
        }
        match self.bus.write_goal_positions(&writes) {
            Ok(()) => {
                self.motion.commanded = writes;
                self.health.failures = 0;
            }
            Err(error) => {
                self.health.failures += 1;
                self.health.error = Some(error.to_string());
            }
        }
    }

    /// The robot's motors in bus-ID order, which for an SO-101 is also the
    /// order the joints appear along the arm.
    fn ordered_motors(&self) -> Vec<(&String, &MotorSpec)> {
        let mut motors = self.spec.motors.iter().collect::<Vec<_>>();
        motors.sort_by(|(left_name, left), (right_name, right)| {
            left.id()
                .cmp(&right.id())
                .then_with(|| left_name.cmp(right_name))
        });
        motors
    }

    /// This instant's positions for the named joints, normalized and raw.
    ///
    /// The caller supplies the order, which comes from published telemetry, so a
    /// sample in the take lines up with the joint rows the operator was watching
    /// while it was captured.
    fn sample(&self, joints: &[String]) -> (Vec<Option<f64>>, Vec<i64>) {
        let calibration = self.spec.calibration.as_ref();
        let mut normalized = Vec::with_capacity(joints.len());
        let mut raw = Vec::with_capacity(joints.len());
        for joint in joints {
            let counts = *self
                .motion
                .positions
                .get(joint)
                .expect("accepted motor reads contain every configured joint");
            raw.push(counts);
            normalized.push(
                calibration
                    .and_then(|calibration| calibration.motors().get(joint))
                    .and_then(|motor| motor.normalize_raw(counts).ok()),
            );
        }
        (normalized, raw)
    }

    /// The normalized goal most recently sent to each named joint.
    ///
    /// A free arm has no action. Returning `None` for the whole arm preserves
    /// the difference between an observed leader pose and a follower command.
    fn sample_action(&self, joints: &[String]) -> Option<Vec<Option<f64>>> {
        if !self.motion.torque.is_engaged() {
            return None;
        }
        let calibration = self.spec.calibration.as_ref()?;
        Some(
            joints
                .iter()
                .map(|joint| {
                    let counts = self.motion.commanded.get(joint).copied()?;
                    let motor = calibration.motors().get(joint)?;
                    motor.normalize_raw(counts).ok()
                })
                .collect(),
        )
    }

    /// This instant's velocity and effort for the named joints.
    ///
    /// Both come back in the caller's order, and both are `None` per joint on a
    /// bus that does not publish them — the caller decides what an incomplete
    /// frame means for the take as a whole.
    fn sample_proprioception(&self, joints: &[String]) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
        joints
            .iter()
            .map(|joint| {
                let track = self.tracks.get(joint);
                (
                    track.and_then(|track| track.speed),
                    track.and_then(|track| track.load),
                )
            })
            .unzip()
    }

    /// The largest gap between a measured and a commanded joint angle, in
    /// degrees — the quantity the inspector shows live as `Follow Δ`.
    ///
    /// `None` when nothing is being commanded, because there is no target to be
    /// away from.
    fn follow_error_degrees(&self) -> Option<f64> {
        if !self.motion.torque.is_engaged() {
            return None;
        }
        let calibration = self.spec.calibration.as_ref()?;
        self.motion
            .target
            .iter()
            .filter_map(|(name, target)| {
                let motor = calibration.motors().get(name)?;
                let measured = self.motion.positions.get(name).copied()?;
                let error = (motor.degrees_from_center(measured)
                    - motor.degrees_from_center(*target))
                .abs();
                error.is_finite().then_some(error)
            })
            .reduce(f64::max)
    }

    fn telemetry(&self) -> RobotTelemetry {
        let calibration = self.spec.calibration.as_ref();
        let joints = self
            .ordered_motors()
            .into_iter()
            .map(|(name, spec)| {
                let raw = *self
                    .motion
                    .positions
                    .get(name)
                    .expect("accepted motor reads contain every configured joint");
                let motor = calibration.and_then(|calibration| calibration.motors().get(name));
                let track = self.tracks.get(name);
                let target_raw = self
                    .motion
                    .torque
                    .is_engaged()
                    .then(|| self.motion.target.get(name).copied())
                    .flatten();
                let goal_raw = self
                    .motion
                    .torque
                    .is_engaged()
                    .then(|| self.motion.commanded.get(name).copied())
                    .flatten();
                let kinematics = JointKinematics::new(
                    raw,
                    motor.map(|motor| motor.degrees_from_center(raw)),
                    motor.and_then(|motor| motor.normalize_raw(raw).ok()),
                )
                .with_raw_targets(goal_raw, target_raw)
                .with_calibrated_target(
                    target_raw
                        .zip(motor)
                        .map(|(target, motor)| motor.degrees_from_center(target)),
                    motor.map(|motor| {
                        JointRange::new(
                            motor.reading().range_min(),
                            motor.reading().center(),
                            motor.reading().range_max(),
                        )
                    }),
                );
                let electrical = MotorElectricalReadings::new(
                    track.and_then(|track| track.sample.electrical().temperature_celsius()),
                    track.and_then(|track| track.sample.electrical().voltage_volts()),
                    track.and_then(|track| track.current),
                );
                let diagnostics = JointDiagnostics::new(
                    electrical,
                    track.and_then(|track| track.load),
                    track.and_then(|track| track.speed),
                )
                .with_temperature_rise(track.and_then(MotorTrack::temperature_rise_per_minute))
                .with_faults(
                    track
                        .map(|track| Vec::from(track.sample.faults()))
                        .unwrap_or_default(),
                );
                let condition =
                    diagnostics.assess(self.limits.get(name).copied().unwrap_or_default());
                JointTelemetry::new(
                    JointIdentity::new(name.clone(), spec.id()),
                    kinematics,
                    diagnostics,
                )
                .with_condition(condition)
            })
            .collect();

        let calibration = self.sweep.as_ref().map(|sweep| {
            let joints = self
                .ordered_motors()
                .into_iter()
                .filter_map(|(name, _)| Some((name, sweep.ranges().get(name)?)))
                .map(|(name, range)| {
                    CalibrationJointProgress::new(name.clone(), range.minimum(), range.position())
                        .with_max(range.maximum())
                        .with_encoder_counts_per_turn(MotorSpec::encoder_counts_per_turn())
                        .with_ready(
                            range.maximum().saturating_sub(range.minimum())
                                >= crate::calibration::MINIMUM_CALIBRATION_TRAVEL,
                        )
                })
                .collect();
            CalibrationProgress::new(joints, sweep.is_ready())
        });
        RobotTelemetry::new(
            self.spec.id.clone(),
            self.motion.torque.is_engaged(),
            joints,
        )
        .with_tracking_lagging(
            self.follow_error_degrees()
                .is_some_and(|error| error >= CAUTION_FOLLOW_ERROR_DEGREES),
        )
        .with_error(self.health.error.clone())
        .with_calibration(calibration)
    }
}

/// Translate the delivery contract into the framework-neutral ownership model.
impl From<&ModeSelection> for ControlMode {
    fn from(selection: &ModeSelection) -> Self {
        match selection {
            ModeSelection::Idle => ControlMode::Idle,
            ModeSelection::Manual { robot } => ControlMode::Manual {
                robot: robot.clone(),
            },
            ModeSelection::Teleoperate { leader, follower } => ControlMode::Teleoperate {
                leader: leader.clone(),
                follower: follower.clone(),
            },
            ModeSelection::Replay { robot, episode } => ControlMode::Replay {
                robot: robot.clone(),
                episode: episode.clone(),
            },
            ModeSelection::Policy { robot, session } => ControlMode::Policy {
                robot: robot.clone(),
                session: session.clone(),
            },
        }
    }
}

/// Translate committed application state back onto the published API contract.
impl From<ControlMode> for ModeSelection {
    fn from(mode: ControlMode) -> Self {
        match mode {
            ControlMode::Idle => ModeSelection::Idle,
            ControlMode::Manual { robot } => ModeSelection::Manual { robot },
            ControlMode::Teleoperate { leader, follower } => {
                ModeSelection::Teleoperate { leader, follower }
            }
            ControlMode::Replay { robot, episode } => ModeSelection::Replay { robot, episode },
            ControlMode::Policy { robot, session } => ModeSelection::Policy { robot, session },
        }
    }
}

impl From<ReplayPhase> for ReplayPolicyState {
    fn from(phase: ReplayPhase) -> Self {
        match phase {
            ReplayPhase::Seeking => Self::Seeking,
            ReplayPhase::Playing => Self::Playing,
        }
    }
}

struct Controller {
    settings: ControlSettings,
    robots: BTreeMap<String, ConnectedRobot>,
    published: PublishedControllerState,
    state: watch::Sender<HardwareState>,
    recording_signals: Arc<RwLock<RecordingSignals>>,
    operations: ActiveOperations,
    #[cfg(test)]
    use_test_bus: bool,
}

#[derive(Default)]
struct ActiveOperations {
    /// The take being captured, if any. Recording is deliberately not a mode:
    /// an operator records *while* teleoperating, and a take of an idle arm
    /// being moved by hand is a legitimate take too.
    recording: Option<Recording>,
    /// The episode being played back, if any. This one *is* a mode, because it
    /// drives motors and only one thing may do that at a time.
    replay: Option<Replay>,
}

/// Controller-owned values copied into each published hardware snapshot.
#[derive(Default)]
struct PublishedControllerState {
    /// Connection state for every robot the thread has been asked about,
    /// including ones whose bus was lost.
    connections: BTreeMap<String, ConnectionStatus>,
    mode: ModeStatus,
    sequence: u64,
    loop_hz: f64,
    recording_issue: Option<String>,
    /// Retain chart samples at 20 Hz independently of browser polling.
    last_chart_sample: Option<Instant>,
}

impl Controller {
    fn new(settings: ControlSettings, state: watch::Sender<HardwareState>) -> Self {
        Self {
            settings,
            robots: BTreeMap::new(),
            published: PublishedControllerState::default(),
            state,
            operations: ActiveOperations::default(),
            recording_signals: Arc::new(RwLock::new(RecordingSignals::default())),
            #[cfg(test)]
            use_test_bus: false,
        }
    }

    #[cfg(test)]
    fn new_for_test(settings: ControlSettings, state: watch::Sender<HardwareState>) -> Self {
        let mut controller = Self::new(settings, state);
        controller.use_test_bus = true;
        controller
    }

    fn run(mut self, requests: &mpsc::Receiver<Command>) {
        let mut previous_tick: Option<Instant> = None;
        // Use an absolute schedule. Sleeping for `period - work` every time
        // permanently adds the platform's sleep overshoot to every cycle (on
        // macOS that made an empty fixed-rate loop run noticeably slow).
        // Carrying the deadline forward lets the following sleep absorb that
        // overshoot.
        let mut next_tick = Instant::now() + self.settings.period;
        loop {
            let started = Instant::now();
            if let Some(previous) = previous_tick.replace(started) {
                let elapsed = started.duration_since(previous).as_secs_f64();
                if elapsed > 0.0 {
                    // Smooth cycle duration and invert it afterwards. Averaging
                    // instantaneous rates biases the result high when an
                    // absolute schedule alternates a slightly long cycle with
                    // a slightly short one.
                    self.published.loop_hz = if self.published.loop_hz == 0.0 {
                        1.0 / elapsed
                    } else {
                        1.0 / (0.9 / self.published.loop_hz + 0.1 * elapsed)
                    };
                }
            }

            loop {
                match requests.try_recv() {
                    Ok(Command::Shutdown { reply }) => {
                        let _ = reply.send(self.shutdown_all());
                        return;
                    }
                    Ok(command) => self.handle(command),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if let Err(error) = self.shutdown_all() {
                            tracing::error!(
                                "hardware request channel closed before safe shutdown: {error}"
                            );
                        }
                        return;
                    }
                }
            }

            // In teleoperation the critical path is leader read -> follower
            // write. Reading every other arm between those two adds visible
            // latency for no control benefit, so service that path first and
            // collect the remaining telemetry immediately afterwards.
            if let ModeSelection::Teleoperate { leader, .. } =
                self.published.mode.selection().clone()
            {
                self.poll_robot(&leader);
                self.apply_mode();
                self.drive_robots();
                self.poll_robots_except(&leader);
            } else {
                self.poll_robots();
                self.apply_mode();
                self.drive_robots();
            }
            self.drop_failed_robots();
            // After driving, so a sample's measured position and the target it
            // is being compared against come from the same cycle.
            self.capture_sample();
            self.publish();

            let now = Instant::now();
            if let Some(remaining) = next_tick.checked_duration_since(now) {
                thread::sleep(remaining);
            } else {
                // The hardware work itself exceeded the period. Do not run a
                // burst of catch-up cycles; resume from reality and report the
                // lower measured rate honestly.
                next_tick = now;
            }
            next_tick += self.settings.period;
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Connect { spec, reply } => {
                let _ = reply.send(self.connect(*spec));
            }
            Command::Disconnect { id, reply } => {
                let result = self.detach_robot(&id, "the robot was disconnected");
                if !self.robots.contains_key(&id) {
                    self.published
                        .connections
                        .insert(id.clone(), ConnectionStatus::default());
                }
                let _ = reply.send(result);
            }
            Command::Hold { id, reply } => {
                let _ = reply.send(self.hold(&id));
            }
            Command::Release { id, reply } => {
                let _ = reply.send(self.release(&id));
            }
            Command::SetGoals {
                id,
                normalized,
                reply,
            } => {
                let _ = reply.send(self.set_goals(&id, normalized.values()));
            }
            Command::SetPolicyGoals { goals, reply } => {
                let _ = reply.send(self.set_policy_goals(&goals));
            }
            Command::StopPolicySession { session, reply } => {
                let result = if matches!(
                    self.published.mode.selection(),
                    ModeSelection::Policy { session: active, .. } if active == &session
                ) {
                    self.set_mode(ModeSelection::Idle)
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Command::SetMode { mode, reply } => {
                let _ = reply.send(self.set_mode(mode));
            }
            Command::StartCalibration { id, reply } => {
                let _ = reply.send(self.start_calibration(&id));
            }
            Command::CalibrationInputFor { id, reply } => {
                let _ = reply.send(self.calibration_input(&id));
            }
            Command::Forget { id, reply } => {
                let result = if self.robots.contains_key(&id) {
                    self.detach_robot(&id, "the robot was removed")
                } else {
                    Ok(())
                };
                if !self.robots.contains_key(&id) {
                    self.published.connections.remove(&id);
                }
                let _ = reply.send(result);
            }
            Command::CancelCalibration { id, reply } => {
                let result = match self.robots.get_mut(&id) {
                    Some(robot) => {
                        robot.sweep = None;
                        Ok(())
                    }
                    None => Err(format!("robot {id:?} is not connected")),
                };
                let _ = reply.send(result);
            }
            Command::ApplyCalibration {
                id,
                calibration,
                reply,
            } => {
                let _ = reply.send(self.apply_calibration(&id, *calibration));
            }
            Command::StartRecording {
                writer,
                arms,
                reply,
            } => {
                let _ = reply.send(self.start_recording(writer, arms));
            }
            Command::StopRecording { keep, reply } => {
                let _ = reply.send(self.stop_recording(keep));
            }
            Command::SetRecordingPaused { paused, reply } => {
                let _ = reply.send(self.set_recording_paused(paused));
            }
            Command::SetReplayPaused { paused, reply } => {
                let _ = reply.send(self.set_replay_paused(paused));
            }
            Command::StartReplay { request, reply } => {
                let _ = reply.send(self.start_replay(*request));
            }
            Command::Shutdown { reply } => {
                // `run` intercepts shutdown so it can leave the control loop.
                // Keeping this arm explicit prevents a future dispatch change
                // from acknowledging shutdown without the safety transaction.
                let _ = reply.send(self.shutdown_all());
            }
        }
    }

    fn start_recording(
        &mut self,
        writer: Box<dyn RecordingWriter>,
        arms: Vec<EpisodeArm>,
    ) -> Result<(), String> {
        if let Some(existing) = &self.operations.recording {
            return Err(format!(
                "take {} is already recording; stop it first",
                existing.writer.id()
            ));
        }
        // Every named arm has to be open right now, or the take would begin
        // with a hole in it.
        for arm in &arms {
            if !self.robots.contains_key(arm.id()) {
                return Err(format!("robot {:?} is not connected", arm.id()));
            }
        }

        // One slot per joint, sized here so every fold below is an index into a
        // list that already has the right shape.
        let condition = arms
            .iter()
            .map(|arm| ArmConditionSummary::new(arm.id().to_owned(), arm.joints().len()))
            .collect();

        let clock = RecordingClock::started_now();
        writer.update_clock(clock.started, Some(Duration::ZERO));
        self.published.recording_issue = None;
        self.operations.recording = Some(Recording {
            writer,
            arms,
            resume_mode: self.published.mode.selection().clone(),
            clock,
            frames: 0,
            tracking: RecordingTrackingSummary::default(),
            condition,
        });
        Ok(())
    }

    fn stop_recording(&mut self, keep: bool) -> Result<CompletedRecording, String> {
        let mut recording = self
            .operations
            .recording
            .take()
            .ok_or_else(|| "no take is being recorded".to_owned())?;
        let now = Instant::now();
        recording.writer.update_clock(now, None);
        recording.close_pause(now);
        let duration = recording.active_elapsed(now).as_secs_f64();
        let follow_error = recording.follow_error();
        let condition = recording.condition;
        let completion = RecordingCompletion::checkpoint(duration, recording.clock.gaps)
            .with_follow_error(follow_error)
            .with_condition(condition);
        let completion = if keep {
            completion
        } else {
            completion.discard()
        };
        recording
            .writer
            .finish(completion)
            .map_err(|error| error.to_string())
    }

    fn set_recording_paused(
        &mut self,
        paused: bool,
    ) -> Result<(Option<EpisodeCheckpoint>, f64), String> {
        if paused {
            let (checkpoint, elapsed_seconds) = {
                let recording = self
                    .operations
                    .recording
                    .as_mut()
                    .ok_or_else(|| "no take is being recorded".to_owned())?;
                let now = Instant::now();
                recording.clock.pause(now);
                recording.writer.update_clock(now, None);
                // FIFO ordering means this barrier is processed after every
                // sample accepted before the pause. The async caller awaits it
                // without blocking this fixed-rate controller thread.
                let elapsed_seconds = recording.active_elapsed(Instant::now()).as_secs_f64();
                let completion =
                    RecordingCompletion::checkpoint(elapsed_seconds, recording.clock.gaps.clone())
                        .with_follow_error(recording.follow_error())
                        .with_condition(recording.condition.clone());
                let checkpoint = recording.writer.checkpoint(completion)?;
                (checkpoint, elapsed_seconds)
            };
            self.set_mode(ModeSelection::Idle)?;
            return Ok((Some(checkpoint), elapsed_seconds));
        }
        let resume_mode = self
            .operations
            .recording
            .as_ref()
            .ok_or_else(|| "no take is being recorded".to_owned())?
            .resume_mode
            .clone();
        self.set_mode(resume_mode)?;
        let elapsed_seconds = self
            .operations
            .recording
            .as_ref()
            .map(|recording| recording.active_elapsed(Instant::now()).as_secs_f64())
            .unwrap_or_default();
        if let Some(recording) = self.operations.recording.as_mut()
            && recording.clock.is_paused()
        {
            let now = Instant::now();
            recording.close_pause(now);
            recording
                .writer
                .update_clock(now, Some(recording.active_elapsed(now)));
        }
        Ok((None, elapsed_seconds))
    }

    /// Append this cycle's positions to the take in progress.
    fn capture_sample(&mut self) {
        let Some(recording) = self.operations.recording.as_mut() else {
            return;
        };

        // An arm that left mid-take ends the take. Recording zeros in its place
        // would produce a take that looks complete and describes motion that
        // never happened.
        let missing = recording
            .arms
            .iter()
            .map(|arm| arm.id().to_owned())
            .find(|id| !self.robots.contains_key(id));
        if let Some(missing) = missing {
            let reason = format!("stopped recording: robot {missing:?} is no longer connected");
            match self.stop_recording(true) {
                Ok(summary) => {
                    self.published.recording_issue = Some(format!(
                        "{reason}. The {} frames captured before that are stored as {}",
                        summary.frames(),
                        summary.episode()
                    ));
                }
                Err(error) => {
                    self.published.recording_issue = Some(format!("{reason}. {error}"));
                }
            }
            return;
        }

        if recording.clock.is_paused() {
            return;
        }

        let seconds = recording.active_elapsed(Instant::now()).as_secs_f64();
        // Cloned once so the loop below can fold this frame into the take's
        // running extremes while it reads. It is two arms of six joint names.
        let arms = recording.arms.clone();
        let mut normalized = Vec::with_capacity(arms.len());
        let mut raw = Vec::with_capacity(arms.len());
        let mut velocity = Vec::with_capacity(arms.len());
        let mut effort = Vec::with_capacity(arms.len());
        let mut action = Vec::with_capacity(arms.len());
        let mut instant_error: Option<f64> = None;
        // A take is one shape from its first frame to its last. If any joint on
        // any arm cannot report velocity or effort, the take records neither,
        // rather than a column of nulls a reader would have to learn to
        // distrust partway down.
        let mut proprioception = true;
        for (index, arm) in arms.iter().enumerate() {
            let Some(robot) = self.robots.get(arm.id()) else {
                return;
            };
            let (arm_normalized, arm_raw) = robot.sample(arm.joints());
            normalized.push(arm_normalized);
            raw.push(arm_raw);
            let (arm_velocity, arm_effort) = robot.sample_proprioception(arm.joints());
            action.push(robot.sample_action(arm.joints()));
            proprioception &= arm_velocity.iter().chain(&arm_effort).all(Option::is_some);
            velocity.push(arm_velocity);
            effort.push(arm_effort);
            if let Some(error) = robot.follow_error_degrees() {
                instant_error = Some(instant_error.map_or(error, |worst: f64| worst.max(error)));
            }
            if let Some(summary) = recording.condition.get_mut(index) {
                for (slot, joint) in arm.joints().iter().enumerate() {
                    if let Some(track) = robot.tracks.get(joint) {
                        summary.observe_motor(slot, track);
                    }
                }
            }
        }
        if let Some(error) = instant_error {
            recording.tracking.observe_follow_error(error);
        }

        if recording.writer.push(
            Sample::new(seconds, normalized, raw)
                .with_follow_error_degrees(instant_error)
                .with_velocity(proprioception.then_some(velocity))
                .with_effort(proprioception.then_some(effort))
                .with_action(action),
        ) {
            recording.frames += 1;
            return;
        }

        // The writer refused the sample, which means the take is already
        // failing. Close it rather than spinning against a dead queue.
        let reason = recording
            .writer
            .failure()
            .unwrap_or_else(|| "the take could not be written".to_owned());
        match self.stop_recording(true) {
            Ok(summary) => {
                self.published.recording_issue =
                    Some(format!("stopped recording {}: {reason}", summary.episode()));
            }
            Err(error) => {
                self.published.recording_issue =
                    Some(format!("stopped recording: {reason}. {error}"));
            }
        }
    }

    fn start_replay(&mut self, request: RobotPlaybackRequest) -> Result<(), String> {
        let RobotPlaybackRequest {
            robot,
            episode,
            trajectory: ReplayTrajectory { frames, sample_hz },
            start_frame,
            resolved_start_seconds,
            purpose,
        } = request;
        if frames.is_empty() {
            return Err(format!("take {episode:?} has no frames to replay"));
        }
        if !sample_hz.is_finite() || sample_hz <= 0.0 {
            return Err(format!(
                "take {episode:?} records an unusable sample rate of {sample_hz}"
            ));
        }
        if start_frame >= frames.len() {
            return Err(format!(
                "take {episode:?} has no replay frame at index {start_frame}"
            ));
        }
        if !resolved_start_seconds.is_finite() || resolved_start_seconds < 0.0 {
            return Err(format!(
                "take {episode:?} resolves to an unusable replay time of {resolved_start_seconds}"
            ));
        }
        let calibration = self.require_calibrated(&robot)?.clone();
        let replay_joints = frames[0].keys().cloned().collect::<BTreeSet<_>>();
        // Validate the complete trajectory, not just its first pose. Silently
        // dropping an unknown or non-finite joint in a later frame would make
        // the physical replay differ from the episode it claims to reproduce.
        for (frame, pose) in frames.iter().enumerate() {
            if pose.is_empty() {
                return Err(format!(
                    "take {episode:?} has no commandable joints in replay frame {frame}"
                ));
            }
            if pose.keys().cloned().collect::<BTreeSet<_>>() != replay_joints {
                return Err(format!(
                    "take {episode:?} changes its replay joint set in frame {frame}"
                ));
            }
            for (joint, value) in pose {
                if !calibration.motors().contains_key(joint) {
                    return Err(format!(
                        "robot {robot:?} has no calibrated joint {joint:?} used by replay frame {frame} of {episode:?}"
                    ));
                }
                if !value.is_finite() || !(-1.0..=1.0).contains(value) {
                    return Err(format!(
                        "take {episode:?} has a normalized value outside [-1, 1] for joint {joint:?} in replay frame {frame}"
                    ));
                }
            }
        }

        // Set the mode through the same path every other mode goes through, so
        // replay inherits the calibration check, the sweep refusal, the
        // target reset, and the torque handling rather than reimplementing them.
        self.set_mode(ModeSelection::Replay {
            robot: robot.to_owned(),
            episode: episode.to_owned(),
        })?;

        let start_position = start_frame as f64;
        self.operations.replay = Some(Replay {
            episode: episode.to_owned(),
            robot: robot.to_owned(),
            frames,
            frame_period: 1.0 / sample_hz,
            purpose,
            phase: ReplayPhase::Seeking,
            timing: ReplayTiming {
                position: start_position,
                start_position,
                resolved_start_seconds,
                paused: false,
                seeking_since: Instant::now(),
            },
        });
        Ok(())
    }

    fn set_replay_paused(&mut self, paused: bool) -> Result<(), String> {
        // A physical seek is an atomic motion consequence, not Timeline
        // transport. A late pause from the replay that preceded a scrub must
        // not freeze the replacement seek before the arm can move.
        if self
            .operations
            .replay
            .as_ref()
            .is_some_and(|replay| replay.purpose == ReplayPurpose::PhysicalSeek)
        {
            return Ok(());
        }
        let robot = self
            .operations
            .replay
            .as_ref()
            .map(|replay| replay.robot.clone())
            .ok_or_else(|| "no episode is being replayed".to_owned())?;
        if self
            .operations
            .replay
            .as_ref()
            .is_some_and(|replay| replay.timing.paused == paused)
        {
            return Ok(());
        }
        if paused {
            let arm = self
                .robots
                .get_mut(&robot)
                .ok_or_else(|| format!("robot {robot:?} is not connected"))?;
            // Freeze the target at the measured pose before suspending the
            // timeline. This prevents the last queued replay target from
            // continuing to pull the arm after Pause was acknowledged.
            arm.follow_measured_positions();
        }
        if let Some(replay) = self.operations.replay.as_mut() {
            replay.timing.paused = paused;
            if !paused && replay.phase == ReplayPhase::Seeking {
                replay.timing.seeking_since = Instant::now();
            }
        }
        Ok(())
    }

    fn connect(&mut self, spec: RobotSpec) -> Result<(), String> {
        if self.robots.contains_key(&spec.id) {
            return Err(format!("robot {:?} is already connected", spec.id));
        }
        MotorSpecifications::try_from(&spec.motors)
            .map_err(|error| format!("robot {:?} has invalid motors: {error}", spec.id))?;
        if let Some(calibration) = &spec.calibration {
            calibration
                .validate_for_motor_specs(&spec.motors)
                .map_err(|error| {
                    format!(
                        "calibration for robot {:?} does not match its configured motors: {error}",
                        spec.id
                    )
                })?;
        }
        // Two robots cannot share one physical serial port.
        let holder = self
            .robots
            .values()
            .find(|robot| robot.spec.port.path() == spec.port.path());
        if let Some(holder) = holder {
            return Err(format!(
                "port {:?} is already open for robot {:?}",
                spec.port.path(),
                holder.spec.id
            ));
        }

        let open = |spec: &RobotSpec| -> Result<Box<dyn MotorBus>, String> {
            #[cfg(test)]
            if self.use_test_bus {
                Ok(Box::new(SimulatedBus::new(&spec.motors, true)))
            } else {
                FeetechBus::connect(spec.port.path(), spec.port.baud_rate(), &spec.motors)
                    .map(|bus| Box::new(bus) as Box<dyn MotorBus>)
                    .map_err(|error| error.to_string())
            }
            #[cfg(not(test))]
            {
                FeetechBus::connect(spec.port.path(), spec.port.baud_rate(), &spec.motors)
                    .map(|bus| Box::new(bus) as Box<dyn MotorBus>)
                    .map_err(|error| error.to_string())
            }
        };

        let mut bus = open(&spec).inspect_err(|error| {
            self.published.connections.insert(
                spec.id.clone(),
                ConnectionStatus::new(false, Some(error.clone())),
            );
        })?;

        // Reading once proves every configured motor is really on the bus
        // before the robot is presented as connected.
        let positions = bus.read_positions().map_err(|error| {
            let message = format!(
                "could not reach all motors on {}: {error}",
                spec.port.path()
            );
            self.published.connections.insert(
                spec.id.clone(),
                ConnectionStatus::new(false, Some(message.clone())),
            );
            message
        })?;
        spec.motors
            .validate_read_keys(&positions, "initial position read")
            .inspect_err(|message| {
                self.published.connections.insert(
                    spec.id.clone(),
                    ConnectionStatus::new(false, Some(message.clone())),
                );
            })?;

        // A newly opened bus has no trustworthy logical torque state. Establish
        // and verify release before publishing the robot as connected; otherwise
        // an enable left by another process could be displayed as torque-off.
        bus.set_torque(false).map_err(|error| {
            let message = format!(
                "could not establish released torque state on {}: {error}",
                spec.port.path()
            );
            self.published.connections.insert(
                spec.id.clone(),
                ConnectionStatus::new(false, Some(message.clone())),
            );
            message
        })?;

        // Off the control loop's path, and allowed to come back empty: an arm
        // that drives without known thresholds is far more useful than one
        // that refuses to open because a limit register would not answer.
        let limits = bus
            .read_limits()
            .into_iter()
            .map(|(name, limits)| (name, limits.into()))
            .collect();

        let id = spec.id.clone();
        self.robots.insert(
            id.clone(),
            ConnectedRobot {
                spec,
                bus,
                motion: MotorMotion {
                    torque: TorqueState::Released,
                    target: positions.clone(),
                    commanded: positions.clone(),
                    positions,
                },
                tracks: BTreeMap::new(),
                limits,
                health: BusHealth::default(),
                sweep: None,
            },
        );
        self.published
            .connections
            .insert(id, ConnectionStatus::new(true, None));
        Ok(())
    }

    fn set_torque(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let robot = self
            .robots
            .get_mut(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        if enabled && robot.sweep.is_some() {
            return Err(
                "finish or cancel calibration before enabling torque; the arm must stay free to move by hand"
                    .to_owned(),
            );
        }

        match robot.bus.set_torque(enabled) {
            Ok(()) => {
                robot.motion.torque = if enabled {
                    TorqueState::Engaged
                } else {
                    TorqueState::Released
                };
                // Whichever way torque just moved, start from where the arm
                // actually is rather than from a stale target.
                robot.follow_measured_positions();
                robot.health.error = None;
                Ok(())
            }
            Err(transition_error) if enabled => {
                // A driver may have failed after its enable broadcast reached
                // hardware. Compensate again at this boundary: the controller
                // must establish release itself before recording Released.
                let compensation = robot.bus.set_torque(false);
                robot.follow_measured_positions();
                let message = match compensation {
                    Ok(()) => {
                        robot.motion.torque = TorqueState::Released;
                        format!(
                            "could not enable torque: {transition_error}; controller compensating torque-off was verified"
                        )
                    }
                    Err(release_error) => {
                        robot.motion.torque = TorqueState::Unknown;
                        format!(
                            "could not enable torque: {transition_error}; controller compensating torque-off also failed: {release_error}; physical torque state is unknown"
                        )
                    }
                };
                robot.health.error = Some(message.clone());
                Err(message)
            }
            Err(release_error) => {
                robot.motion.torque = TorqueState::Unknown;
                robot.follow_measured_positions();
                let message = format!(
                    "could not verify torque-off: {release_error}; physical torque state is unknown"
                );
                robot.health.error = Some(message.clone());
                Err(message)
            }
        }
    }

    fn hold(&mut self, id: &str) -> Result<(), String> {
        self.decide_and_execute(ControlIntent::Hold {
            robot: id.to_owned(),
        })
    }

    fn release(&mut self, id: &str) -> Result<(), String> {
        self.decide_and_execute(ControlIntent::Release {
            robot: id.to_owned(),
        })
    }

    fn set_goals(&mut self, id: &str, normalized: &BTreeMap<String, f64>) -> Result<(), String> {
        self.authorize(ControlIntent::AuthorizeManualCommand {
            robot: id.to_owned(),
        })?;
        self.set_normalized_goals(id, normalized)
    }

    fn set_policy_goals(&mut self, goals: &PolicyGoals) -> Result<(), String> {
        self.authorize(ControlIntent::AuthorizePolicyCommand {
            robot: goals.robot.clone(),
            session: goals.session.clone(),
        })?;
        self.set_normalized_goals(&goals.robot, goals.normalized.values())
    }

    fn set_normalized_goals(
        &mut self,
        id: &str,
        normalized: &BTreeMap<String, f64>,
    ) -> Result<(), String> {
        let robot = self
            .robots
            .get_mut(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        if robot.motion.torque != TorqueState::Engaged {
            return Err(match robot.motion.torque {
                TorqueState::Released => {
                    format!("enable torque on robot {id:?} before commanding its joints")
                }
                TorqueState::Unknown => format!(
                    "robot {id:?} cannot move because its physical torque state is unknown; verify release before enabling torque again"
                ),
                TorqueState::Engaged => unreachable!("engaged torque passed the guard"),
            });
        }
        let calibration =
            robot.spec.calibration.as_ref().ok_or_else(|| {
                format!("robot {id:?} must be calibrated before it can be commanded")
            })?;

        let mut targets = BTreeMap::new();
        for (name, value) in normalized {
            if !value.is_finite() || !(-1.0..=1.0).contains(value) {
                return Err(format!(
                    "joint {name:?} command must be finite and within normalized [-1, 1], found {value}"
                ));
            }
            let motor = calibration
                .motors()
                .get(name)
                .ok_or_else(|| format!("joint {name:?} has no calibration on robot {id:?}"))?;
            let raw = motor
                .denormalize(*value)
                .map_err(|error| format!("joint {name:?}: {error}"))?;
            targets.insert(name.clone(), raw);
        }
        robot.motion.target.extend(targets);
        Ok(())
    }

    fn set_mode(&mut self, selection: ModeSelection) -> Result<(), String> {
        self.decide_and_execute(ControlIntent::Select((&selection).into()))
    }

    fn decide_and_execute(&mut self, intent: ControlIntent) -> Result<(), String> {
        let decision = self
            .control_state()
            .decide(intent)
            .map_err(|error| error.to_string())?;
        self.execute_control_decision(decision)
    }

    fn authorize(&self, intent: ControlIntent) -> Result<(), String> {
        let decision = self
            .control_state()
            .decide(intent)
            .map_err(|error| error.to_string())?;
        debug_assert!(decision.is_empty(), "authorization is state-free");
        Ok(())
    }

    /// Snapshot only facts the application policy needs. Driver and contract
    /// objects stop here at the adapter boundary.
    fn control_state(&self) -> ControlState {
        let robots = self
            .robots
            .iter()
            .map(|(id, robot)| {
                let calibration = match robot.spec.calibration.as_ref() {
                    None => CalibrationAdmission::Missing,
                    Some(calibration) => {
                        match calibration.validate_for_motor_specs(&robot.spec.motors) {
                            Ok(()) => CalibrationAdmission::Ready(
                                calibration.motors().keys().cloned().collect(),
                            ),
                            Err(error) => CalibrationAdmission::Invalid(error.to_string()),
                        }
                    }
                };
                (
                    id.clone(),
                    RobotAdmission::new(calibration, robot.sweep.is_some()),
                )
            })
            .collect();
        ControlState::new(self.published.mode.selection().into(), robots)
    }

    fn execute_control_decision(&mut self, decision: ControlDecision) -> Result<(), String> {
        for effect in decision.into_effects() {
            match effect {
                ControlEffect::ClearReplay => self.operations.replay = None,
                ControlEffect::ResetAllTargets => {
                    for robot in self.robots.values_mut() {
                        robot.follow_measured_positions();
                    }
                }
                ControlEffect::ResetTargets { robot } => {
                    if let Some(robot) = self.robots.get_mut(&robot) {
                        robot.follow_measured_positions();
                    }
                }
                ControlEffect::SetTorque {
                    robot,
                    engaged,
                    on_failure,
                } => {
                    if let Err(error) = self.set_torque(&robot, engaged) {
                        let (recovery_effects, recovery_error) =
                            on_failure.failure_decision(error).into_parts();
                        // Recovery is deliberately limited to state-only
                        // effects, so a second driver fault cannot prevent the
                        // policy from surrendering ownership to Idle.
                        for recovery_effect in recovery_effects {
                            self.apply_state_effect(recovery_effect);
                        }
                        return Err(recovery_error);
                    }
                }
                ControlEffect::CommitMode(mode) => {
                    self.commit_control_mode(mode);
                }
                ControlEffect::CommitModeWithError { mode, error } => {
                    self.commit_control_mode_with_error(mode, error);
                }
                ControlEffect::SetActivity { active, error } => {
                    self.published.mode.update_activity(active, error);
                }
            }
        }
        Ok(())
    }

    fn apply_state_effect(&mut self, effect: ControlEffect) {
        match effect {
            ControlEffect::ClearReplay => self.operations.replay = None,
            ControlEffect::ResetAllTargets => {
                for robot in self.robots.values_mut() {
                    robot.follow_measured_positions();
                }
            }
            ControlEffect::ResetTargets { robot } => {
                if let Some(robot) = self.robots.get_mut(&robot) {
                    robot.follow_measured_positions();
                }
            }
            ControlEffect::CommitMode(mode) => self.commit_control_mode(mode),
            ControlEffect::CommitModeWithError { mode, error } => {
                self.commit_control_mode_with_error(mode, error);
            }
            ControlEffect::SetActivity { active, error } => {
                self.published.mode.update_activity(active, error);
            }
            ControlEffect::SetTorque { .. } => {
                unreachable!("recovery policy must not contain physical effects")
            }
        }
    }

    fn commit_control_mode(&mut self, mode: ControlMode) {
        let selection = ModeSelection::from(mode);
        self.published.mode = ModeStatus::new(
            selection.clone(),
            !matches!(selection, ModeSelection::Idle),
            None,
        );
    }

    fn commit_control_mode_with_error(&mut self, mode: ControlMode, error: String) {
        let selection = ModeSelection::from(mode);
        self.published.mode = ModeStatus::new(
            selection.clone(),
            !matches!(selection, ModeSelection::Idle),
            Some(error),
        );
    }

    fn require_calibrated(&self, id: &str) -> Result<&Calibration, String> {
        let robot = self
            .robots
            .get(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        let calibration = robot
            .spec
            .calibration
            .as_ref()
            .ok_or_else(|| format!("robot {id:?} must be calibrated first"))?;
        calibration.validate_for_motor_specs(&robot.spec.motors).map_err(|error| {
            format!(
                "robot {id:?} has calibration that does not match its configured motors: {error}"
            )
        })?;
        Ok(calibration)
    }

    fn start_calibration(&mut self, id: &str) -> Result<(), String> {
        self.decide_and_execute(ControlIntent::StartCalibration {
            robot: id.to_owned(),
        })
        .map_err(|error| format!("could not release torque for calibration: {error}"))?;

        let robot = self
            .robots
            .get_mut(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;

        let positions = robot
            .bus
            .read_positions()
            .map_err(|error| error.to_string())?;
        robot
            .spec
            .motors
            .validate_read_keys(&positions, "calibration position read")?;
        robot.motion.positions = positions.clone();
        robot.follow_measured_positions();
        robot.sweep = Some(MotorRangeTracker::new(positions).map_err(|error| error.to_string())?);
        robot.health.error = None;
        Ok(())
    }

    fn calibration_input(&self, id: &str) -> Result<CalibrationInput, String> {
        let robot = self
            .robots
            .get(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        let sweep = robot
            .sweep
            .as_ref()
            .ok_or_else(|| format!("no calibration is in progress for robot {id:?}"))?;

        // Reading too early must not throw away the sweep so far: the operator
        // should be able to keep moving joints and try again.
        if !sweep.is_ready() {
            return Err(CalibrationError::InsufficientTravel {
                motors: sweep.insufficient_travel(),
            }
            .to_string());
        }

        sweep
            .clone()
            .into_input()
            .map_err(|error| error.to_string())
    }

    fn apply_calibration(&mut self, id: &str, calibration: Calibration) -> Result<(), String> {
        let robot = self
            .robots
            .get_mut(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        calibration
            .validate_for_motor_specs(&robot.spec.motors)
            .map_err(|error| {
                format!(
                    "calibration for robot {id:?} does not match its configured motors: {error}"
                )
            })?;
        robot.spec.calibration = Some(calibration);
        Ok(())
    }

    fn poll_robots(&mut self) {
        for robot in self.robots.values_mut() {
            Self::poll(robot);
        }
    }

    fn poll_robot(&mut self, id: &str) {
        if let Some(robot) = self.robots.get_mut(id) {
            Self::poll(robot);
        }
    }

    fn poll_robots_except(&mut self, excluded: &str) {
        for (id, robot) in &mut self.robots {
            if id != excluded {
                Self::poll(robot);
            }
        }
    }

    fn poll(robot: &mut ConnectedRobot) {
        match robot.bus.read_samples() {
            Ok(samples) => {
                if let Err(error) = robot
                    .spec
                    .motors
                    .validate_read_keys(&samples, "telemetry read")
                {
                    robot.health.failures += 1;
                    robot.health.error = Some(error);
                    return;
                }
                robot.health.failures = 0;
                // A telemetry read says nothing about Torque_Enable. Preserve
                // the transition evidence until a verified torque operation
                // resolves an unknown physical state.
                if robot.motion.torque != TorqueState::Unknown {
                    robot.health.error = None;
                }
                let positions = samples
                    .iter()
                    .map(|(name, sample)| (name.clone(), sample.position()))
                    .collect::<BTreeMap<_, _>>();
                if let Some(sweep) = robot.sweep.as_mut()
                    && let Err(error) = sweep.observe(&positions)
                {
                    robot.health.error = Some(error.to_string());
                    robot.sweep = None;
                }
                let now = Instant::now();
                for (name, sample) in samples {
                    robot.tracks.entry(name).or_default().observe(sample, now);
                }
                robot.motion.positions = positions;
                if !robot.motion.torque.is_engaged() {
                    robot.follow_measured_positions();
                }
            }
            Err(error) => {
                robot.health.failures += 1;
                robot.health.error = Some(error.to_string());
            }
        }
    }

    fn drive_robots(&mut self) {
        // An arm keeping up with recorded motion needs the same faster ramp a
        // teleoperation follower gets, for the same reason: the trajectory it is
        // chasing was produced by a human hand moving at human speed. While a
        // replay is still walking to the take's first frame it stays on the
        // gentle manual ramp, because that move is not in the recording and the
        // operator has no way to anticipate it.
        let quick = match self.published.mode.selection() {
            ModeSelection::Teleoperate { follower, .. } => Some(follower.as_str()),
            ModeSelection::Replay { robot, .. } => self
                .operations
                .replay
                .as_ref()
                .filter(|replay| replay.phase == ReplayPhase::Playing)
                .map(|_| robot.as_str()),
            ModeSelection::Policy { robot, .. } => Some(robot.as_str()),
            _ => None,
        };
        for (id, robot) in &mut self.robots {
            let max_step = if quick == Some(id.as_str()) {
                self.settings.teleop_max_step_counts
            } else {
                self.settings.max_step_counts
            };
            robot.drive(max_step);
        }
    }

    fn apply_mode(&mut self) {
        if matches!(
            self.published.mode.selection(),
            ModeSelection::Replay { .. }
        ) {
            self.apply_replay();
            return;
        }
        let ModeSelection::Teleoperate { leader, follower } =
            self.published.mode.selection().clone()
        else {
            return;
        };

        let targets = {
            let Some(leader_robot) = self.robots.get(&leader) else {
                self.leave_mode_involving(&leader, "the leader arm is no longer connected");
                return;
            };
            let Some(follower_robot) = self.robots.get(&follower) else {
                self.leave_mode_involving(&follower, "the follower arm is no longer connected");
                return;
            };
            let (Some(leader_calibration), Some(follower_calibration)) = (
                leader_robot.spec.calibration.as_ref(),
                follower_robot.spec.calibration.as_ref(),
            ) else {
                let decision = self
                    .control_state()
                    .decide_event(ControlEvent::TeleoperationUnavailable);
                self.execute_control_decision(decision)
                    .expect("teleoperation observation contains no physical effects");
                return;
            };

            leader_robot
                .motion
                .positions
                .iter()
                .filter_map(|(name, raw)| {
                    let source = leader_calibration.motors().get(name)?;
                    let destination = follower_calibration.motors().get(name)?;
                    let normalized = source.normalize_raw(*raw).ok()?;
                    let target = destination.denormalize(normalized).ok()?;
                    Some((name.clone(), target))
                })
                .collect::<BTreeMap<_, _>>()
        };

        if let Some(follower_robot) = self.robots.get_mut(&follower) {
            follower_robot.motion.target.extend(targets);
        }
        self.published.mode.update_activity(true, None);
    }

    /// Walk the replay forward one control cycle.
    ///
    /// Two phases. The arm is first ramped to the requested frame at the
    /// gentle manual rate, because a recording rarely matches where the
    /// arm is standing — the previous take left it at *its* last pose. Only once
    /// every joint is there does playback begin, and playback advances by real
    /// elapsed time so a take captured at sixty hertz plays at sixty hertz
    /// whatever rate this loop is managing today.
    fn apply_replay(&mut self) {
        let Some(replay) = self.operations.replay.as_mut() else {
            let decision = self.control_state().decide_event(ControlEvent::ReplayLost);
            self.execute_control_decision(decision)
                .expect("replay-loss policy contains no physical effects");
            return;
        };
        let (robot_id, episode) = (replay.robot.clone(), replay.episode.clone());

        if replay.timing.paused {
            let decision = ReplayPolicyState::from(replay.phase)
                .decide(ReplayPolicyEvent::Paused)
                .expect("pause is legal in every replay phase");
            debug_assert_eq!(decision, ReplayPolicyDecision::Keep);
            return;
        }

        let Some(robot) = self.robots.get(&robot_id) else {
            let decision = ReplayPolicyState::from(replay.phase)
                .decide(ReplayPolicyEvent::Failed(
                    "stopped Replay: the arm is no longer connected".to_owned(),
                ))
                .expect("failure is legal in every replay phase");
            self.apply_replay_policy(&robot_id, decision);
            return;
        };
        let Some(calibration) = robot.spec.calibration.clone() else {
            let decision = ReplayPolicyState::from(replay.phase)
                .decide(ReplayPolicyEvent::Failed(format!(
                    "stopped Replay: robot {robot_id:?} is no longer calibrated"
                )))
                .expect("failure is legal in every replay phase");
            self.apply_replay_policy(&robot_id, decision);
            return;
        };

        if replay.phase == ReplayPhase::Seeking {
            let Some(first) = replay.pose().cloned() else {
                self.fail_replay(
                    &robot_id,
                    format!("take {episode:?} has no frames to replay"),
                );
                return;
            };
            let arrived =
                first.iter().try_fold(true, |arrived, (joint, wanted)| {
                    let motor = calibration.motors().get(joint).ok_or_else(|| {
                        format!("calibration no longer contains replay joint {joint:?}")
                    })?;
                    let measured =
                        robot.motion.positions.get(joint).copied().ok_or_else(|| {
                            format!("robot stopped reporting replay joint {joint:?}")
                        })?;
                    let actual = motor.normalize_raw(measured).map_err(|error| {
                        format!("could not normalize replay joint {joint:?}: {error}")
                    })?;
                    Ok::<bool, String>(arrived && (actual - wanted).abs() <= REPLAY_SEEK_TOLERANCE)
                });
            let arrived = match arrived {
                Ok(arrived) => arrived,
                Err(error) => {
                    let decision = ReplayPolicyState::Seeking
                        .decide(ReplayPolicyEvent::Failed(format!(
                            "stopped Replay: {error}"
                        )))
                        .expect("failure is legal while seeking");
                    self.apply_replay_policy(&robot_id, decision);
                    return;
                }
            };

            if !arrived && replay.timing.seeking_since.elapsed() >= REPLAY_SEEK_TIMEOUT {
                let waited = REPLAY_SEEK_TIMEOUT.as_secs();
                let decision = ReplayPolicyState::Seeking.decide(ReplayPolicyEvent::Failed(format!(
                        "stopped Replay: {robot_id} did not reach the requested pose in {episode} within {waited} seconds. Check whether a joint is obstructed."
                    )),
                )
                .expect("failure is legal while seeking");
                self.apply_replay_policy(&robot_id, decision);
                return;
            }
            let decision = ReplayPolicyState::Seeking
                .decide(ReplayPolicyEvent::SeekObserved { arrived })
                .expect("seek observation is legal while seeking");
            if self.apply_replay_policy(&robot_id, decision) {
                return;
            }
        }

        let Some(replay) = self.operations.replay.as_mut() else {
            return;
        };
        if replay.phase == ReplayPhase::Playing {
            let decision = ReplayPolicyState::Playing
                .decide(ReplayPolicyEvent::PlaybackObserved {
                    finished: replay.finished(),
                })
                .expect("playback observation is legal while playing");
            if self.apply_replay_policy(&robot_id, decision) {
                return;
            }
            let replay = self
                .operations
                .replay
                .as_mut()
                .expect("keeping playback preserves the replay");
            replay.timing.position += self.settings.period.as_secs_f64() / replay.frame_period;
        }

        let Some(replay) = self.operations.replay.as_ref() else {
            return;
        };
        let Some(pose) = replay.pose().cloned() else {
            return;
        };
        let targets = pose
            .iter()
            .map(|(joint, value)| {
                let motor = calibration.motors().get(joint).ok_or_else(|| {
                    format!("calibration no longer contains replay joint {joint:?}")
                })?;
                let target = motor.denormalize(*value).map_err(|error| {
                    format!("could not command replay joint {joint:?}: {error}")
                })?;
                Ok((joint.clone(), target))
            })
            .collect::<Result<BTreeMap<_, _>, String>>();
        let targets = match targets {
            Ok(targets) => targets,
            Err(error) => {
                let state = self
                    .operations
                    .replay
                    .as_ref()
                    .map(|replay| ReplayPolicyState::from(replay.phase))
                    .unwrap_or(ReplayPolicyState::Playing);
                let decision = state
                    .decide(ReplayPolicyEvent::Failed(format!(
                        "stopped Replay: {error}"
                    )))
                    .expect("failure is legal in every replay phase");
                self.apply_replay_policy(&robot_id, decision);
                return;
            }
        };
        if let Some(robot) = self.robots.get_mut(&robot_id) {
            robot.motion.target.extend(targets);
        }
        self.published.mode.update_activity(true, None);
    }

    /// Apply the lifecycle consequence chosen by the application policy.
    /// Returns true when the replay ended and this cycle must stop processing.
    fn apply_replay_policy(&mut self, robot_id: &str, decision: ReplayPolicyDecision) -> bool {
        match decision {
            ReplayPolicyDecision::Keep => false,
            ReplayPolicyDecision::EnterPlaying => {
                if let Some(replay) = self.operations.replay.as_mut() {
                    replay.phase = ReplayPhase::Playing;
                }
                false
            }
            ReplayPolicyDecision::FinishToIdle => {
                // Completion leaves the arm holding its final pose; only the
                // exclusive replay owner is released.
                let decision = self
                    .control_state()
                    .decide_event(ControlEvent::ReplayFinished);
                self.execute_control_decision(decision)
                    .expect("replay completion contains no physical effects");
                true
            }
            ReplayPolicyDecision::FailToIdle(error) => {
                self.fail_replay(robot_id, error);
                true
            }
        }
    }

    fn fail_replay(&mut self, robot_id: &str, error: String) {
        let decision = self
            .control_state()
            .decide_event(ControlEvent::ReplayFailed {
                robot: robot_id.to_owned(),
                error,
            });
        self.execute_control_decision(decision)
            .expect("replay failure policy contains no physical effects");
    }

    /// Fall back to idle when a robot the current mode depends on goes away.
    fn leave_mode_involving(&mut self, id: &str, reason: &str) {
        let decision = self.control_state().decide_event(ControlEvent::RobotLost {
            robot: id.to_owned(),
            reason: reason.to_owned(),
        });
        self.execute_control_decision(decision)
            .expect("robot-loss policy contains no fallible physical effects");
    }

    fn drop_failed_robots(&mut self) {
        let lost = self
            .robots
            .iter()
            .filter(|(_, robot)| robot.health.failures >= MAX_CONSECUTIVE_FAILURES)
            .map(|(id, robot)| {
                (
                    id.clone(),
                    robot
                        .health
                        .error
                        .clone()
                        .unwrap_or_else(|| "the motor bus stopped responding".to_owned()),
                )
            })
            .collect::<Vec<_>>();

        for (id, error) in lost {
            let detach = self.detach_robot(&id, "its motor bus stopped responding");
            let reported = match detach {
                Ok(()) => error,
                Err(detach) => format!("{error}; {detach}"),
            };
            self.published
                .connections
                .insert(id, ConnectionStatus::new(false, Some(reported)));
        }
    }

    /// Finish any take involving `id`, leave its control mode, establish
    /// torque-off, and only then drop the bus. A failed release keeps the bus
    /// owned by the controller so later cycles can retry rather than turning an
    /// unknown physical state into a successful detach.
    fn detach_robot(&mut self, id: &str, reason: &str) -> Result<(), String> {
        if !self.robots.contains_key(id) {
            return Err(format!("robot {id:?} is not connected"));
        }

        let recording_error = self
            .operations
            .recording
            .as_ref()
            .is_some_and(|recording| recording.arms.iter().any(|arm| arm.id() == id))
            .then(|| self.stop_recording(true).err())
            .flatten();
        if let Some(error) = &recording_error {
            self.published.recording_issue = Some(format!(
                "stopped recording because robot {id:?} detached, but finalizing the take failed: {error}"
            ));
        }

        self.leave_mode_involving(id, reason);
        self.set_torque(id, false).map_err(|release| {
            format!("refused to detach robot {id:?}: torque-off could not be verified: {release}")
        })?;
        self.robots.remove(id);

        if let Some(error) = recording_error {
            Err(format!(
                "robot {id:?} was safely released and detached, but its active recording could not be finalized: {error}"
            ))
        } else {
            Ok(())
        }
    }

    /// Close the global recording before releasing every connected robot. All
    /// release attempts run even when an earlier cleanup step failed.
    fn shutdown_all(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        if self.operations.recording.is_some()
            && let Err(error) = self.stop_recording(true)
        {
            self.published.recording_issue = Some(format!(
                "shutdown could not finalize the active take: {error}"
            ));
            failures.push(format!("could not finalize active recording: {error}"));
        }
        let ids = self.robots.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            if let Err(error) = self.detach_robot(&id, "the controller is shutting down") {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    fn publish(&mut self) {
        self.published.sequence += 1;
        let recording = self.operations.recording.as_ref().map(|recording| {
            let now = Instant::now();
            let identity = RecordingIdentity::new(
                recording.writer.id().to_owned(),
                recording.writer.dataset_id().to_owned(),
                recording
                    .arms
                    .iter()
                    .map(|arm| arm.id().to_owned())
                    .collect(),
            );
            let clock = RecordingProgressClock::new(
                recording.frames,
                recording.active_elapsed(now).as_secs_f64(),
                recording.clock.wall_elapsed(now).as_secs_f64(),
            )
            .with_paused_seconds(recording.paused_elapsed(now).as_secs_f64());
            let state = RecordingState::new(
                recording.clock.is_paused(),
                recording.follow_error(),
                recording.writer.failure(),
            );
            RecordingProgress::new(identity, clock, state)
        });
        let replay = self.operations.replay.as_ref().map(|replay| {
            let identity = ReplayIdentity::new(replay.episode.clone(), replay.robot.clone());
            // Seeking preserves the requested episode position while the arm
            // ramps there, so visualizations never jump back to frame zero
            // after a committed scrub.
            let timeline = ReplayTimeline::new(
                replay.cursor().min(replay.frames.len()) as u64,
                replay.frames.len() as u64,
                replay.elapsed_seconds(),
            )
            .with_duration_seconds(replay.duration_seconds());
            let transport =
                ReplayTransport::new(replay.purpose, replay.phase, replay.timing.paused);
            ReplayProgress::new(identity, timeline, transport)
        });
        let state = HardwareState {
            telemetry: TelemetryFrame::new(
                self.published.sequence,
                self.published.loop_hz,
                1.0 / self.settings.period.as_secs_f64(),
            )
            .with_robots(
                self.robots
                    .values()
                    .map(ConnectedRobot::telemetry)
                    .collect(),
            )
            .with_operations(recording, replay),
            connections: self.published.connections.clone(),
            mode: self.published.mode.clone(),
            limits: self
                .robots
                .iter()
                .map(|(id, robot)| (id.clone(), robot.limits.clone()))
                .collect(),
            recording_issue: self.published.recording_issue.clone(),
        };
        // `send_replace` rather than `send`: the latest state must be readable
        // through the handle even at moments when nothing is subscribed.
        let now = Instant::now();
        if self
            .published
            .last_chart_sample
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(50))
        {
            self.recording_signals
                .write()
                .unwrap_or_else(|error| error.into_inner())
                .observe(state.telemetry());
            self.published.last_chart_sample = Some(now);
        }
        let _ = self.state.send_replace(state);
    }
}

trait MotorSpecMapExt {
    fn validate_read_keys<T>(
        &self,
        readings: &BTreeMap<String, T>,
        context: &str,
    ) -> Result<(), String>;
}

impl MotorSpecMapExt for BTreeMap<String, MotorSpec> {
    fn validate_read_keys<T>(
        &self,
        readings: &BTreeMap<String, T>,
        context: &str,
    ) -> Result<(), String> {
        let expected = self.keys().cloned().collect::<BTreeSet<_>>();
        let actual = readings.keys().cloned().collect::<BTreeSet<_>>();
        if actual == expected {
            return Ok(());
        }
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
        Err(format!(
            "{context} did not match the configured motor set; missing={missing:?}, unexpected={unexpected:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::bus::{BusError, MotorElectricalSample, MotorMechanicalSample};
    use crate::calibration::MotorReading;

    use super::*;

    fn motors() -> BTreeMap<String, MotorSpec> {
        crate::calibration::MotorSpec::so101_arm()
    }

    fn calibration(center: i64, span: i64) -> Calibration {
        let motors = motors();
        calibration_for(&motors, center, span)
    }

    fn calibration_for(
        motors: &BTreeMap<String, MotorSpec>,
        center: i64,
        span: i64,
    ) -> Calibration {
        let input = CalibrationInput::Direct {
            motors: motors
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        MotorReading::new(center, center - span, center + span),
                    )
                })
                .collect(),
        };
        input.build(motors).expect("calibration")
    }

    fn spec(id: &str, calibration: Option<Calibration>) -> RobotSpec {
        let specification = RobotSpec::new(
            id.to_owned(),
            PortSettings::try_new(format!("/dev/{id}"), 1_000_000).expect("port settings"),
            motors(),
        )
        .expect("robot specification");
        calibration.map_or(specification.clone(), |calibration| {
            specification
                .with_calibration(calibration)
                .expect("matching calibration")
        })
    }

    fn timeline_replay(
        robot: &str,
        frames: Vec<BTreeMap<String, f64>>,
        sample_hz: f64,
    ) -> RobotPlaybackRequest {
        RobotPlaybackRequest::timeline(
            robot.to_owned(),
            "episode_000001".to_owned(),
            ReplayTrajectory::new(frames, sample_hz).expect("valid trajectory"),
        )
        .expect("valid replay request")
    }

    fn policy_goals(robot: &str, session: &str, goals: BTreeMap<String, f64>) -> PolicyGoals {
        PolicyGoals::new(
            robot.to_owned(),
            session.to_owned(),
            JointGoals::try_from(goals).expect("valid joint goals"),
        )
        .expect("valid policy goals")
    }

    fn controller() -> Controller {
        let (state, _receiver) = watch::channel(HardwareState::default());
        Controller::new_for_test(ControlSettings::default(), state)
    }

    struct FailingEnableBus {
        calls: Arc<Mutex<Vec<bool>>>,
        release_fails: bool,
        positions: BTreeMap<String, i64>,
    }

    impl MotorBus for FailingEnableBus {
        fn read_positions(&mut self) -> Result<BTreeMap<String, i64>, BusError> {
            Ok(self.positions.clone())
        }

        fn read_samples(&mut self) -> Result<BTreeMap<String, MotorSample>, BusError> {
            Ok(self
                .positions
                .iter()
                .map(|(name, position)| (name.clone(), MotorSample::at(*position)))
                .collect())
        }

        fn read_limits(&mut self) -> BTreeMap<String, BusMotorLimits> {
            BTreeMap::new()
        }

        fn write_goal_positions(&mut self, _goals: &BTreeMap<String, i64>) -> Result<(), BusError> {
            Ok(())
        }

        fn set_torque(&mut self, enabled: bool) -> Result<(), BusError> {
            self.calls.lock().unwrap().push(enabled);
            if enabled || self.release_fails {
                Err(BusError::driver(std::io::Error::other(if enabled {
                    "injected enable verification failure"
                } else {
                    "injected release verification failure"
                })))
            } else {
                Ok(())
            }
        }
    }

    fn install_failing_enable_bus(
        controller: &mut Controller,
        release_fails: bool,
    ) -> Arc<Mutex<Vec<bool>>> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let positions = controller.robots["arm"].motion.positions.clone();
        controller.robots.get_mut("arm").unwrap().bus = Box::new(FailingEnableBus {
            calls: Arc::clone(&calls),
            release_fails,
            positions,
        });
        calls
    }

    #[test]
    fn motor_reads_must_match_the_exact_configured_set() {
        let configured = motors();
        let exact = configured
            .keys()
            .map(|name| (name.clone(), 0_i64))
            .collect::<BTreeMap<_, _>>();
        assert!(configured.validate_read_keys(&exact, "test").is_ok());

        let mut missing = exact.clone();
        missing.remove(configured.keys().next().unwrap());
        assert!(configured.validate_read_keys(&missing, "test").is_err());

        let mut extra = exact;
        extra.insert("phantom".to_owned(), 0);
        assert!(configured.validate_read_keys(&extra, "test").is_err());
    }

    #[test]
    fn a_replay_walks_to_the_start_before_it_plays() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();

        // A trajectory that starts well away from where the arm is standing.
        let frames = (0..4)
            .map(|step| BTreeMap::from([("gripper".to_owned(), 0.5 + 0.1 * step as f64)]))
            .collect::<Vec<_>>();
        controller
            .start_replay(
                RobotPlaybackRequest::timeline(
                    "arm".to_owned(),
                    "episode_000001".to_owned(),
                    ReplayTrajectory::new(frames, 60.0).expect("valid trajectory"),
                )
                .expect("valid replay request"),
            )
            .expect("start replay");

        assert_eq!(
            controller
                .operations
                .replay
                .as_ref()
                .map(|replay| replay.phase),
            Some(ReplayPhase::Seeking)
        );
        // Entering replay energizes the arm, the same way entering teleoperation
        // holds its follower.
        assert_eq!(controller.robots["arm"].motion.torque, TorqueState::Engaged);

        // Seeking uses the gentle manual ramp, so the arm does not lunge at a
        // start pose the operator never saw coming.
        let mut cycles = 0;
        while controller
            .operations
            .replay
            .as_ref()
            .is_some_and(|replay| replay.phase == ReplayPhase::Seeking)
            && cycles < 400
        {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
            cycles += 1;
        }
        assert!(cycles > 1, "the arm had to travel to reach the start");
        assert_eq!(
            controller
                .operations
                .replay
                .as_ref()
                .map(|replay| replay.phase),
            Some(ReplayPhase::Playing),
            "the replay starts playing once the arm arrives"
        );

        // Playing to the end leaves idle with no fault attached.
        for _ in 0..400 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
            if controller.operations.replay.is_none() {
                break;
            }
        }
        assert!(
            controller.operations.replay.is_none(),
            "the replay finished"
        );
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert_eq!(
            controller.published.mode.error(),
            None,
            "reaching the end of a take is not a fault"
        );
    }

    #[test]
    fn a_paused_replay_resumes_from_the_same_frame() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        let frames = (0..24)
            .map(|step| BTreeMap::from([("gripper".to_owned(), 0.05 + 0.01 * step as f64)]))
            .collect::<Vec<_>>();
        controller
            .start_replay(
                RobotPlaybackRequest::timeline(
                    "arm".to_owned(),
                    "episode_000001".to_owned(),
                    ReplayTrajectory::new(frames, 60.0).expect("valid trajectory"),
                )
                .expect("valid replay request"),
            )
            .expect("start replay");

        for _ in 0..400 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
            if controller
                .operations
                .replay
                .as_ref()
                .is_some_and(|replay| replay.phase == ReplayPhase::Playing)
            {
                break;
            }
        }
        for _ in 0..4 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
        }
        let paused_at = controller
            .operations
            .replay
            .as_ref()
            .expect("replay is active")
            .timing
            .position;
        assert!(paused_at > 0.0, "the replay advanced before pausing");

        controller.set_replay_paused(true).expect("pause replay");
        for _ in 0..12 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
        }
        assert_eq!(
            controller
                .operations
                .replay
                .as_ref()
                .expect("paused replay remains loaded")
                .timing
                .position,
            paused_at,
            "pause must retain the exact trajectory position"
        );

        controller.set_replay_paused(false).expect("resume replay");
        controller.poll_robots();
        controller.apply_mode();
        controller.drive_robots();
        assert!(
            controller
                .operations
                .replay
                .as_ref()
                .expect("replay continues")
                .timing
                .position
                > paused_at,
            "resume continues forward instead of seeking to frame zero"
        );
    }

    #[test]
    fn a_replay_can_begin_at_the_committed_timeline_position() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        let frames = (0..120)
            .map(|step| BTreeMap::from([("gripper".to_owned(), step as f64 / 240.0)]))
            .collect::<Vec<_>>();

        controller
            .start_replay(
                timeline_replay("arm", frames, 60.0)
                    .starting_at(60, 1.25)
                    .expect("valid replay start"),
            )
            .expect("start replay at playhead");

        let replay = controller
            .operations
            .replay
            .as_ref()
            .expect("replay is active");
        assert_eq!(replay.timing.position, 60.0);
        assert_eq!(replay.cursor(), 60);
        assert_eq!(replay.elapsed_seconds(), 1.25);
        assert_eq!(
            replay.pose().and_then(|pose| pose.get("gripper")),
            Some(&0.25)
        );
    }

    #[test]
    fn a_late_transport_pause_cannot_freeze_a_physical_seek() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(
                RobotPlaybackRequest::physical_seek(
                    "arm".to_owned(),
                    "episode_000001".to_owned(),
                    ReplayTrajectory::single_pose(
                        BTreeMap::from([("gripper".to_owned(), 0.25)]),
                        60.0,
                    )
                    .expect("valid pose"),
                )
                .expect("valid physical seek"),
            )
            .expect("start physical seek");

        controller
            .set_replay_paused(true)
            .expect("late pause is harmless");

        assert!(
            !controller
                .operations
                .replay
                .as_ref()
                .expect("seek remains active")
                .timing
                .paused,
            "one-frame physical motion must finish even if the prior replay's pause arrives late"
        );
    }

    #[test]
    fn a_one_frame_timeline_replay_still_supports_transport_pause() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(timeline_replay(
                "arm",
                vec![BTreeMap::from([("gripper".to_owned(), 0.25)])],
                60.0,
            ))
            .expect("start timeline replay");

        controller.set_replay_paused(true).expect("pause replay");

        let replay = controller
            .operations
            .replay
            .as_ref()
            .expect("replay remains loaded");
        assert_eq!(replay.purpose, ReplayPurpose::Timeline);
        assert!(replay.timing.paused);
    }

    #[test]
    fn a_replay_needs_a_calibrated_arm_and_joints_that_exist() {
        let mut controller = controller();
        controller.connect(spec("bare", None)).expect("connect");
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");

        let pose = vec![BTreeMap::from([("gripper".to_owned(), 0.1)])];
        assert!(
            controller
                .start_replay(timeline_replay("bare", pose.clone(), 60.0))
                .expect_err("uncalibrated")
                .contains("calibrated")
        );
        assert!(
            ReplayTrajectory::new(Vec::new(), 60.0)
                .expect_err("no frames")
                .to_string()
                .contains("at least one frame")
        );
        assert!(
            ReplayTrajectory::new(pose.clone(), 0.0)
                .expect_err("bad rate")
                .to_string()
                .contains("sample rate")
        );
        assert!(
            controller
                .start_replay(timeline_replay(
                    "arm",
                    vec![BTreeMap::from([("sixth_axis".to_owned(), 0.1)])],
                    60.0,
                ))
                .expect_err("unknown joint")
                .contains("sixth_axis")
        );
        assert!(controller.operations.replay.is_none());
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
    }

    #[test]
    fn replay_static_shape_and_live_joint_membership_are_both_enforced() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");

        assert!(
            ReplayTrajectory::new(
                vec![
                    BTreeMap::from([("gripper".to_owned(), 0.0)]),
                    BTreeMap::from([("sixth_axis".to_owned(), 0.1)]),
                ],
                60.0,
            )
            .is_err(),
            "a changing joint set must be rejected before actor admission"
        );

        let frames = vec![
            BTreeMap::from([("sixth_axis".to_owned(), 0.0)]),
            BTreeMap::from([("sixth_axis".to_owned(), 0.1)]),
        ];
        let error = controller
            .start_replay(timeline_replay("arm", frames, 60.0))
            .expect_err("unknown live joint");
        assert!(error.contains("no calibrated joint"), "{error}");

        for (label, frames) in [
            (
                "later non-finite value",
                vec![
                    BTreeMap::from([("gripper".to_owned(), 0.0)]),
                    BTreeMap::from([("gripper".to_owned(), f64::NAN)]),
                ],
            ),
            (
                "empty pose",
                vec![
                    BTreeMap::from([("gripper".to_owned(), 0.0)]),
                    BTreeMap::new(),
                ],
            ),
        ] {
            assert!(
                ReplayTrajectory::new(frames, 60.0).is_err(),
                "{label} must be rejected before actor admission"
            );
        }
        assert!(controller.operations.replay.is_none());
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
    }

    #[test]
    fn replay_seek_fails_closed_when_required_joint_state_disappears() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(timeline_replay(
                "arm",
                vec![BTreeMap::from([("gripper".to_owned(), 0.5)])],
                60.0,
            ))
            .expect("start replay");
        controller
            .robots
            .get_mut("arm")
            .unwrap()
            .motion
            .positions
            .remove("gripper");

        controller.apply_replay();

        assert!(controller.operations.replay.is_none());
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert!(!controller.published.mode.active());
        assert!(
            controller
                .published
                .mode
                .error()
                .is_some_and(|error| error.contains("stopped reporting"))
        );
        assert!(
            !controller.robots["arm"]
                .motion
                .target
                .contains_key("gripper"),
            "failure pins targets to the remaining measured state"
        );
    }

    #[test]
    fn replay_seek_fails_closed_if_calibration_becomes_missing() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(timeline_replay(
                "arm",
                vec![BTreeMap::from([("gripper".to_owned(), 0.5)])],
                60.0,
            ))
            .expect("start replay");
        controller.robots.get_mut("arm").unwrap().spec.calibration = None;

        controller.apply_replay();

        assert!(controller.operations.replay.is_none());
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert!(!controller.published.mode.active());
        assert!(controller.published.mode.error().is_some());
        let robot = &controller.robots["arm"];
        assert_eq!(robot.motion.target, robot.motion.positions);
    }

    #[test]
    fn leaving_replay_drops_the_trajectory() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(timeline_replay(
                "arm",
                vec![BTreeMap::from([("gripper".to_owned(), 0.1)])],
                60.0,
            ))
            .expect("start replay");
        assert!(controller.operations.replay.is_some());

        controller.set_mode(ModeSelection::Idle).expect("stop");
        assert!(
            controller.operations.replay.is_none(),
            "an idle studio must not keep samples queued to drive an arm"
        );

        // And losing the arm does the same, with a reason.
        controller.poll_robots();
        controller
            .start_replay(timeline_replay(
                "arm",
                vec![BTreeMap::from([("gripper".to_owned(), 0.1)])],
                60.0,
            ))
            .expect("start replay again");
        controller.leave_mode_involving("arm", "its motor bus stopped responding");
        assert!(controller.operations.replay.is_none());
        assert!(
            controller
                .published
                .mode
                .error()
                .is_some_and(|error| error.contains("Replay"))
        );
    }

    #[test]
    fn connecting_twice_is_refused_and_leaves_the_first_bus_alone() {
        let mut controller = controller();
        controller.connect(spec("arm", None)).expect("connect");

        assert!(
            controller
                .connect(spec("arm", None))
                .expect_err("duplicate connect")
                .contains("already connected")
        );
        assert_eq!(controller.robots.len(), 1);
        assert!(controller.published.connections["arm"].connected());
    }

    #[test]
    fn an_uncalibrated_robot_cannot_be_commanded() {
        let mut controller = controller();
        controller.connect(spec("arm", None)).expect("connect");

        assert!(
            controller
                .set_mode(ModeSelection::Manual {
                    robot: "arm".to_owned()
                })
                .expect_err("uncalibrated manual mode")
                .contains("calibrated")
        );
        assert!(
            controller
                .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 1.0)]))
                .expect_err("commanding without a mode")
                .contains("manual mode")
        );
    }

    #[test]
    fn hardware_rejects_calibration_for_a_different_motor_definition() {
        let mut controller = controller();
        let mut stale_motors = motors();
        stale_motors.insert(
            "gripper".to_owned(),
            MotorSpec::new(99, MotorDirection::Normal).with_model("sts3215"),
        );
        let stale = calibration_for(&stale_motors, 2048, 500);

        let stale_specification = RobotSpec::new(
            "stale".to_owned(),
            PortSettings::try_new("/dev/stale", 1_000_000).expect("port"),
            motors(),
        )
        .expect("robot definition");
        let error = stale_specification
            .with_calibration(stale.clone())
            .expect_err("stale calibration must not become a robot specification");
        assert!(error.to_string().contains("does not match"), "{error}");
        assert!(controller.robots.is_empty());

        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect with matching calibration");
        let original = controller.robots["arm"].spec.calibration.clone();
        let error = controller
            .apply_calibration("arm", stale.clone())
            .expect_err("live replacement must also be checked");
        assert!(error.contains("does not match"), "{error}");
        assert_eq!(controller.robots["arm"].spec.calibration, original);

        controller.robots.get_mut("arm").unwrap().spec.calibration = Some(stale);
        let error = controller
            .require_calibrated("arm")
            .expect_err("command guard must revalidate the binding");
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn hold_and_release_change_mode_and_torque_together() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");

        controller.hold("arm").expect("hold");
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Manual {
                robot: "arm".to_owned()
            }
        );
        assert_eq!(controller.robots["arm"].motion.torque, TorqueState::Engaged);

        controller.release("arm").expect("release");
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert_eq!(
            controller.robots["arm"].motion.torque,
            TorqueState::Released
        );
    }

    #[test]
    fn failed_hold_compensates_physically_and_never_claims_an_unknown_release() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        let calls = install_failing_enable_bus(&mut controller, false);

        let error = controller
            .hold("arm")
            .expect_err("enable verification fails");
        assert_eq!(*calls.lock().unwrap(), [true, false]);
        assert!(
            error.contains("compensating torque-off was verified"),
            "{error}"
        );
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert_eq!(
            controller.robots["arm"].motion.torque,
            TorqueState::Released
        );

        let calls = install_failing_enable_bus(&mut controller, true);
        let error = controller
            .hold("arm")
            .expect_err("release verification fails");
        assert_eq!(*calls.lock().unwrap(), [true, false]);
        assert!(
            error.contains("physical torque state is unknown"),
            "{error}"
        );
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert_eq!(controller.robots["arm"].motion.torque, TorqueState::Unknown);
        assert!(!controller.robots["arm"].telemetry().torque());
        controller
            .set_mode(ModeSelection::Manual {
                robot: "arm".to_owned(),
            })
            .expect("mode alone does not resolve physical torque");
        assert!(
            controller
                .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 0.0)]))
                .expect_err("unknown torque state must not authorize motion")
                .contains("physical torque state is unknown")
        );
    }

    #[test]
    fn detach_removes_a_robot_only_after_verified_torque_off() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.set_torque("arm", true).expect("torque on");
        let calls = install_failing_enable_bus(&mut controller, false);

        controller
            .detach_robot("arm", "operator disconnected it")
            .expect("verified release permits detach");
        assert_eq!(*calls.lock().unwrap(), [false]);
        assert!(!controller.robots.contains_key("arm"));
    }

    #[test]
    fn failed_torque_off_refuses_detach_and_keeps_the_bus_for_retry() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.set_torque("arm", true).expect("torque on");
        let calls = install_failing_enable_bus(&mut controller, true);

        let error = controller
            .detach_robot("arm", "operator disconnected it")
            .expect_err("unknown physical state cannot be acknowledged as detached");
        assert!(
            error.contains("torque-off could not be verified"),
            "{error}"
        );
        assert_eq!(*calls.lock().unwrap(), [false]);
        assert!(controller.robots.contains_key("arm"));
        assert_eq!(controller.robots["arm"].motion.torque, TorqueState::Unknown);
    }

    #[test]
    fn commands_require_torque_and_accept_the_normalized_boundary() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller
            .set_mode(ModeSelection::Manual {
                robot: "arm".to_owned(),
            })
            .expect("manual mode");

        assert!(
            controller
                .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 1.0)]))
                .expect_err("torque is still off")
                .contains("enable torque")
        );

        controller.set_torque("arm", true).expect("torque on");
        controller
            .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 1.0)]))
            .expect("the exact normalized boundary is valid");

        assert_eq!(controller.robots["arm"].motion.target["gripper"], 2548);
    }

    #[test]
    fn hardware_rejects_out_of_range_normalized_goals_without_clamping() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.hold("arm").expect("hold");
        let before = controller.robots["arm"].motion.target.clone();

        for value in [-1.000_001, 1.000_001, f64::NAN] {
            let error = controller
                .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), value)]))
                .expect_err("invalid normalized goal must fail closed");
            assert!(error.contains("normalized [-1, 1]"), "{error}");
            assert_eq!(controller.robots["arm"].motion.target, before);
        }
    }

    #[test]
    fn policy_commands_are_scoped_to_the_admitted_session() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller
            .set_mode(ModeSelection::Policy {
                robot: "arm".to_owned(),
                session: "rollout-1".to_owned(),
            })
            .expect("policy mode");
        assert_eq!(controller.robots["arm"].motion.torque, TorqueState::Engaged);
        let robot = controller.robots.get_mut("arm").unwrap();
        robot.motion.positions = robot
            .spec
            .motors
            .keys()
            .map(|name| (name.clone(), 2048))
            .collect();
        robot.follow_measured_positions();

        let goals = BTreeMap::from([("gripper".to_owned(), 0.5)]);
        assert!(
            controller
                .set_policy_goals(&policy_goals("arm", "old-rollout", goals.clone()))
                .expect_err("stale session")
                .contains("not allowed")
        );
        controller
            .set_policy_goals(&policy_goals("arm", "rollout-1", goals.clone()))
            .expect("active session may command");
        assert_eq!(controller.robots["arm"].motion.target["gripper"], 2298);
        let before = controller.robots["arm"].motion.commanded["gripper"];
        controller.drive_robots();
        let after = controller.robots["arm"].motion.commanded["gripper"];
        assert_ne!(
            after, before,
            "the admitted policy target reaches the motor ramp"
        );
        assert!(
            after.abs_diff(before) <= controller.settings.teleop_max_step_counts.unsigned_abs(),
            "policy motion stays inside the fast control-path step bound"
        );

        let measured = after.saturating_sub(1);
        controller
            .robots
            .get_mut("arm")
            .unwrap()
            .motion
            .positions
            .insert("gripper".to_owned(), measured);

        controller
            .set_mode(ModeSelection::Idle)
            .expect("stop policy");
        assert!(
            controller.robots["arm"].motion.torque.is_engaged(),
            "stopping policy control must not release the arm"
        );
        assert_eq!(
            controller.robots["arm"].motion.target["gripper"], measured,
            "stopping policy control holds the arm at its measured position"
        );
        assert!(
            controller
                .set_policy_goals(&policy_goals("arm", "rollout-1", goals.clone()))
                .expect_err("finished session")
                .contains("not allowed")
        );
    }

    #[test]
    fn commanded_positions_ramp_toward_their_target() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller
            .set_mode(ModeSelection::Manual {
                robot: "arm".to_owned(),
            })
            .expect("manual mode");
        controller.set_torque("arm", true).expect("torque on");
        // Park the arm at center so the step under test has a known direction.
        let robot = controller.robots.get_mut("arm").unwrap();
        robot.motion.positions = robot
            .spec
            .motors
            .keys()
            .map(|name| (name.clone(), 2048))
            .collect();
        robot.follow_measured_positions();
        controller
            .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 1.0)]))
            .expect("goal");

        let max_step = controller.settings.max_step_counts;
        controller.robots.get_mut("arm").unwrap().drive(max_step);
        let after_one_step = controller.robots["arm"].motion.commanded["gripper"];

        assert_eq!(after_one_step - 2048, max_step, "one bounded step");
        for _ in 0..200 {
            controller.robots.get_mut("arm").unwrap().drive(max_step);
        }
        assert_eq!(controller.robots["arm"].motion.commanded["gripper"], 2548);
    }

    #[test]
    fn teleoperation_uses_the_faster_bounded_follower_step() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 500))))
            .expect("connect leader");
        controller
            .connect(spec("follower", Some(calibration(2048, 500))))
            .expect("connect follower");
        controller
            .set_mode(ModeSelection::Teleoperate {
                leader: "leader".to_owned(),
                follower: "follower".to_owned(),
            })
            .expect("teleoperation");

        let robot = controller.robots.get_mut("follower").unwrap();
        robot.motion.positions.insert("gripper".to_owned(), 2048);
        robot.motion.commanded.insert("gripper".to_owned(), 2048);
        robot.motion.target.insert("gripper".to_owned(), 3048);
        controller.drive_robots();

        assert_eq!(
            controller.robots["follower"].motion.commanded["gripper"] - 2048,
            controller.settings.teleop_max_step_counts
        );
        assert!(
            controller.settings.teleop_max_step_counts > controller.settings.max_step_counts,
            "teleoperation should catch up faster than a manual slider move"
        );
    }

    #[test]
    fn an_idle_robot_keeps_its_target_pinned_to_reality() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");

        // Torque is off, so the test arm is back-driven; polling must keep
        // the target following it so energizing later cannot cause a jump.
        for _ in 0..20 {
            controller.poll_robots();
        }
        let robot = &controller.robots["arm"];
        assert_eq!(robot.motion.target, robot.motion.positions);
        assert_eq!(robot.motion.commanded, robot.motion.positions);
    }

    #[test]
    fn teleoperation_maps_the_leader_pose_onto_a_differently_calibrated_follower() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 400))))
            .expect("connect leader");
        controller
            .connect(spec("follower", Some(calibration(1500, 800))))
            .expect("connect follower");
        controller
            .set_mode(ModeSelection::Teleoperate {
                leader: "leader".to_owned(),
                follower: "follower".to_owned(),
            })
            .expect("teleop");

        // Put the leader at the top of its travel and let the mode map it.
        let leader = controller.robots.get_mut("leader").unwrap();
        leader.motion.positions.insert("gripper".to_owned(), 2448);
        controller.apply_mode();

        assert!(controller.published.mode.active());
        assert_eq!(
            controller.robots["follower"].motion.target["gripper"], 2300,
            "the follower's own range, not the leader's counts"
        );
    }

    #[test]
    fn entering_teleoperation_holds_the_follower_and_frees_the_leader() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 400))))
            .expect("connect leader");
        controller
            .connect(spec("follower", Some(calibration(2048, 400))))
            .expect("connect follower");
        // Start from the worst case: exactly backwards.
        controller.set_torque("leader", true).expect("torque on");

        controller
            .set_mode(ModeSelection::Teleoperate {
                leader: "leader".to_owned(),
                follower: "follower".to_owned(),
            })
            .expect("teleop");

        assert!(
            !controller.robots["leader"].motion.torque.is_engaged(),
            "the leader has to be free to move by hand"
        );
        assert!(
            controller.robots["follower"].motion.torque.is_engaged(),
            "the follower has to hold, or the mode does nothing"
        );
        // Holding means holding where it stands, not where it was last sent.
        let follower = &controller.robots["follower"];
        assert_eq!(follower.motion.target, follower.motion.positions);

        // Leaving the mode does not release the follower: torque is what keeps
        // an arm from falling, so dropping it is always the operator's call.
        controller.set_mode(ModeSelection::Idle).expect("idle");
        assert_eq!(
            controller.robots["follower"].motion.torque,
            TorqueState::Engaged
        );
    }

    #[test]
    fn teleoperation_needs_two_calibrated_robots() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 400))))
            .expect("connect leader");
        controller.connect(spec("follower", None)).expect("connect");

        assert!(
            controller
                .set_mode(ModeSelection::Teleoperate {
                    leader: "leader".to_owned(),
                    follower: "leader".to_owned(),
                })
                .expect_err("same robot twice")
                .contains("two different robots")
        );
        assert!(
            controller
                .set_mode(ModeSelection::Teleoperate {
                    leader: "leader".to_owned(),
                    follower: "follower".to_owned(),
                })
                .expect_err("uncalibrated follower")
                .contains("calibrated")
        );
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
    }

    #[test]
    fn losing_an_arm_stops_the_mode_that_depended_on_it() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 400))))
            .expect("connect leader");
        controller
            .connect(spec("follower", Some(calibration(2048, 400))))
            .expect("connect follower");
        controller
            .set_mode(ModeSelection::Teleoperate {
                leader: "leader".to_owned(),
                follower: "follower".to_owned(),
            })
            .expect("teleop");

        controller.robots.get_mut("leader").unwrap().health.failures = MAX_CONSECUTIVE_FAILURES;
        controller.robots.get_mut("leader").unwrap().health.error =
            Some("bus went away".to_owned());
        controller.drop_failed_robots();

        assert!(!controller.robots.contains_key("leader"));
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert!(!controller.published.mode.active());
        assert!(
            controller
                .published
                .mode
                .error()
                .unwrap()
                .contains("stopped")
        );
        let connection = &controller.published.connections["leader"];
        assert!(!connection.connected());
        assert_eq!(connection.error(), Some("bus went away"));
    }

    #[test]
    fn calibration_releases_torque_and_blocks_driving() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 400))))
            .expect("connect");
        controller
            .set_mode(ModeSelection::Manual {
                robot: "arm".to_owned(),
            })
            .expect("manual mode");
        controller.set_torque("arm", true).expect("torque on");

        controller.start_calibration("arm").expect("start");

        assert_eq!(
            controller.robots["arm"].motion.torque,
            TorqueState::Released
        );
        assert_eq!(
            controller.published.mode.selection().clone(),
            ModeSelection::Idle
        );
        assert!(
            controller
                .set_torque("arm", true)
                .expect_err("torque during a sweep")
                .contains("calibration")
        );
        assert!(
            controller
                .set_mode(ModeSelection::Manual {
                    robot: "arm".to_owned()
                })
                .expect_err("mode during a sweep")
                .contains("calibration is in progress")
        );
    }

    #[test]
    fn a_sweep_becomes_calibration_input_once_every_joint_has_moved() {
        let mut controller = controller();
        controller.connect(spec("arm", None)).expect("connect");
        controller.start_calibration("arm").expect("start");

        assert!(
            controller
                .calibration_input("arm")
                .expect_err("nothing has moved yet")
                .contains("did not move far enough")
        );

        // A refused read keeps the sweep, so the operator can keep going.
        assert!(controller.robots["arm"].sweep.is_some());

        // The test arm drifts through its travel while torque is off.
        for _ in 0..500 {
            controller.poll_robots();
        }

        let input = controller.calibration_input("arm").expect("finish");
        let CalibrationInput::Direct { motors } = input else {
            panic!("a sweep produces direct readings");
        };
        assert_eq!(motors.len(), 6);
        assert!(
            motors
                .values()
                .all(|reading| reading.range_max() > reading.range_min())
        );

        // Reading the ranges must not end the sweep: the caller still has to
        // build and store a calibration, and either can fail.
        assert!(
            controller.robots["arm"].sweep.is_some(),
            "the sweep survives until the caller clears it"
        );
        assert_eq!(
            controller.calibration_input("arm").expect("read again"),
            CalibrationInput::Direct { motors },
            "reading twice gives the same answer"
        );
    }

    #[test]
    fn telemetry_reports_angles_only_for_calibrated_joints() {
        let mut controller = controller();
        controller
            .connect(spec("calibrated", Some(calibration(2048, 1024))))
            .expect("connect");
        controller.connect(spec("raw", None)).expect("connect");
        // Park both arms at center so the reported angles are exact.
        for robot in controller.robots.values_mut() {
            robot.motion.positions = robot
                .spec
                .motors
                .keys()
                .map(|name| (name.clone(), 2048))
                .collect();
        }
        controller.publish();

        let state = controller.state.borrow().clone();
        let calibrated = state
            .robots()
            .iter()
            .find(|robot| robot.id() == "calibrated")
            .expect("calibrated telemetry");
        let raw = state
            .robots()
            .iter()
            .find(|robot| robot.id() == "raw")
            .expect("raw telemetry");

        assert_eq!(calibrated.joints().len(), 6);
        assert_eq!(calibrated.joints()[0].degrees(), Some(0.0));
        assert_eq!(calibrated.joints()[0].normalized(), Some(0.0));
        assert_eq!(
            calibrated.joints()[0].range(),
            Some(JointRange::new(1024, 2048, 3072))
        );
        assert!(raw.joints().iter().all(|joint| joint.degrees().is_none()));
        assert!(raw.joints().iter().all(|joint| joint.goal_raw().is_none()));
        assert!(
            raw.joints()
                .iter()
                .all(|joint| joint.target_raw().is_none())
        );
        assert!(
            raw.joints()
                .iter()
                .all(|joint| joint.target_degrees().is_none())
        );
    }

    #[test]
    fn an_uncalibrated_arm_still_reports_what_its_motors_say_about_themselves() {
        let mut controller = controller();
        controller.connect(spec("raw", None)).expect("connect");
        controller.poll_robots();
        controller.publish();

        let state = controller.state.borrow().clone();
        let raw = &state.robots()[0];

        // Calibration is what turns counts into degrees; it has nothing to do
        // with the motor's own condition, and an arm nobody has calibrated yet
        // is exactly the arm whose wiring you want to check.
        assert!(raw.joints().iter().all(|joint| joint.degrees().is_none()));
        assert!(
            raw.joints()
                .iter()
                .all(|joint| joint.temperature_celsius().is_some()),
            "temperature does not wait for a calibration"
        );
        assert!(
            raw.joints()
                .iter()
                .all(|joint| joint.voltage_volts().is_some())
        );
        assert!(raw.joints().iter().all(|joint| joint.faults().is_empty()));

        let limits = &state.limits["raw"];
        assert_eq!(limits.len(), 6);
        assert!(
            limits
                .values()
                .all(|limits| limits.max_temperature_celsius().is_some()),
            "limits are read from the motors when the robot connects"
        );
    }

    #[test]
    fn a_transient_load_spike_survives_to_the_next_published_frame() {
        let mut track = MotorTrack::default();
        let now = Instant::now();
        let idle = MotorSample::new(
            0,
            MotorMechanicalSample::new(None, Some(0.05)),
            MotorElectricalSample::new(None, None, Some(0.1)),
        );
        for _ in 0..40 {
            track.observe(idle, now);
        }
        let settled = track.load.expect("a settled load");

        track.observe(
            MotorSample::new(
                0,
                MotorMechanicalSample::new(None, Some(0.9)),
                MotorElectricalSample::new(None, None, Some(1.3)),
            ),
            now,
        );
        let spiked = track.load.expect("the spike");
        assert!(spiked > settled, "the spike moves the reading");

        // Two of every three samples never reach a client, so the spike has to
        // outlive the cycle it happened in or nobody ever sees a stall.
        track.observe(idle, now);
        track.observe(idle, now);
        assert!(
            track.load.expect("the held peak") > settled,
            "a spike is still visible two cycles later"
        );

        for _ in 0..40 {
            track.observe(idle, now);
        }
        assert!(
            (track.load.expect("back to rest") - settled).abs() < 0.02,
            "and it does not pin the readout at the spike forever"
        );
    }
}
