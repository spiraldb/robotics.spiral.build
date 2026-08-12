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

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use spiral_robotics::bus::{MotorBus, SimulatedBus};
use spiral_robotics::feetech::{self, FeetechBus, MotorSample};

use crate::backend::recording::episodes::{
    ArmConditionSummary, EpisodeCheckpoint, EpisodeWriter, Sample,
};
use spiral_robotics_core::api::{
    CAUTION_FOLLOW_ERROR_DEGREES, CalibrationJointProgress, CalibrationProgress, ConnectionStatus,
    EpisodeArm, EpisodeSummary, JointRange, JointTelemetry, ModeSelection, ModeStatus, MotorFault,
    MotorLimits, PortSettings, RecordingGap, RecordingProgress, ReplayPhase, ReplayProgress,
    ReplayPurpose, RobotTelemetry, TelemetryFrame,
};
use spiral_robotics_core::arm;
use spiral_robotics_core::calibration::{
    Calibration, CalibrationError, CalibrationInput, Direction, MotorCalibration, MotorSpec,
    RangeTracker, denormalize_centered_raw, normalize_centered_raw,
};
use tokio::sync::{oneshot, watch};

/// How often the control loop runs.
pub(crate) const CONTROL_HZ: u64 = 60;
pub(crate) const CONTROL_PERIOD: Duration = Duration::from_nanos(1_000_000_000 / CONTROL_HZ);

/// Default ceiling on how far one joint may be commanded to move per cycle.
///
/// At the control rate above this is roughly 100 degrees per second, which is
/// brisk enough for teleoperation and slow enough to stay in front of.
pub(crate) const DEFAULT_MAX_STEP_COUNTS: i64 = 20;

/// Default teleoperation ceiling on follower movement per control cycle.
///
/// Manual slider moves stay deliberately gentle. A follower has to keep up
/// with ordinary hand motion, so it gets a faster, still-bounded ramp instead
/// of accumulating a large pose error behind the leader.
pub(crate) const DEFAULT_TELEOP_MAX_STEP_COUNTS: i64 = 40;

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
pub(crate) struct RobotSpec {
    pub id: String,
    pub port: PortSettings,
    pub motors: BTreeMap<String, MotorSpec>,
    pub calibration: Option<Calibration>,
}

/// Tuning for the control loop.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlSettings {
    pub period: Duration,
    /// Largest change in encoder counts applied to a joint in one cycle.
    pub max_step_counts: i64,
    /// Largest per-cycle change while an arm is the teleoperation follower.
    pub teleop_max_step_counts: i64,
    /// Drive simulated motors instead of opening serial ports.
    pub simulated: bool,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            period: CONTROL_PERIOD,
            max_step_counts: DEFAULT_MAX_STEP_COUNTS,
            teleop_max_step_counts: DEFAULT_TELEOP_MAX_STEP_COUNTS,
            simulated: false,
        }
    }
}

/// A published sample of everything the control thread knows.
#[derive(Clone, Debug, Default)]
pub(crate) struct HardwareState {
    pub telemetry: TelemetryFrame,
    /// Every robot the thread has been asked to connect, including ones whose
    /// bus has since been lost, so the reason stays visible.
    pub connections: BTreeMap<String, ConnectionStatus>,
    pub mode: ModeStatus,
    /// What each connected robot's motors said they will fault at.
    ///
    /// Keyed robot then motor. These ride the snapshot rather than the
    /// telemetry frame because they are read once at connect and do not change
    /// while an arm runs; publishing them twenty times a second would be
    /// publishing a constant.
    pub limits: BTreeMap<String, BTreeMap<String, MotorLimits>>,
    /// Why the last take stopped on its own, kept until another one starts.
    ///
    /// A take cut short by an arm going away has to survive the moment it
    /// happens: the recording counters vanish with the recording, and an
    /// operator looking up a second later would otherwise see nothing at all.
    pub recording_issue: Option<String>,
}

struct ReplayPlan {
    frames: Vec<BTreeMap<String, f64>>,
    sample_hz: f64,
    start_frame: usize,
    resolved_start_seconds: f64,
    purpose: ReplayPurpose,
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
    SetTorque {
        id: String,
        enabled: bool,
        reply: Reply<()>,
    },
    SetGoals {
        id: String,
        normalized: BTreeMap<String, f64>,
        reply: Reply<()>,
    },
    SetPolicyGoals {
        id: String,
        session: String,
        normalized: BTreeMap<String, f64>,
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
        writer: Box<EpisodeWriter>,
        arms: Vec<EpisodeArm>,
        reply: Reply<()>,
    },
    StopRecording {
        keep: bool,
        reply: Reply<EpisodeSummary>,
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
        robot: String,
        episode: String,
        plan: ReplayPlan,
        reply: Reply<()>,
    },
    Shutdown {
        reply: Reply<()>,
    },
}

/// A take being captured by the control loop.
struct Recording {
    writer: EpisodeWriter,
    /// Arms sampled, fixed when the take started so every frame has one shape.
    arms: Vec<EpisodeArm>,
    /// Control mode suspended for review and restored when capture resumes.
    resume_mode: ModeSelection,
    started: Instant,
    /// Start of the current wall-clock omission, when capture is paused.
    paused_at: Option<Instant>,
    /// Completed pause time omitted from the active sample clock.
    paused_duration: Duration,
    /// Deliberate wall-clock omissions, persisted with the finished take.
    gaps: Vec<RecordingGap>,
    /// Samples handed to the writer.
    frames: u64,
    /// Largest `Follow Δ` since the arm first caught up, in degrees.
    ///
    /// A recording almost always opens with the arm travelling — to a fresh
    /// manual goal, or to wherever the leader is standing. That transient is
    /// real, stays in the file, and says nothing about how well the take
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
    /// The worst each motor has seen so far, in the arms' joint order.
    ///
    /// Accumulated as the take runs so the footer can state it without a second
    /// pass over a file that may be a hundred thousand lines long.
    condition: Vec<ArmConditionSummary>,
}

impl Recording {
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
        self.gaps.push(RecordingGap {
            after_seconds,
            duration_seconds: duration.as_secs_f64(),
        });
    }

    /// Fold one instant's tracking error into the take's summary.
    fn observe(&mut self, error: f64) {
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

    /// Fold one arm's motor readings into the take's running extremes.
    fn observe_condition(&mut self, index: usize, joints: &[String], robot: &ConnectedRobot) {
        let Some(summary) = self.condition.get_mut(index) else {
            return;
        };
        for (slot, name) in joints.iter().enumerate() {
            let Some(track) = robot.tracks.get(name) else {
                continue;
            };
            worst(
                &mut summary.peak_temperature_celsius,
                slot,
                track.sample.temperature_celsius,
                f64::max,
            );
            worst(
                &mut summary.min_voltage_volts,
                slot,
                track.sample.voltage_volts,
                f64::min,
            );
            // Effort is signed by which way the motor is pushing; the extreme
            // worth keeping is how hard, not which way.
            worst(
                &mut summary.peak_load,
                slot,
                track.load.map(f64::abs),
                f64::max,
            );
            worst(
                &mut summary.peak_current_amps,
                slot,
                track.current,
                f64::max,
            );
        }
    }

    /// The take's headline tracking error.
    fn follow_error(&self) -> Option<f64> {
        self.max_follow_error.or(self.unsettled_peak)
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
        self.position as usize
    }

    fn finished(&self) -> bool {
        self.phase == ReplayPhase::Playing && self.cursor() >= self.frames.len()
    }

    fn duration_seconds(&self) -> f64 {
        self.frames.len() as f64 * self.frame_period
    }

    fn elapsed_seconds(&self) -> f64 {
        self.resolved_start_seconds
            + (self.position - self.start_position).max(0.0) * self.frame_period
    }
}

