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

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rerobot::bus::{MotorBus, SimulatedBus};
use rerobot::feetech::FeetechBus;
use rerobot_core::api::{
    CalibrationJointProgress, CalibrationProgress, ConnectionStatus, JointRange, JointTelemetry,
    ModeSelection, ModeStatus, PortSettings, RobotTelemetry, TelemetryFrame,
};
use rerobot_core::arm;
use rerobot_core::calibration::{
    Calibration, CalibrationError, CalibrationInput, Direction, MotorCalibration, MotorSpec,
    RangeTracker, denormalize_centered_raw, normalize_centered_raw,
};
use tokio::sync::{oneshot, watch};

/// How often the control loop runs.
pub const CONTROL_PERIOD: Duration = Duration::from_millis(20);

/// Default ceiling on how far one joint may be commanded to move per cycle.
///
/// At the control rate above this is roughly 100 degrees per second, which is
/// brisk enough for teleoperation and slow enough to stay in front of.
pub const DEFAULT_MAX_STEP_COUNTS: i64 = 24;

/// Consecutive failed transfers before a bus is considered lost.
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

type Reply<T> = oneshot::Sender<Result<T, String>>;

/// Everything the control thread needs to open one arm.
#[derive(Clone, Debug)]
pub struct RobotSpec {
    pub id: String,
    pub port: PortSettings,
    pub motors: BTreeMap<String, MotorSpec>,
    pub calibration: Option<Calibration>,
}

/// Tuning for the control loop.
#[derive(Clone, Copy, Debug)]
pub struct ControlSettings {
    pub period: Duration,
    /// Largest change in encoder counts applied to a joint in one cycle.
    pub max_step_counts: i64,
    /// Drive simulated motors instead of opening serial ports.
    pub simulated: bool,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            period: CONTROL_PERIOD,
            max_step_counts: DEFAULT_MAX_STEP_COUNTS,
            simulated: false,
        }
    }
}

/// A published sample of everything the control thread knows.
#[derive(Clone, Debug, Default)]
pub struct HardwareState {
    pub telemetry: TelemetryFrame,
    /// Every robot the thread has been asked to connect, including ones whose
    /// bus has since been lost, so the reason stays visible.
    pub connections: BTreeMap<String, ConnectionStatus>,
    pub mode: ModeStatus,
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
    Shutdown {
        reply: Reply<()>,
    },
}

/// The asynchronous side of the control thread.
#[derive(Debug)]
pub struct HardwareHandle {
    commands: Sender<Command>,
    state: watch::Receiver<HardwareState>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl HardwareHandle {
    /// Start the control thread.
    pub fn spawn(settings: ControlSettings) -> Self {
        let (commands, requests) = mpsc::channel();
        let (state, receiver) = watch::channel(HardwareState::default());
        let thread = thread::Builder::new()
            .name("rerobot-control".to_owned())
            .spawn(move || Controller::new(settings, state).run(&requests))
            .expect("the control thread must start");

        Self {
            commands,
            state: receiver,
            thread: Mutex::new(Some(thread)),
        }
    }

    /// Watch the control thread's published state.
    pub fn subscribe(&self) -> watch::Receiver<HardwareState> {
        self.state.clone()
    }

    /// Read the control thread's most recent state.
    pub fn state(&self) -> HardwareState {
        self.state.borrow().clone()
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

    /// Switch a robot's holding torque.
    pub async fn set_torque(&self, id: &str, enabled: bool) -> Result<(), String> {
        self.request(|reply| Command::SetTorque {
            id: id.to_owned(),
            enabled,
            reply,
        })
        .await
    }

    /// Command joint targets in normalized units.
    pub async fn set_goals(
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

    /// Adopt a calibration that has just been written to disk.
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
    pub async fn shutdown(&self) {
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
struct ConnectedRobot {
    spec: RobotSpec,
    bus: Box<dyn MotorBus>,
    torque: bool,
    positions: BTreeMap<String, i64>,
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

    fn telemetry(&self) -> RobotTelemetry {
        let calibration = self.spec.calibration.as_ref();
        let joints = self
            .ordered_motors()
            .into_iter()
            .map(|(name, spec)| {
                let raw = self.positions.get(name).copied().unwrap_or_default();
                let motor = calibration.and_then(|calibration| calibration.motors.get(name));
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
                    range: motor.map(|motor| JointRange {
                        min: motor.range_min,
                        center: motor.center,
                        max: motor.range_max,
                    }),
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
        }
    }

    fn run(mut self, requests: &mpsc::Receiver<Command>) {
        let mut previous_tick: Option<Instant> = None;
        loop {
            let started = Instant::now();
            if let Some(previous) = previous_tick.replace(started) {
                let elapsed = started.duration_since(previous).as_secs_f64();
                if elapsed > 0.0 {
                    // Smooth the measured rate so the readout is legible.
                    self.loop_hz = 0.9 * self.loop_hz + 0.1 / elapsed;
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

            self.poll_robots();
            self.apply_mode();
            let max_step = self.settings.max_step_counts;
            for robot in self.robots.values_mut() {
                robot.drive(max_step);
            }
            self.drop_failed_robots();
            self.publish();

            if let Some(remaining) = self.settings.period.checked_sub(started.elapsed()) {
                thread::sleep(remaining);
            }
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
            Command::Shutdown { reply } => {
                let _ = reply.send(Ok(()));
            }
        }
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

        let id = spec.id.clone();
        self.robots.insert(
            id.clone(),
            ConnectedRobot {
                spec,
                bus,
                torque: false,
                target: positions.clone(),
                commanded: positions.clone(),
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
        }

        // Entering a mode must never inherit an old target.
        for robot in self.robots.values_mut() {
            robot.follow_measured_positions();
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
            match robot.bus.read_positions() {
                Ok(positions) => {
                    robot.failures = 0;
                    robot.error = None;
                    if let Some(sweep) = robot.sweep.as_mut()
                        && let Err(error) = sweep.observe(&positions)
                    {
                        robot.error = Some(error.to_string());
                        robot.sweep = None;
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
    }

    fn apply_mode(&mut self) {
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

    /// Fall back to idle when a robot the current mode depends on goes away.
    fn leave_mode_involving(&mut self, id: &str, reason: &str) {
        let involved = match &self.mode.selection {
            ModeSelection::Idle => false,
            ModeSelection::Manual { robot } => robot == id,
            ModeSelection::Teleoperate { leader, follower } => leader == id || follower == id,
        };
        if involved {
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
                robots: self
                    .robots
                    .values()
                    .map(ConnectedRobot::telemetry)
                    .collect(),
            },
            connections: self.connections.clone(),
            mode: self.mode.clone(),
        };
        // `send_replace` rather than `send`: the latest state must be readable
        // through the handle even at moments when nothing is subscribed.
        let _ = self.state.send_replace(state);
    }
}

#[cfg(test)]
mod tests {
    use rerobot_core::calibration::{MotorReading, build_calibration};

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
        controller.set_torque("follower", true).expect("torque on");

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
    }
}