/// The asynchronous side of the control thread.
#[derive(Debug)]
pub(crate) struct HardwareHandle {
    commands: Sender<Command>,
    state: watch::Receiver<HardwareState>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl HardwareHandle {
    /// Start the control thread.
    pub(crate) fn spawn(settings: ControlSettings) -> Self {
        let (commands, requests) = mpsc::channel();
        let (state, receiver) = watch::channel(HardwareState::default());
        let thread = thread::Builder::new()
            .name("spiral-robotics-control".to_owned())
            .spawn(move || Controller::new(settings, state).run(&requests))
            .expect("the control thread must start");

        Self {
            commands,
            state: receiver,
            thread: Mutex::new(Some(thread)),
        }
    }

    /// Read the control thread's most recent state.
    pub(crate) fn state(&self) -> HardwareState {
        self.state.borrow().clone()
    }

    /// Open a robot's motor bus.
    pub(crate) async fn connect(&self, spec: RobotSpec) -> Result<(), String> {
        self.request(|reply| Command::Connect {
            spec: Box::new(spec),
            reply,
        })
        .await
    }

    /// Close a robot's motor bus.
    pub(crate) async fn disconnect(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Disconnect {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Switch a robot's holding torque.
    pub(crate) async fn set_torque(&self, id: &str, enabled: bool) -> Result<(), String> {
        self.request(|reply| Command::SetTorque {
            id: id.to_owned(),
            enabled,
            reply,
        })
        .await
    }

    /// Command joint targets in normalized units.
    pub(crate) async fn set_goals(
        &self,
        id: &str,
        normalized: BTreeMap<String, f64>,
    ) -> Result<(), String> {
        self.request(|reply| Command::SetGoals {
            id: id.to_owned(),
            normalized,
            reply,
        })
        .await
    }

    /// Command normalized targets for the currently admitted policy session.
    pub(crate) async fn set_policy_goals(
        &self,
        id: &str,
        session: &str,
        normalized: BTreeMap<String, f64>,
    ) -> Result<(), String> {
        self.request(|reply| Command::SetPolicyGoals {
            id: id.to_owned(),
            session: session.to_owned(),
            normalized,
            reply,
        })
        .await
    }

    /// Leave policy mode only when the named session still owns it.
    pub(crate) async fn stop_policy_session(&self, session: &str) -> Result<(), String> {
        self.request(|reply| Command::StopPolicySession {
            session: session.to_owned(),
            reply,
        })
        .await
    }

    /// Select the operating mode.
    pub(crate) async fn set_mode(&self, mode: ModeSelection) -> Result<(), String> {
        self.request(|reply| Command::SetMode { mode, reply }).await
    }

    /// Disable torque and begin recording a joint-range sweep.
    pub(crate) async fn start_calibration(&self, id: &str) -> Result<(), String> {
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
    pub(crate) async fn calibration_input(&self, id: &str) -> Result<CalibrationInput, String> {
        self.request(|reply| Command::CalibrationInputFor {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Drop every trace of a robot, including why its bus was lost.
    pub(crate) async fn forget(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::Forget {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Abandon a sweep without producing a calibration.
    pub(crate) async fn cancel_calibration(&self, id: &str) -> Result<(), String> {
        self.request(|reply| Command::CancelCalibration {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Begin sampling the named arms into an already-opened take.
    ///
    /// The writer is created by the caller because the episode store owns the
    /// directory, and handed over whole because only this thread runs at the
    /// rate a take has to be sampled at.
    pub(crate) async fn start_recording(
        &self,
        writer: EpisodeWriter,
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
    pub(crate) async fn stop_recording(&self, keep: bool) -> Result<EpisodeSummary, String> {
        self.request(|reply| Command::StopRecording { keep, reply })
            .await
    }

    /// Pause or resume sampling while keeping the same open take and ID.
    pub(crate) async fn set_recording_paused(&self, paused: bool) -> Result<f64, String> {
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

    /// Play a recorded trajectory onto one arm.
    ///
    /// The samples arrive already loaded and already in normalized units, which
    /// is the only unit anything here can be commanded in.
    pub(crate) async fn start_replay(
        &self,
        robot: &str,
        episode: &str,
        frames: Vec<BTreeMap<String, f64>>,
        sample_hz: f64,
    ) -> Result<(), String> {
        self.start_replay_at_frame(robot, episode, frames, sample_hz, 0, 0.0)
            .await
    }

    /// Play a recorded trajectory beginning at one resolved trajectory frame.
    pub(crate) async fn start_replay_at_frame(
        &self,
        robot: &str,
        episode: &str,
        frames: Vec<BTreeMap<String, f64>>,
        sample_hz: f64,
        start_frame: usize,
        resolved_start_seconds: f64,
    ) -> Result<(), String> {
        self.start_replay_for(
            robot,
            episode,
            ReplayPlan {
                frames,
                sample_hz,
                start_frame,
                resolved_start_seconds,
                purpose: ReplayPurpose::Timeline,
            },
        )
        .await
    }

    /// Move to one committed historical pose without creating Timeline transport.
    pub(crate) async fn start_physical_seek(
        &self,
        robot: &str,
        episode: &str,
        pose: BTreeMap<String, f64>,
        sample_hz: f64,
    ) -> Result<(), String> {
        self.start_replay_for(
            robot,
            episode,
            ReplayPlan {
                frames: vec![pose],
                sample_hz,
                start_frame: 0,
                resolved_start_seconds: 0.0,
                purpose: ReplayPurpose::PhysicalSeek,
            },
        )
        .await
    }

    async fn start_replay_for(
        &self,
        robot: &str,
        episode: &str,
        plan: ReplayPlan,
    ) -> Result<(), String> {
        self.request(|reply| Command::StartReplay {
            robot: robot.to_owned(),
            episode: episode.to_owned(),
            plan,
            reply,
        })
        .await
    }

    /// Pause or resume the loaded replay without returning to frame zero.
    pub(crate) async fn set_replay_paused(&self, paused: bool) -> Result<(), String> {
        self.request(|reply| Command::SetReplayPaused { paused, reply })
            .await
    }

    /// Adopt a calibration that has just been written to disk.
    pub(crate) async fn apply_calibration(
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
    pub(crate) async fn shutdown(&self) {
        let _ = self.request(|reply| Command::Shutdown { reply }).await;
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(thread) = thread {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
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
        self.speed = smooth(self.speed, sample.speed_degrees_per_second);
        self.load = hold_peak(self.load, smooth(self.load, sample.load), f64::abs);
        self.current = hold_peak(
            self.current,
            smooth(self.current, sample.current_amps),
            |amps| amps,
        );
        if let Some(celsius) = sample.temperature_celsius {
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
}

/// Keep the more extreme of a slot's running value and a new reading.
///
/// A slot that has never had a reading stays `None` rather than becoming zero:
/// a take whose bus reports no temperature must not be readable a week later as
/// a take that ran at nought degrees.
fn worst(slots: &mut [Option<f64>], slot: usize, reading: Option<f64>, keep: fn(f64, f64) -> f64) {
    let Some(reading) = reading else { return };
    let Some(held) = slots.get_mut(slot) else {
        return;
    };
    *held = Some(held.map_or(reading, |held| keep(held, reading)));
}

/// Restate a bus-level limit on the wire.
///
/// The driver and the wire describe the same quantities and deliberately do not
/// share a type: the wire contract is the product boundary, and it must not
/// change shape because a motor SDK did.
fn convert_limits(limits: feetech::MotorLimits) -> MotorLimits {
    MotorLimits {
        max_temperature_celsius: limits.max_temperature_celsius,
        min_voltage_volts: limits.min_voltage_volts,
        max_voltage_volts: limits.max_voltage_volts,
        max_load: limits.max_load,
    }
}

/// Restate the servo's fault byte as the faults the wire names.
fn convert_faults(faults: feetech::MotorFaults) -> Vec<MotorFault> {
    let mut named = faults
        .named()
        .into_iter()
        .map(|fault| match fault {
            feetech::MotorFault::Overheat => MotorFault::Overheat,
            feetech::MotorFault::Overload => MotorFault::Overload,
            feetech::MotorFault::Voltage => MotorFault::Voltage,
        })
        .collect::<Vec<_>>();
    // A bit this build cannot name is still a motor saying something is wrong,
    // and dropping it would report a faulted arm as healthy.
    if faults.unnamed() != 0 {
        named.push(MotorFault::Unrecognized);
    }
    named
}

/// Fold `next` into a smoothed reading, starting from it when there is none.
fn smooth(previous: Option<f64>, next: Option<f64>) -> Option<f64> {
    let next = next?;
    Some(match previous {
        Some(previous) if previous.is_finite() => previous + (next - previous) * READING_SMOOTHING,
        _ => next,
    })
}

/// Keep the larger of a decaying held peak and the current reading.
///
/// `magnitude` says what "larger" means, so a signed reading holds the peak of
/// its size while keeping the direction it was pushing in.
fn hold_peak(held: Option<f64>, next: Option<f64>, magnitude: impl Fn(f64) -> f64) -> Option<f64> {
    let next = next?;
    match held {
        Some(held) if magnitude(held * PEAK_HOLD_DECAY) > magnitude(next) => {
            Some(held * PEAK_HOLD_DECAY)
        }
        _ => Some(next),
    }
}

struct ConnectedRobot {
    spec: RobotSpec,
    bus: Box<dyn MotorBus>,
    torque: bool,
    positions: BTreeMap<String, i64>,
    /// What each motor reports about itself, smoothed for display.
    tracks: BTreeMap<String, MotorTrack>,
    /// What each motor said it will fault at, read once when it connected.
    limits: BTreeMap<String, MotorLimits>,
    /// Where each joint should end up.
    target: BTreeMap<String, i64>,
    /// The last goal actually written, which ramps toward `target`.
    commanded: BTreeMap<String, i64>,
    error: Option<String>,
    failures: u32,
    sweep: Option<RangeTracker>,
}

impl ConnectedRobot {
    /// Pin intent to measured reality, so energizing never causes a jump.
    fn follow_measured_positions(&mut self) {
        self.target = self.positions.clone();
        self.commanded = self.positions.clone();
    }

    /// Step every commanded goal toward its target and write what changed.
    fn drive(&mut self, max_step: i64) {
        if !self.torque || self.target.is_empty() {
            return;
        }

        let mut writes = BTreeMap::new();
        for (name, target) in &self.target {
            let current = self
                .commanded
                .get(name)
                .or_else(|| self.positions.get(name))
                .copied()
                .unwrap_or(*target);
            let step = target.saturating_sub(current).clamp(-max_step, max_step);
            writes.insert(name.clone(), current.saturating_add(step));
        }

        if writes == self.commanded {
            return;
        }
        match self.bus.write_goal_positions(&writes) {
            Ok(()) => {
                self.commanded = writes;
                self.failures = 0;
            }
            Err(error) => {
                self.failures += 1;
                self.error = Some(error.to_string());
            }
        }
    }

    /// The robot's motors in bus-ID order, which for an SO-101 is also the
    /// order the joints appear along the arm.
    fn ordered_motors(&self) -> Vec<(&String, &MotorSpec)> {
        let mut motors = self.spec.motors.iter().collect::<Vec<_>>();
        motors.sort_by(|(left_name, left), (right_name, right)| {
            left.id
                .cmp(&right.id)
                .then_with(|| left_name.cmp(right_name))
        });
        motors
    }

    /// This instant's positions for the named joints, normalized and raw.
    ///
    /// The caller supplies the order, which comes from published telemetry, so a
    /// frame in the file lines up with the joint rows the operator was watching
    /// while it was captured.
    fn sample(&self, joints: &[String]) -> (Vec<Option<f64>>, Vec<i64>) {
        let calibration = self.spec.calibration.as_ref();
        let mut normalized = Vec::with_capacity(joints.len());
        let mut raw = Vec::with_capacity(joints.len());
        for joint in joints {
            let counts = self.positions.get(joint).copied().unwrap_or_default();
            raw.push(counts);
            normalized.push(
                calibration
                    .and_then(|calibration| calibration.motors.get(joint))
                    .and_then(|motor| normalize_centered_raw(motor, counts).ok()),
            );
        }
        (normalized, raw)
    }

    /// The normalized goal most recently sent to each named joint.
    ///
    /// A free arm has no action. Returning `None` for the whole arm preserves
    /// the difference between an observed leader pose and a follower command.
    fn sample_action(&self, joints: &[String]) -> Option<Vec<Option<f64>>> {
        if !self.torque {
            return None;
        }
        let calibration = self.spec.calibration.as_ref()?;
        Some(
            joints
                .iter()
                .map(|joint| {
                    let counts = self.commanded.get(joint).copied()?;
                    let motor = calibration.motors.get(joint)?;
                    normalize_centered_raw(motor, counts).ok()
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
        if !self.torque {
            return None;
        }
        let calibration = self.spec.calibration.as_ref()?;
        self.target
            .iter()
            .filter_map(|(name, target)| {
                let motor = calibration.motors.get(name)?;
                let measured = self.positions.get(name).copied()?;
                let error = (joint_degrees(motor, measured) - joint_degrees(motor, *target)).abs();
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
                let raw = self.positions.get(name).copied().unwrap_or_default();
                let motor = calibration.and_then(|calibration| calibration.motors.get(name));
                let track = self.tracks.get(name);
                let target_raw = self
                    .torque
                    .then(|| self.target.get(name).copied())
                    .flatten();
                JointTelemetry {
                    name: name.clone(),
                    id: spec.id,
                    raw,
                    degrees: motor.map(|motor| joint_degrees(motor, raw)),
                    normalized: motor.and_then(|motor| normalize_centered_raw(motor, raw).ok()),
                    goal_raw: self
                        .torque
                        .then(|| self.commanded.get(name).copied())
                        .flatten(),
                    target_raw,
                    target_degrees: target_raw
                        .zip(motor)
                        .map(|(target, motor)| joint_degrees(motor, target)),
                    range: motor.map(|motor| JointRange {
                        min: motor.range_min,
                        center: motor.center,
                        max: motor.range_max,
                    }),
                    // Everything below is what the motor says about itself, and
                    // none of it depends on calibration: these are the only
                    // readings an arm that has never been calibrated can give.
                    temperature_celsius: track.and_then(|track| track.sample.temperature_celsius),
                    voltage_volts: track.and_then(|track| track.sample.voltage_volts),
                    current_amps: track.and_then(|track| track.current),
                    load: track.and_then(|track| track.load),
                    speed_degrees_per_second: track.and_then(|track| track.speed),
                    temperature_rise_per_minute: track
                        .and_then(MotorTrack::temperature_rise_per_minute),
                    faults: track
                        .map(|track| convert_faults(track.sample.faults))
                        .unwrap_or_default(),
                }
            })
            .collect();

        RobotTelemetry {
            id: self.spec.id.clone(),
            torque: self.torque,
            joints,
            error: self.error.clone(),
            calibration: self.sweep.as_ref().map(|sweep| CalibrationProgress {
                joints: self
                    .ordered_motors()
                    .into_iter()
                    .filter_map(|(name, _)| Some((name, sweep.ranges().get(name)?)))
                    .map(|(name, range)| CalibrationJointProgress {
                        name: name.clone(),
                        min: range.minimum,
                        position: range.position,
                        max: range.maximum,
                    })
                    .collect(),
                ready: sweep.is_ready(),
            }),
        }
    }
}

/// Degrees from a joint's calibrated center, in the joint's own direction.
fn joint_degrees(motor: &MotorCalibration, raw: i64) -> f64 {
    let degrees = arm::counts_to_degrees(raw.saturating_sub(motor.center));
    match motor.direction {
        Direction::Normal => degrees,
        Direction::Reversed => -degrees,
    }
}

struct Controller {
    settings: ControlSettings,
    robots: BTreeMap<String, ConnectedRobot>,
    /// Connection state for every robot the thread has been asked about,
    /// including ones whose bus was lost.
    connections: BTreeMap<String, ConnectionStatus>,
    mode: ModeStatus,
    state: watch::Sender<HardwareState>,
    sequence: u64,
    loop_hz: f64,
    /// The take being captured, if any. Recording is deliberately not a mode:
    /// an operator records *while* teleoperating, and a take of an idle arm
    /// being moved by hand is a legitimate take too.
    recording: Option<Recording>,
    /// The episode being played back, if any. This one *is* a mode, because it
    /// drives motors and only one thing may do that at a time.
    replay: Option<Replay>,
    recording_issue: Option<String>,
}

impl Controller {
    fn new(settings: ControlSettings, state: watch::Sender<HardwareState>) -> Self {
        Self {
            settings,
            robots: BTreeMap::new(),
            connections: BTreeMap::new(),
            mode: ModeStatus::default(),
            state,
            sequence: 0,
            loop_hz: 0.0,
            recording: None,
            replay: None,
            recording_issue: None,
        }
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
                    self.loop_hz = if self.loop_hz == 0.0 {
                        1.0 / elapsed
                    } else {
                        1.0 / (0.9 / self.loop_hz + 0.1 * elapsed)
                    };
                }
            }

            loop {
                match requests.try_recv() {
                    Ok(Command::Shutdown { reply }) => {
                        let _ = reply.send(Ok(()));
                        return;
                    }
                    Ok(command) => self.handle(command),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            // In teleoperation the critical path is leader read -> follower
            // write. Reading every other arm between those two adds visible
            // latency for no control benefit, so service that path first and
            // collect the remaining telemetry immediately afterwards.
            if let ModeSelection::Teleoperate { leader, .. } = self.mode.selection.clone() {
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
                let removed = self.robots.remove(&id).is_some();
                if removed {
                    self.connections
                        .insert(id.clone(), ConnectionStatus::default());
                    self.leave_mode_involving(&id, "the robot was disconnected");
                }
                let _ = reply.send(if removed {
                    Ok(())
                } else {
                    Err(format!("robot {id:?} is not connected"))
                });
            }
            Command::SetTorque { id, enabled, reply } => {
                let _ = reply.send(self.set_torque(&id, enabled));
            }
            Command::SetGoals {
                id,
                normalized,
                reply,
            } => {
                let _ = reply.send(self.set_goals(&id, &normalized));
            }
            Command::SetPolicyGoals {
                id,
                session,
                normalized,
                reply,
            } => {
                let _ = reply.send(self.set_policy_goals(&id, &session, &normalized));
            }
            Command::StopPolicySession { session, reply } => {
                let result = if matches!(
                    &self.mode.selection,
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
                self.robots.remove(&id);
                self.connections.remove(&id);
                self.leave_mode_involving(&id, "the robot was removed");
                let _ = reply.send(Ok(()));
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
                let result = match self.robots.get_mut(&id) {
                    Some(robot) => {
                        robot.spec.calibration = Some(*calibration);
                        Ok(())
                    }
                    None => Err(format!("robot {id:?} is not connected")),
                };
                let _ = reply.send(result);
            }
            Command::StartRecording {
                writer,
                arms,
                reply,
            } => {
                let _ = reply.send(self.start_recording(*writer, arms));
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
            Command::StartReplay {
                robot,
                episode,
                plan,
                reply,
            } => {
                let _ = reply.send(self.start_replay_for(&robot, &episode, plan));
            }
            Command::Shutdown { reply } => {
                // A take in progress is worth more than a tidy exit: close it
                // so its samples land as a real episode rather than as a part
                // file the operator has to notice and reason about.
                if self.recording.is_some()
                    && let Err(error) = self.stop_recording(true)
                {
                    tracing::warn!("could not close the take in progress: {error}");
                }
                let _ = reply.send(Ok(()));
            }
        }
    }

    fn start_recording(
        &mut self,
        writer: EpisodeWriter,
        arms: Vec<EpisodeArm>,
    ) -> Result<(), String> {
        if let Some(existing) = &self.recording {
            return Err(format!(
                "take {} is already recording; stop it first",
                existing.writer.id()
            ));
        }
        // Every named arm has to be open right now, or the take would begin
        // with a hole in it.
        for arm in &arms {
            if !self.robots.contains_key(&arm.id) {
                return Err(format!("robot {:?} is not connected", arm.id));
            }
        }

        // One slot per joint, sized here so every fold below is an index into a
        // list that already has the right shape.
        let condition = arms
            .iter()
            .map(|arm| ArmConditionSummary {
                arm: arm.id.clone(),
                peak_temperature_celsius: vec![None; arm.joints.len()],
                min_voltage_volts: vec![None; arm.joints.len()],
                peak_load: vec![None; arm.joints.len()],
                peak_current_amps: vec![None; arm.joints.len()],
            })
            .collect();

        self.recording_issue = None;
        self.recording = Some(Recording {
            writer,
            arms,
            resume_mode: self.mode.selection.clone(),
            started: Instant::now(),
            paused_at: None,
            paused_duration: Duration::ZERO,
            gaps: Vec::new(),
            frames: 0,
            max_follow_error: None,
            settled: false,
            unsettled_peak: None,
            condition,
        });
        Ok(())
    }

    fn stop_recording(&mut self, keep: bool) -> Result<EpisodeSummary, String> {
        let mut recording = self
            .recording
            .take()
            .ok_or_else(|| "no take is being recorded".to_owned())?;
        let now = Instant::now();
        recording.close_pause(now);
        let duration = recording.active_elapsed(now).as_secs_f64();
        let follow_error = recording.follow_error();
        let condition = recording.condition;
        recording
            .writer
            .finish(duration, recording.gaps, follow_error, !keep, condition)
            .map_err(|error| error.to_string())
    }

    fn set_recording_paused(
        &mut self,
        paused: bool,
    ) -> Result<(Option<EpisodeCheckpoint>, f64), String> {
        if paused {
            let (checkpoint, elapsed_seconds) = {
                let recording = self
                    .recording
                    .as_mut()
                    .ok_or_else(|| "no take is being recorded".to_owned())?;
                if recording.paused_at.is_none() {
                    recording.paused_at = Some(Instant::now());
                }
                // FIFO ordering means this barrier is processed after every
                // sample accepted before the pause. The async caller awaits it
                // without blocking this fixed-rate controller thread.
                (
                    recording.writer.checkpoint()?,
                    recording.active_elapsed(Instant::now()).as_secs_f64(),
                )
            };
            self.set_mode(ModeSelection::Idle)?;
            return Ok((Some(checkpoint), elapsed_seconds));
        }
        let resume_mode = self
            .recording
            .as_ref()
            .ok_or_else(|| "no take is being recorded".to_owned())?
            .resume_mode
            .clone();
        self.set_mode(resume_mode)?;
        let elapsed_seconds = self
            .recording
            .as_ref()
            .map(|recording| recording.active_elapsed(Instant::now()).as_secs_f64())
            .unwrap_or_default();
        if let Some(recording) = self.recording.as_mut()
            && recording.paused_at.is_some()
        {
            recording.close_pause(Instant::now());
        }
        Ok((None, elapsed_seconds))
    }

    /// Append this cycle's positions to the take in progress.
    fn capture_sample(&mut self) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };

        // An arm that left mid-take ends the take. Recording zeros in its place
        // would produce a file that looks complete and describes motion that
        // never happened.
        let missing = recording
            .arms
            .iter()
            .map(|arm| arm.id.clone())
            .find(|id| !self.robots.contains_key(id));
        if let Some(missing) = missing {
            let reason = format!("stopped recording: robot {missing:?} is no longer connected");
            match self.stop_recording(true) {
                Ok(summary) => {
                    self.recording_issue = Some(format!(
                        "{reason}. The {} frames captured before that are stored as {}",
                        summary.frames, summary.id
                    ));
                }
                Err(error) => self.recording_issue = Some(format!("{reason}. {error}")),
            }
            return;
        }

        if recording.paused_at.is_some() {
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
            let Some(robot) = self.robots.get(&arm.id) else {
                return;
            };
            let (arm_normalized, arm_raw) = robot.sample(&arm.joints);
            normalized.push(arm_normalized);
            raw.push(arm_raw);
            let (arm_velocity, arm_effort) = robot.sample_proprioception(&arm.joints);
            action.push(robot.sample_action(&arm.joints));
            proprioception &= arm_velocity.iter().chain(&arm_effort).all(Option::is_some);
            velocity.push(arm_velocity);
            effort.push(arm_effort);
            if let Some(error) = robot.follow_error_degrees() {
                instant_error = Some(instant_error.map_or(error, |worst: f64| worst.max(error)));
            }
            recording.observe_condition(index, &arm.joints, robot);
        }
        if let Some(error) = instant_error {
            recording.observe(error);
        }

        if recording.writer.push(Sample {
            seconds,
            normalized,
            raw,
            follow_error_degrees: instant_error,
            velocity: proprioception.then_some(velocity),
            effort: proprioception.then_some(effort),
            action,
        }) {
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
                self.recording_issue = Some(format!("stopped recording {}: {reason}", summary.id));
            }
            Err(error) => {
                self.recording_issue = Some(format!("stopped recording: {reason}. {error}"));
            }
        }
    }

    #[cfg(test)]
    fn start_replay(
        &mut self,
        robot: &str,
        episode: &str,
        frames: Vec<BTreeMap<String, f64>>,
        sample_hz: f64,
    ) -> Result<(), String> {
        self.start_replay_for(
            robot,
            episode,
            ReplayPlan {
                frames,
                sample_hz,
                start_frame: 0,
                resolved_start_seconds: 0.0,
                purpose: ReplayPurpose::Timeline,
            },
        )
    }

    fn start_replay_for(
        &mut self,
        robot: &str,
        episode: &str,
        plan: ReplayPlan,
    ) -> Result<(), String> {
        let ReplayPlan {
            frames,
            sample_hz,
            start_frame,
            resolved_start_seconds,
            purpose,
        } = plan;
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
        let calibration = self.require_calibrated(robot)?.clone();
        // Every joint in the take has to exist on this arm, or playback would
        // silently drop part of the motion it claims to be reproducing.
        let unknown = frames[0]
            .keys()
            .find(|joint| !calibration.motors.contains_key(*joint))
            .cloned();
        if let Some(joint) = unknown {
            return Err(format!(
                "robot {robot:?} has no calibrated joint {joint:?}, so it cannot replay {episode:?}"
            ));
        }

        // Set the mode through the same path every other mode goes through, so
        // replay inherits the calibration check, the sweep refusal, the
        // target reset, and the torque handling rather than reimplementing them.
        self.set_mode(ModeSelection::Replay {
            robot: robot.to_owned(),
            episode: episode.to_owned(),
        })?;

        let start_position = start_frame as f64;
        self.replay = Some(Replay {
            episode: episode.to_owned(),
            robot: robot.to_owned(),
            frames,
            frame_period: 1.0 / sample_hz,
            purpose,
            phase: ReplayPhase::Seeking,
            position: start_position,
            start_position,
            resolved_start_seconds,
            paused: false,
            seeking_since: Instant::now(),
        });
        Ok(())
    }

    fn set_replay_paused(&mut self, paused: bool) -> Result<(), String> {
        // A physical seek is an atomic motion consequence, not Timeline
        // transport. A late pause from the replay that preceded a scrub must
        // not freeze the replacement seek before the arm can move.
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.purpose == ReplayPurpose::PhysicalSeek)
        {
            return Ok(());
        }
        let robot = self
            .replay
            .as_ref()
            .map(|replay| replay.robot.clone())
            .ok_or_else(|| "no episode is being replayed".to_owned())?;
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.paused == paused)
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
        if let Some(replay) = self.replay.as_mut() {
            replay.paused = paused;
            if !paused && replay.phase == ReplayPhase::Seeking {
                replay.seeking_since = Instant::now();
            }
        }
        Ok(())
    }

    fn connect(&mut self, spec: RobotSpec) -> Result<(), String> {
        if self.robots.contains_key(&spec.id) {
            return Err(format!("robot {:?} is already connected", spec.id));
        }
        if spec.motors.is_empty() {
            return Err(format!("robot {:?} has no motors configured", spec.id));
        }
        // Two robots cannot share one serial port. Simulated buses open no
        // device, so a placeholder path may legitimately be reused.
        if !self.settings.simulated {
            let holder = self
                .robots
                .values()
                .find(|robot| robot.spec.port.path == spec.port.path);
            if let Some(holder) = holder {
                return Err(format!(
                    "port {:?} is already open for robot {:?}",
                    spec.port.path, holder.spec.id
                ));
            }
        }

        let open = |spec: &RobotSpec| -> Result<Box<dyn MotorBus>, String> {
            if self.settings.simulated {
                Ok(Box::new(SimulatedBus::new(&spec.motors, true)))
            } else {
                FeetechBus::connect(&spec.port.path, spec.port.baud_rate, &spec.motors)
                    .map(|bus| Box::new(bus) as Box<dyn MotorBus>)
                    .map_err(|error| error.to_string())
            }
        };

        let mut bus = open(&spec).inspect_err(|error| {
            self.connections.insert(
                spec.id.clone(),
                ConnectionStatus {
                    connected: false,
                    error: Some(error.clone()),
                },
            );
        })?;

        // Reading once proves every configured motor is really on the bus
        // before the robot is presented as connected.
        let positions = bus.read_positions().map_err(|error| {
            let message = format!("could not reach all motors on {}: {error}", spec.port.path);
            self.connections.insert(
                spec.id.clone(),
                ConnectionStatus {
                    connected: false,
                    error: Some(message.clone()),
                },
            );
            message
        })?;

        // Off the control loop's path, and allowed to come back empty: an arm
        // that drives without known thresholds is far more useful than one
        // that refuses to open because a limit register would not answer.
        let limits = bus
            .read_limits()
            .into_iter()
            .map(|(name, limits)| (name, convert_limits(limits)))
            .collect();

        let id = spec.id.clone();
        self.robots.insert(
            id.clone(),
            ConnectedRobot {
                spec,
                bus,
                torque: false,
                target: positions.clone(),
                commanded: positions.clone(),
                tracks: BTreeMap::new(),
                limits,
                positions,
                error: None,
                failures: 0,
                sweep: None,
            },
        );
        self.connections.insert(
            id,
            ConnectionStatus {
                connected: true,
                error: None,
            },
        );
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

        robot
            .bus
            .set_torque(enabled)
            .map_err(|error| error.to_string())?;
        robot.torque = enabled;
        // Whichever way torque just moved, start from where the arm actually
        // is rather than from a stale target.
        robot.follow_measured_positions();
        robot.error = None;
        Ok(())
    }

    fn set_goals(&mut self, id: &str, normalized: &BTreeMap<String, f64>) -> Result<(), String> {
        if !matches!(&self.mode.selection, ModeSelection::Manual { robot } if robot == id) {
            return Err(format!(
                "switch to manual mode for robot {id:?} before commanding its joints"
            ));
        }

        self.set_normalized_goals(id, normalized)
    }

    fn set_policy_goals(
        &mut self,
        id: &str,
        session: &str,
        normalized: &BTreeMap<String, f64>,
    ) -> Result<(), String> {
        if !matches!(
            &self.mode.selection,
            ModeSelection::Policy { robot, session: active }
                if robot == id && active == session
        ) {
            return Err(format!(
                "policy session {session:?} is not allowed to command robot {id:?}"
            ));
        }
        self.set_normalized_goals(id, normalized)
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
        if !robot.torque {
            return Err(format!(
                "enable torque on robot {id:?} before commanding its joints"
            ));
        }
        let calibration =
            robot.spec.calibration.as_ref().ok_or_else(|| {
                format!("robot {id:?} must be calibrated before it can be commanded")
            })?;

        let mut targets = BTreeMap::new();
        for (name, value) in normalized {
            let motor = calibration
                .motors
                .get(name)
                .ok_or_else(|| format!("joint {name:?} has no calibration on robot {id:?}"))?;
            let raw = denormalize_centered_raw(motor, *value)
                .map_err(|error| format!("joint {name:?}: {error}"))?;
            targets.insert(name.clone(), raw);
        }
        robot.target.extend(targets);
        Ok(())
    }

    fn set_mode(&mut self, selection: ModeSelection) -> Result<(), String> {
        let sweeping = self
            .robots
            .values()
            .filter(|robot| robot.sweep.is_some())
            .map(|robot| robot.spec.id.clone())
            .collect::<Vec<_>>();
        if !sweeping.is_empty() && !matches!(selection, ModeSelection::Idle) {
            return Err(format!(
                "calibration is in progress for {}; finish or cancel it first",
                sweeping.join(", ")
            ));
        }

        match &selection {
            ModeSelection::Idle => {}
            ModeSelection::Manual { robot } => {
                self.require_calibrated(robot)?;
            }
            ModeSelection::Teleoperate { leader, follower } => {
                if leader == follower {
                    return Err("teleoperation needs two different robots".to_owned());
                }
                let leader_calibration = self.require_calibrated(leader)?;
                let follower_calibration = self.require_calibrated(follower)?;
                let shared = leader_calibration
                    .motors
                    .keys()
                    .filter(|name| follower_calibration.motors.contains_key(*name))
                    .count();
                if shared == 0 {
                    return Err(format!(
                        "robots {leader:?} and {follower:?} share no calibrated joints"
                    ));
                }
            }
            ModeSelection::Replay { robot, .. } => {
                self.require_calibrated(robot)?;
            }
            ModeSelection::Policy { robot, .. } => {
                self.require_calibrated(robot)?;
            }
        }

        // A mode change of any kind ends a replay: the trajectory belongs to
        // the mode, and leaving it must not leave samples queued to drive an
        // arm nothing is steering.
        if !matches!(selection, ModeSelection::Replay { .. }) {
            self.replay = None;
        }

        // Entering a mode must never inherit an old target.
        for robot in self.robots.values_mut() {
            robot.follow_measured_positions();
        }

        // Teleoperation only means anything with the follower holding and the
        // leader loose, so entering it puts the pair into that state instead of
        // leaving the operator to work out why nothing moved. This is the one
        // place torque is enabled without being asked for, and it goes through
        // the same path as the button: each goal is seeded from the measured
        // position first, so the follower holds where it stands rather than
        // snapping to wherever it was last sent.
        //
        // The order matters. Releasing the leader first means a failure part
        // way through leaves an arm loose rather than an arm driving toward
        // something with nobody in the mode that was supposed to steer it.
        if let ModeSelection::Teleoperate { leader, follower } = &selection {
            let (leader, follower) = (leader.clone(), follower.clone());
            self.set_torque(&leader, false)?;
            self.set_torque(&follower, true)?;
        }

        // Replay is the second — and only other — place torque is enabled
        // without a separate request for it, for the same reason and by the
        // same route: a replay that cannot move the arm is not a replay, and
        // the goal has just been seeded from the measured position above, so
        // the arm holds where it stands until the seek starts walking it to the
        // take's first frame.
        if let ModeSelection::Replay { robot, .. } = &selection {
            let robot = robot.clone();
            self.set_torque(&robot, true)?;
        }
        if let ModeSelection::Policy { robot, .. } = &selection {
            let robot = robot.clone();
            self.set_torque(&robot, true)?;
        }

        self.mode = ModeStatus {
            active: !matches!(selection, ModeSelection::Idle),
            selection,
            error: None,
        };
        Ok(())
    }

    fn require_calibrated(&self, id: &str) -> Result<&Calibration, String> {
        let robot = self
            .robots
            .get(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        robot
            .spec
            .calibration
            .as_ref()
            .ok_or_else(|| format!("robot {id:?} must be calibrated first"))
    }

    fn start_calibration(&mut self, id: &str) -> Result<(), String> {
        // A sweep is done by hand, so nothing may be driving anything. Say why
        // the mode stopped rather than letting it silently flip to idle.
        if !matches!(self.mode.selection, ModeSelection::Idle) {
            self.mode = ModeStatus {
                error: Some(format!(
                    "stopped {} to calibrate {id}",
                    self.mode.selection.label()
                )),
                selection: ModeSelection::Idle,
                active: false,
            };
        }

        let robot = self
            .robots
            .get_mut(id)
            .ok_or_else(|| format!("robot {id:?} is not connected"))?;
        robot
            .bus
            .set_torque(false)
            .map_err(|error| format!("could not release torque for calibration: {error}"))?;
        robot.torque = false;

        let positions = robot
            .bus
            .read_positions()
            .map_err(|error| error.to_string())?;
        robot.positions = positions.clone();
        robot.follow_measured_positions();
        robot.sweep = Some(RangeTracker::new(positions).map_err(|error| error.to_string())?);
        robot.error = None;
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
                robot.failures = 0;
                robot.error = None;
                let positions = samples
                    .iter()
                    .map(|(name, sample)| (name.clone(), sample.position))
                    .collect::<BTreeMap<_, _>>();
                if let Some(sweep) = robot.sweep.as_mut()
                    && let Err(error) = sweep.observe(&positions)
                {
                    robot.error = Some(error.to_string());
                    robot.sweep = None;
                }
                let now = Instant::now();
                for (name, sample) in samples {
                    robot.tracks.entry(name).or_default().observe(sample, now);
                }
                robot.positions = positions;
                if !robot.torque {
                    robot.follow_measured_positions();
                }
            }
            Err(error) => {
                robot.failures += 1;
                robot.error = Some(error.to_string());
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
        let quick = match &self.mode.selection {
            ModeSelection::Teleoperate { follower, .. } => Some(follower.as_str()),
            ModeSelection::Replay { robot, .. } => self
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
        if matches!(self.mode.selection, ModeSelection::Replay { .. }) {
            self.apply_replay();
            return;
        }
        let ModeSelection::Teleoperate { leader, follower } = self.mode.selection.clone() else {
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
                self.mode.error = Some("both arms must be calibrated".to_owned());
                self.mode.active = false;
                return;
            };

            leader_robot
                .positions
                .iter()
                .filter_map(|(name, raw)| {
                    let source = leader_calibration.motors.get(name)?;
                    let destination = follower_calibration.motors.get(name)?;
                    let normalized = normalize_centered_raw(source, *raw).ok()?;
                    let target = denormalize_centered_raw(destination, normalized).ok()?;
                    Some((name.clone(), target))
                })
                .collect::<BTreeMap<_, _>>()
        };

        if let Some(follower_robot) = self.robots.get_mut(&follower) {
            follower_robot.target.extend(targets);
        }
        self.mode.active = true;
        self.mode.error = None;
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
        let Some(replay) = self.replay.as_mut() else {
            self.mode.error = Some("the replay was lost".to_owned());
            self.mode.selection = ModeSelection::Idle;
            self.mode.active = false;
            return;
        };
        let (robot_id, episode) = (replay.robot.clone(), replay.episode.clone());

        if replay.paused {
            return;
        }

        let Some(robot) = self.robots.get(&robot_id) else {
            self.replay = None;
            self.leave_mode_involving(&robot_id, "the arm is no longer connected");
            return;
        };
        let Some(calibration) = robot.spec.calibration.clone() else {
            self.mode.error = Some(format!("robot {robot_id:?} must be calibrated"));
            self.mode.active = false;
            return;
        };

        if replay.phase == ReplayPhase::Seeking {
            let Some(first) = replay.pose() else {
                self.replay = None;
                self.mode = ModeStatus {
                    selection: ModeSelection::Idle,
                    active: false,
                    error: Some(format!("take {episode:?} has no frames to replay")),
                };
                return;
            };
            let arrived = first.iter().all(|(joint, wanted)| {
                let Some(motor) = calibration.motors.get(joint) else {
                    return true;
                };
                let Some(measured) = robot.positions.get(joint).copied() else {
                    return true;
                };
                normalize_centered_raw(motor, measured)
                    .map(|actual| (actual - wanted).abs() <= REPLAY_SEEK_TOLERANCE)
                    .unwrap_or(true)
            });

            if arrived {
                replay.phase = ReplayPhase::Playing;
            } else if replay.seeking_since.elapsed() >= REPLAY_SEEK_TIMEOUT {
                let waited = REPLAY_SEEK_TIMEOUT.as_secs();
                self.replay = None;
                self.mode = ModeStatus {
                    selection: ModeSelection::Idle,
                    active: false,
                    error: Some(format!(
                        "stopped Replay: {robot_id} did not reach the requested pose in {episode} within {waited} seconds. Check whether a joint is obstructed."
                    )),
                };
                return;
            }
        }

        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        if replay.phase == ReplayPhase::Playing {
            if replay.finished() {
                // Reaching the end is a completed replay, not a fault, so the
                // arm is left holding its final pose with no error attached.
                self.replay = None;
                self.mode = ModeStatus {
                    selection: ModeSelection::Idle,
                    active: false,
                    error: None,
                };
                return;
            }
            replay.position += self.settings.period.as_secs_f64() / replay.frame_period;
        }

        let Some(pose) = replay.pose().cloned() else {
            return;
        };
        let targets = pose
            .iter()
            .filter_map(|(joint, value)| {
                let motor = calibration.motors.get(joint)?;
                Some((joint.clone(), denormalize_centered_raw(motor, *value).ok()?))
            })
            .collect::<BTreeMap<_, _>>();
        if let Some(robot) = self.robots.get_mut(&robot_id) {
            robot.target.extend(targets);
        }
        self.mode.active = true;
        self.mode.error = None;
    }

    /// Fall back to idle when a robot the current mode depends on goes away.
    fn leave_mode_involving(&mut self, id: &str, reason: &str) {
        let involved = match &self.mode.selection {
            ModeSelection::Idle => false,
            ModeSelection::Manual { robot } => robot == id,
            ModeSelection::Teleoperate { leader, follower } => leader == id || follower == id,
            ModeSelection::Replay { robot, .. } => robot == id,
            ModeSelection::Policy { robot, .. } => robot == id,
        };
        if involved {
            self.replay = None;
            self.mode = ModeStatus {
                selection: ModeSelection::Idle,
                active: false,
                error: Some(format!("stopped {}: {reason}", self.mode.selection.label())),
            };
        }
    }

    fn drop_failed_robots(&mut self) {
        let lost = self
            .robots
            .iter()
            .filter(|(_, robot)| robot.failures >= MAX_CONSECUTIVE_FAILURES)
            .map(|(id, robot)| {
                (
                    id.clone(),
                    robot
                        .error
                        .clone()
                        .unwrap_or_else(|| "the motor bus stopped responding".to_owned()),
                )
            })
            .collect::<Vec<_>>();

        for (id, error) in lost {
            self.robots.remove(&id);
            self.connections.insert(
                id.clone(),
                ConnectionStatus {
                    connected: false,
                    error: Some(error),
                },
            );
            self.leave_mode_involving(&id, "its motor bus stopped responding");
        }
    }

    fn publish(&mut self) {
        self.sequence += 1;
        let state = HardwareState {
            telemetry: TelemetryFrame {
                sequence: self.sequence,
                loop_hz: self.loop_hz,
                target_hz: 1.0 / self.settings.period.as_secs_f64(),
                robots: self
                    .robots
                    .values()
                    .map(ConnectedRobot::telemetry)
                    .collect(),
                recording: self.recording.as_ref().map(|recording| {
                    let now = Instant::now();
                    RecordingProgress {
                        episode: recording.writer.id().to_owned(),
                        frames: recording.frames,
                        elapsed_seconds: recording.active_elapsed(now).as_secs_f64(),
                        wall_elapsed_seconds: now.duration_since(recording.started).as_secs_f64(),
                        paused_seconds: recording.paused_elapsed(now).as_secs_f64(),
                        paused: recording.paused_at.is_some(),
                        arms: recording.arms.iter().map(|arm| arm.id.clone()).collect(),
                        max_follow_error_degrees: recording.follow_error(),
                        error: recording.writer.failure(),
                    }
                }),
                replay: self.replay.as_ref().map(|replay| ReplayProgress {
                    episode: replay.episode.clone(),
                    robot: replay.robot.clone(),
                    // Seeking preserves the requested episode position while
                    // the arm ramps there, so visualizations never jump back
                    // to frame zero after a committed scrub.
                    frame: replay.cursor().min(replay.frames.len()) as u64,
                    frames: replay.frames.len() as u64,
                    elapsed_seconds: replay.elapsed_seconds(),
                    duration_seconds: replay.duration_seconds(),
                    purpose: replay.purpose,
                    phase: replay.phase,
                    paused: replay.paused,
                }),
            },
            connections: self.connections.clone(),
            mode: self.mode.clone(),
            limits: self
                .robots
                .iter()
                .map(|(id, robot)| (id.clone(), robot.limits.clone()))
                .collect(),
            recording_issue: self.recording_issue.clone(),
        };
        // `send_replace` rather than `send`: the latest state must be readable
        // through the handle even at moments when nothing is subscribed.
        let _ = self.state.send_replace(state);
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::calibration::{MotorReading, build_calibration};

    use super::*;

    fn motors() -> BTreeMap<String, MotorSpec> {
        arm::so101_motor_specs()
    }

    fn calibration(center: i64, span: i64) -> Calibration {
        let motors = motors();
        let input = CalibrationInput::Direct {
            motors: motors
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        MotorReading {
                            center,
                            range_min: center - span,
                            range_max: center + span,
                        },
                    )
                })
                .collect(),
        };
        build_calibration(&motors, &input).expect("calibration")
    }

    fn spec(id: &str, calibration: Option<Calibration>) -> RobotSpec {
        RobotSpec {
            id: id.to_owned(),
            port: PortSettings {
                path: format!("/dev/{id}"),
                baud_rate: 1_000_000,
            },
            motors: motors(),
            calibration,
        }
    }

    fn controller() -> Controller {
        let (state, _receiver) = watch::channel(HardwareState::default());
        Controller::new(
            ControlSettings {
                simulated: true,
                ..ControlSettings::default()
            },
            state,
        )
    }

    /// A take on disk, plus the temporary directory that holds it.
    fn take(
        controller: &mut Controller,
        arms: &[&str],
        frames: usize,
    ) -> (tempfile::TempDir, EpisodeSummary) {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = crate::backend::recording::episodes::EpisodeStore::open(
            directory.path().join("episodes"),
        )
        .expect("store");
        let layout = arms
            .iter()
            .map(|id| {
                let joints = controller.robots[*id]
                    .ordered_motors()
                    .into_iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                ((*id).to_owned(), joints)
            })
            .collect::<Vec<_>>();
        let layout = crate::backend::recording::episodes::build_arms(&layout).expect("arms");

        let writer = store
            .begin(
                "Teleoperation".to_owned(),
                None,
                60.0,
                layout.clone(),
                Vec::new(),
            )
            .expect("begin");
        controller
            .start_recording(writer, layout)
            .expect("start recording");
        for _ in 0..frames {
            controller.poll_robots();
            controller.capture_sample();
        }
        let summary = controller.stop_recording(true).expect("stop recording");
        store.reload().expect("reload");
        (directory, summary)
    }

    #[test]
    fn a_take_captures_one_frame_per_control_cycle() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");

        let (_directory, summary) = take(&mut controller, &["arm"], 5);

        assert_eq!(summary.frames, 5);
        assert_eq!(summary.arms.len(), 1);
        assert_eq!(summary.arms[0].id, "arm");
        assert_eq!(summary.arms[0].joints.len(), motors().len());
        assert!(summary.usable());
        assert!(controller.recording.is_none(), "the take is closed");
    }

    #[test]
    fn pause_keeps_one_take_flushes_history_and_restores_teleoperation() {
        let mut controller = controller();
        controller
            .connect(spec("leader", Some(calibration(2048, 500))))
            .expect("connect leader");
        controller
            .connect(spec("follower", Some(calibration(2048, 500))))
            .expect("connect follower");
        let teleoperation = ModeSelection::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        };
        controller
            .set_mode(teleoperation.clone())
            .expect("start teleoperation");

        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = crate::backend::recording::episodes::EpisodeStore::open(
            directory.path().join("episodes"),
        )
        .expect("store");
        let layout = crate::backend::recording::episodes::build_arms(&[
            ("leader".to_owned(), vec!["gripper".to_owned()]),
            ("follower".to_owned(), vec!["gripper".to_owned()]),
        ])
        .expect("arms");
        let writer = store
            .begin(
                "Teleoperation".to_owned(),
                None,
                60.0,
                layout.clone(),
                Vec::new(),
            )
            .expect("begin");
        let episode = writer.id().to_owned();
        controller
            .start_recording(writer, layout)
            .expect("start recording");
        for _ in 0..2 {
            controller.poll_robots();
            controller.capture_sample();
        }

        let (checkpoint, before_pause) = controller
            .set_recording_paused(true)
            .expect("pause recording");
        checkpoint
            .expect("pause has a writer barrier")
            .blocking_recv()
            .expect("writer still running")
            .expect("partial file flushed");
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
        assert!(controller.recording.as_ref().unwrap().paused_at.is_some());

        store.reload().expect("reload flushed partial");
        assert_eq!(
            store.timeline_metadata(&episode).unwrap().samples,
            2,
            "all pre-pause samples are visible before pause acknowledges"
        );
        std::thread::sleep(Duration::from_millis(15));
        controller.capture_sample();
        assert_eq!(controller.recording.as_ref().unwrap().frames, 2);

        let (_, resume_at) = controller
            .set_recording_paused(false)
            .expect("resume recording");
        assert_eq!(controller.mode.selection, teleoperation);
        assert!((resume_at - before_pause).abs() < 0.01);
        for _ in 0..2 {
            controller.poll_robots();
            controller.capture_sample();
        }
        let summary = controller.stop_recording(true).expect("finish same take");

        assert_eq!(summary.id, episode);
        assert_eq!(summary.frames, 4);
        assert_eq!(summary.recording_gaps.len(), 1);
        assert!(summary.recording_gaps[0].duration_seconds >= 0.01);
        assert!(
            summary.duration_seconds + summary.recording_gaps[0].duration_seconds
                >= Duration::from_millis(15).as_secs_f64()
        );
    }

    #[test]
    fn a_takes_headline_error_ignores_the_arm_getting_into_position() {
        let mut recording = Recording {
            condition: Vec::new(),
            writer: {
                let directory = tempfile::TempDir::new().expect("temporary directory");
                let store = crate::backend::recording::episodes::EpisodeStore::open(
                    directory.path().join("e"),
                )
                .expect("store");
                let arms = crate::backend::recording::episodes::build_arms(&[(
                    "a".to_owned(),
                    vec!["j".to_owned()],
                )])
                .unwrap();
                let writer = store
                    .begin("t".to_owned(), None, 60.0, arms, Vec::new())
                    .expect("begin");
                std::mem::forget(directory);
                writer
            },
            arms: Vec::new(),
            resume_mode: ModeSelection::Idle,
            started: Instant::now(),
            paused_at: None,
            paused_duration: Duration::ZERO,
            gaps: Vec::new(),
            frames: 0,
            max_follow_error: None,
            settled: false,
            unsettled_peak: None,
        };

        // The arm travels to the take's opening pose, catches up, then tracks
        // with one genuine wobble.
        for error in [43.4, 30.0, 12.0, 2.1, 0.4, 6.8, 1.0] {
            recording.observe(error);
        }
        assert!(recording.settled);
        assert_eq!(
            recording.follow_error(),
            Some(6.8),
            "the opening 43.4 degrees is the arm getting into position, not the take tracking badly"
        );

        // A take that never catches up has no settled period, and must still
        // report badly rather than reporting nothing.
        let mut never = Recording {
            settled: false,
            max_follow_error: None,
            unsettled_peak: None,
            ..recording
        };
        for error in [80.0, 120.0, 95.0] {
            never.observe(error);
        }
        assert!(!never.settled);
        assert_eq!(never.follow_error(), Some(120.0));
    }

    #[test]
    fn recording_a_second_take_while_one_runs_is_refused() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = crate::backend::recording::episodes::EpisodeStore::open(
            directory.path().join("episodes"),
        )
        .expect("store");
        let layout = crate::backend::recording::episodes::build_arms(&[(
            "arm".to_owned(),
            vec!["gripper".to_owned()],
        )])
        .expect("arms");

        let first = store
            .begin("Idle".to_owned(), None, 60.0, layout.clone(), Vec::new())
            .expect("begin");
        controller
            .start_recording(first, layout.clone())
            .expect("first take");

        let second = store
            .begin("Idle".to_owned(), None, 60.0, layout.clone(), Vec::new())
            .expect("begin");
        assert!(
            controller
                .start_recording(second, layout)
                .expect_err("second take")
                .contains("already recording")
        );

        // And stopping when nothing is running says so rather than inventing an
        // empty episode.
        controller.stop_recording(true).expect("stop the first");
        assert!(
            controller
                .stop_recording(true)
                .expect_err("nothing to stop")
                .contains("no take")
        );
    }

    #[test]
    fn losing_an_arm_mid_take_closes_it_and_says_so() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = crate::backend::recording::episodes::EpisodeStore::open(
            directory.path().join("episodes"),
        )
        .expect("store");
        let layout = crate::backend::recording::episodes::build_arms(&[(
            "arm".to_owned(),
            vec!["gripper".to_owned()],
        )])
        .expect("arms");
        let writer = store
            .begin("Idle".to_owned(), None, 60.0, layout.clone(), Vec::new())
            .expect("begin");
        controller.start_recording(writer, layout).expect("start");

        controller.poll_robots();
        controller.capture_sample();
        assert!(controller.recording.is_some());

        // The bus goes away underneath the recording.
        controller.robots.remove("arm");
        controller.capture_sample();

        assert!(controller.recording.is_none(), "the take stops itself");
        let issue = controller
            .recording_issue
            .as_deref()
            .expect("the operator is told why");
        assert!(issue.contains("no longer connected"), "{issue}");
        // The frames captured before the arm left are kept, not thrown away.
        assert!(issue.contains("1 frames"), "{issue}");
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
            .start_replay("arm", "episode_000001", frames, 60.0)
            .expect("start replay");

        assert_eq!(
            controller.replay.as_ref().map(|replay| replay.phase),
            Some(ReplayPhase::Seeking)
        );
        // Entering replay energizes the arm, the same way entering teleoperation
        // holds its follower.
        assert!(controller.robots["arm"].torque);

        // Seeking uses the gentle manual ramp, so the arm does not lunge at a
        // start pose the operator never saw coming.
        let mut cycles = 0;
        while controller
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
            controller.replay.as_ref().map(|replay| replay.phase),
            Some(ReplayPhase::Playing),
            "the replay starts playing once the arm arrives"
        );

        // Playing to the end leaves idle with no fault attached.
        for _ in 0..400 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
            if controller.replay.is_none() {
                break;
            }
        }
        assert!(controller.replay.is_none(), "the replay finished");
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
        assert_eq!(
            controller.mode.error, None,
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
            .start_replay("arm", "episode_000001", frames, 60.0)
            .expect("start replay");

        for _ in 0..400 {
            controller.poll_robots();
            controller.apply_mode();
            controller.drive_robots();
            if controller
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
            .replay
            .as_ref()
            .expect("replay is active")
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
                .replay
                .as_ref()
                .expect("paused replay remains loaded")
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
                .replay
                .as_ref()
                .expect("replay continues")
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
            .start_replay_for(
                "arm",
                "episode_000001",
                ReplayPlan {
                    frames,
                    sample_hz: 60.0,
                    start_frame: 60,
                    resolved_start_seconds: 1.25,
                    purpose: ReplayPurpose::Timeline,
                },
            )
            .expect("start replay at playhead");

        let replay = controller.replay.as_ref().expect("replay is active");
        assert_eq!(replay.position, 60.0);
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
            .start_replay_for(
                "arm",
                "episode_000001",
                ReplayPlan {
                    frames: vec![BTreeMap::from([("gripper".to_owned(), 0.25)])],
                    sample_hz: 60.0,
                    start_frame: 0,
                    resolved_start_seconds: 0.0,
                    purpose: ReplayPurpose::PhysicalSeek,
                },
            )
            .expect("start physical seek");

        controller
            .set_replay_paused(true)
            .expect("late pause is harmless");

        assert!(
            !controller
                .replay
                .as_ref()
                .expect("seek remains active")
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
            .start_replay(
                "arm",
                "episode_000001",
                vec![BTreeMap::from([("gripper".to_owned(), 0.25)])],
                60.0,
            )
            .expect("start timeline replay");

        controller.set_replay_paused(true).expect("pause replay");

        let replay = controller.replay.as_ref().expect("replay remains loaded");
        assert_eq!(replay.purpose, ReplayPurpose::Timeline);
        assert!(replay.paused);
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
                .start_replay("bare", "episode_000001", pose.clone(), 60.0)
                .expect_err("uncalibrated")
                .contains("calibrated")
        );
        assert!(
            controller
                .start_replay("arm", "episode_000001", Vec::new(), 60.0)
                .expect_err("no frames")
                .contains("no frames")
        );
        assert!(
            controller
                .start_replay("arm", "episode_000001", pose.clone(), 0.0)
                .expect_err("bad rate")
                .contains("sample rate")
        );
        assert!(
            controller
                .start_replay(
                    "arm",
                    "episode_000001",
                    vec![BTreeMap::from([("sixth_axis".to_owned(), 0.1)])],
                    60.0,
                )
                .expect_err("unknown joint")
                .contains("sixth_axis")
        );
        assert!(controller.replay.is_none());
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
    }

    #[test]
    fn leaving_replay_drops_the_trajectory() {
        let mut controller = controller();
        controller
            .connect(spec("arm", Some(calibration(2048, 500))))
            .expect("connect");
        controller.poll_robots();
        controller
            .start_replay(
                "arm",
                "episode_000001",
                vec![BTreeMap::from([("gripper".to_owned(), 0.1)])],
                60.0,
            )
            .expect("start replay");
        assert!(controller.replay.is_some());

        controller.set_mode(ModeSelection::Idle).expect("stop");
        assert!(
            controller.replay.is_none(),
            "an idle studio must not keep samples queued to drive an arm"
        );

        // And losing the arm does the same, with a reason.
        controller.poll_robots();
        controller
            .start_replay(
                "arm",
                "episode_000001",
                vec![BTreeMap::from([("gripper".to_owned(), 0.1)])],
                60.0,
            )
            .expect("start replay again");
        controller.leave_mode_involving("arm", "its motor bus stopped responding");
        assert!(controller.replay.is_none());
        assert!(
            controller
                .mode
                .error
                .as_deref()
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
        assert!(controller.connections["arm"].connected);
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
    fn commands_require_torque_and_stay_inside_the_calibrated_range() {
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
            .set_goals("arm", &BTreeMap::from([("gripper".to_owned(), 9.0)]))
            .expect("an out-of-range command is clamped, not rejected");

        assert_eq!(controller.robots["arm"].target["gripper"], 2548);
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
        assert!(controller.robots["arm"].torque);
        let robot = controller.robots.get_mut("arm").unwrap();
        robot.positions = robot
            .spec
            .motors
            .keys()
            .map(|name| (name.clone(), 2048))
            .collect();
        robot.follow_measured_positions();

        let goals = BTreeMap::from([("gripper".to_owned(), 0.5)]);
        assert!(
            controller
                .set_policy_goals("arm", "old-rollout", &goals)
                .expect_err("stale session")
                .contains("not allowed")
        );
        controller
            .set_policy_goals("arm", "rollout-1", &goals)
            .expect("active session may command");
        assert_eq!(controller.robots["arm"].target["gripper"], 2298);
        let before = controller.robots["arm"].commanded["gripper"];
        controller.drive_robots();
        let after = controller.robots["arm"].commanded["gripper"];
        assert_ne!(
            after, before,
            "the admitted policy target reaches the motor ramp"
        );
        assert!(
            after.abs_diff(before) <= controller.settings.teleop_max_step_counts.unsigned_abs(),
            "policy motion stays inside the fast control-path step bound"
        );

        controller
            .set_mode(ModeSelection::Idle)
            .expect("stop policy");
        assert!(
            controller
                .set_policy_goals("arm", "rollout-1", &goals)
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
        robot.positions = robot
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
        let after_one_step = controller.robots["arm"].commanded["gripper"];

        assert_eq!(after_one_step - 2048, max_step, "one bounded step");
        for _ in 0..200 {
            controller.robots.get_mut("arm").unwrap().drive(max_step);
        }
        assert_eq!(controller.robots["arm"].commanded["gripper"], 2548);
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
        robot.positions.insert("gripper".to_owned(), 2048);
        robot.commanded.insert("gripper".to_owned(), 2048);
        robot.target.insert("gripper".to_owned(), 3048);
        controller.drive_robots();

        assert_eq!(
            controller.robots["follower"].commanded["gripper"] - 2048,
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

        // Torque is off, so the simulated arm is back-driven; polling must keep
        // the target following it so energizing later cannot cause a jump.
        for _ in 0..20 {
            controller.poll_robots();
        }
        let robot = &controller.robots["arm"];
        assert_eq!(robot.target, robot.positions);
        assert_eq!(robot.commanded, robot.positions);
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
        leader.positions.insert("gripper".to_owned(), 2448);
        controller.apply_mode();

        assert!(controller.mode.active);
        assert_eq!(
            controller.robots["follower"].target["gripper"], 2300,
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
            !controller.robots["leader"].torque,
            "the leader has to be free to move by hand"
        );
        assert!(
            controller.robots["follower"].torque,
            "the follower has to hold, or the mode does nothing"
        );
        // Holding means holding where it stands, not where it was last sent.
        let follower = &controller.robots["follower"];
        assert_eq!(follower.target, follower.positions);

        // Leaving the mode does not release the follower: torque is what keeps
        // an arm from falling, so dropping it is always the operator's call.
        controller.set_mode(ModeSelection::Idle).expect("idle");
        assert!(controller.robots["follower"].torque);
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
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
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

        controller.robots.get_mut("leader").unwrap().failures = MAX_CONSECUTIVE_FAILURES;
        controller.robots.get_mut("leader").unwrap().error = Some("bus went away".to_owned());
        controller.drop_failed_robots();

        assert!(!controller.robots.contains_key("leader"));
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
        assert!(!controller.mode.active);
        assert!(controller.mode.error.unwrap().contains("stopped"));
        let connection = &controller.connections["leader"];
        assert!(!connection.connected);
        assert_eq!(connection.error.as_deref(), Some("bus went away"));
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

        assert!(!controller.robots["arm"].torque);
        assert_eq!(controller.mode.selection, ModeSelection::Idle);
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

        // The simulated arm drifts through its travel while torque is off.
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
                .all(|reading| reading.range_max > reading.range_min)
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
            robot.positions = robot
                .spec
                .motors
                .keys()
                .map(|name| (name.clone(), 2048))
                .collect();
        }
        controller.publish();

        let state = controller.state.borrow().clone();
        let calibrated = state
            .telemetry
            .robots
            .iter()
            .find(|robot| robot.id == "calibrated")
            .expect("calibrated telemetry");
        let raw = state
            .telemetry
            .robots
            .iter()
            .find(|robot| robot.id == "raw")
            .expect("raw telemetry");

        assert_eq!(calibrated.joints.len(), 6);
        assert_eq!(calibrated.joints[0].degrees, Some(0.0));
        assert_eq!(calibrated.joints[0].normalized, Some(0.0));
        assert_eq!(
            calibrated.joints[0].range,
            Some(JointRange {
                min: 1024,
                center: 2048,
                max: 3072
            })
        );
        assert!(raw.joints.iter().all(|joint| joint.degrees.is_none()));
        assert!(raw.joints.iter().all(|joint| joint.goal_raw.is_none()));
        assert!(raw.joints.iter().all(|joint| joint.target_raw.is_none()));
        assert!(
            raw.joints
                .iter()
                .all(|joint| joint.target_degrees.is_none())
        );
    }

    #[test]
    fn an_uncalibrated_arm_still_reports_what_its_motors_say_about_themselves() {
        let mut controller = controller();
        controller.connect(spec("raw", None)).expect("connect");
        controller.poll_robots();
        controller.publish();

        let state = controller.state.borrow().clone();
        let raw = &state.telemetry.robots[0];

        // Calibration is what turns counts into degrees; it has nothing to do
        // with the motor's own condition, and an arm nobody has calibrated yet
        // is exactly the arm whose wiring you want to check.
        assert!(raw.joints.iter().all(|joint| joint.degrees.is_none()));
        assert!(
            raw.joints
                .iter()
                .all(|joint| joint.temperature_celsius.is_some()),
            "temperature does not wait for a calibration"
        );
        assert!(raw.joints.iter().all(|joint| joint.voltage_volts.is_some()));
        assert!(raw.joints.iter().all(|joint| joint.faults.is_empty()));

        let limits = &state.limits["raw"];
        assert_eq!(limits.len(), 6);
        assert!(
            limits
                .values()
                .all(|limits| limits.max_temperature_celsius.is_some()),
            "limits are read from the motors when the robot connects"
        );
    }

    #[test]
    fn a_transient_load_spike_survives_to_the_next_published_frame() {
        let mut track = MotorTrack::default();
        let now = Instant::now();
        let idle = MotorSample {
            load: Some(0.05),
            current_amps: Some(0.1),
            ..MotorSample::default()
        };
        for _ in 0..40 {
            track.observe(idle, now);
        }
        let settled = track.load.expect("a settled load");

        track.observe(
            MotorSample {
                load: Some(0.9),
                current_amps: Some(1.3),
                ..MotorSample::default()
            },
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
