//! Robot configuration, operation, calibration, and telemetry contracts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::recording::RecordingProgress;

pub mod visualization;

/// A saved robot as the Studio sees it: its configuration and live connection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotSummary {
    config: config::RobotConfig,
    /// Storage revision the configuration was read at; a save names it to detect conflicts.
    revision: u64,
    connection: ConnectionStatus,
    /// What each motor will fault at, read from its EEPROM when it connected.
    ///
    /// Lives on the snapshot rather than the telemetry frame because it does
    /// not change while an arm is running; resending it twenty times a second
    /// would be resending a constant.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    motor_limits: BTreeMap<String, MotorLimits>,
}

impl RobotSummary {
    pub fn new(config: config::RobotConfig, revision: u64, connection: ConnectionStatus) -> Self {
        Self {
            config,
            revision,
            connection,
            motor_limits: BTreeMap::new(),
        }
    }

    pub fn with_motor_limits(mut self, motor_limits: BTreeMap<String, MotorLimits>) -> Self {
        self.motor_limits = motor_limits;
        self
    }

    pub fn id(&self) -> &str {
        self.config.id()
    }

    pub const fn config(&self) -> &config::RobotConfig {
        &self.config
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn connection(&self) -> &ConnectionStatus {
        &self.connection
    }

    /// The limits `motor` reported when it connected, or none while disconnected.
    pub fn motor_limits(&self, motor: &str) -> MotorLimits {
        self.motor_limits.get(motor).copied().unwrap_or_default()
    }
}

/// The limits a motor enforces on itself.
///
/// These are read from the hardware rather than written down here. A motor
/// already knows the temperature and voltage it will fault at, and a threshold
/// copied out of a datasheet is a threshold that can disagree with the motor in
/// front of the operator.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MotorLimits {
    max_temperature_celsius: Option<f64>,
    min_voltage_volts: Option<f64>,
    max_voltage_volts: Option<f64>,
    /// Configured torque ceiling as a fraction of the motor's rated torque.
    max_load: Option<f64>,
}

impl MotorLimits {
    pub const fn with_max_temperature_celsius(
        mut self,
        max_temperature_celsius: Option<f64>,
    ) -> Self {
        self.max_temperature_celsius = max_temperature_celsius;
        self
    }

    pub const fn with_voltage_range(
        mut self,
        min_voltage_volts: Option<f64>,
        max_voltage_volts: Option<f64>,
    ) -> Self {
        self.min_voltage_volts = min_voltage_volts;
        self.max_voltage_volts = max_voltage_volts;
        self
    }

    pub const fn with_max_load(mut self, max_load: Option<f64>) -> Self {
        self.max_load = max_load;
        self
    }

    pub const fn max_temperature_celsius(self) -> Option<f64> {
        self.max_temperature_celsius
    }

    pub const fn min_voltage_volts(self) -> Option<f64> {
        self.min_voltage_volts
    }

    pub const fn max_voltage_volts(self) -> Option<f64> {
        self.max_voltage_volts
    }

    pub const fn max_load(self) -> Option<f64> {
        self.max_load
    }
}

/// Serial settings for a robot's motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortSettings {
    path: String,
    baud_rate: u32,
}

impl PortSettings {
    pub fn new(path: impl Into<String>, baud_rate: u32) -> Self {
        Self {
            path: path.into(),
            baud_rate,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.path.trim().is_empty() || self.path.contains('\0') {
            return Err("Serial port path must be nonempty without NUL bytes".into());
        }
        if self.baud_rate == 0 {
            return Err("Serial port baud rate must be greater than zero".into());
        }
        Ok(())
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn baud_rate(&self) -> u32 {
        self.baud_rate
    }
}

/// Where a camera sits and how much of the bench it can see.
///
/// Optional, and stays optional. A camera with no mount is a camera whose
/// position nobody has told the studio, and the honest thing to do with an
/// unknown position is not to draw one — a frustum at the origin would be a
/// claim about the hardware rather than a picture of it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraMount {
    /// Link in the robot description the camera rides, or `None` for one fixed
    /// to the bench. A wrist camera moves with the wrist; a top camera does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frame: Option<String>,
    /// Offset from that frame, in metres.
    #[serde(default)]
    xyz: [f64; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    #[serde(default)]
    rpy: [f64; 3],
    /// Horizontal field of view, in degrees.
    #[serde(default = "CameraMount::default_field_of_view")]
    fov_degrees: f64,
}

impl CameraMount {
    /// Field of view assumed for a mount that does not state one.
    const fn default_field_of_view() -> f64 {
        62.0
    }

    pub fn frame(&self) -> Option<&str> {
        self.frame.as_deref()
    }
    pub const fn fov_degrees(&self) -> f64 {
        self.fov_degrees
    }
    pub const fn xyz(&self) -> [f64; 3] {
        self.xyz
    }
    pub const fn rpy(&self) -> [f64; 3] {
        self.rpy
    }
}

/// Whether a robot's motor bus is currently open.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionStatus {
    connected: bool,
    /// Populated when the last connect or transfer attempt failed.
    error: Option<String>,
}

impl ConnectionStatus {
    pub fn new(connected: bool, error: Option<String>) -> Self {
        Self { connected, error }
    }

    pub const fn connected(&self) -> bool {
        self.connected
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// A camera the server knows about.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraStatus {
    /// Stable device identifier, also used in stream URLs.
    id: String,
    name: String,
    #[serde(flatten)]
    format: CameraStreamFormat,
    #[serde(flatten)]
    state: CameraStreamState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraStreamFormat {
    width: u32,
    height: u32,
    fps: u32,
}

impl CameraStreamFormat {
    pub const fn new(width: u32, height: u32, fps: u32) -> Self {
        Self { width, height, fps }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraStreamState {
    streaming: bool,
    frames: u64,
    error: Option<String>,
}

impl CameraStreamState {
    pub fn new(streaming: bool, frames: u64, error: Option<String>) -> Self {
        Self {
            streaming,
            frames,
            error,
        }
    }
}

impl CameraStatus {
    pub fn new(id: impl Into<String>, name: impl Into<String>, format: CameraStreamFormat) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            format,
            state: CameraStreamState::default(),
        }
    }

    pub fn with_state(mut self, state: CameraStreamState) -> Self {
        self.state = state;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn width(&self) -> u32 {
        self.format.width
    }
    pub const fn height(&self) -> u32 {
        self.format.height
    }
    pub const fn fps(&self) -> u32 {
        self.format.fps
    }
    pub const fn streaming(&self) -> bool {
        self.state.streaming
    }
    pub const fn frames(&self) -> u64 {
        self.state.frames
    }
    pub fn error(&self) -> Option<&str> {
        self.state.error.as_deref()
    }
}

/// A serial port offered as a candidate motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SerialPortSummary {
    path: String,
    description: Option<String>,
    /// ID of the robot currently holding this port open, if any.
    used_by: Option<String>,
}

impl SerialPortSummary {
    pub fn new(path: impl Into<String>, description: Option<String>) -> Self {
        Self {
            path: path.into(),
            description,
            used_by: None,
        }
    }
    pub fn with_owner(mut self, used_by: Option<String>) -> Self {
        self.used_by = used_by;
        self
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    pub fn used_by(&self) -> Option<&str> {
        self.used_by.as_deref()
    }
}

/// The operating mode the studio is in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModeSelection {
    /// Motors are read but never commanded.
    #[default]
    Idle,
    /// The UI commands one robot's joints directly.
    Manual { robot: String },
    /// A leader arm's measured pose is mirrored onto a follower arm.
    Teleoperate { leader: String, follower: String },
    /// A backend-owned remote policy session commands one calibrated arm.
    Policy { robot: String, session: String },
}

/// The selected mode plus whatever went wrong entering or running it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModeStatus {
    selection: ModeSelection,
    /// True once the mode is actually driving motors.
    active: bool,
    error: Option<String>,
}

impl ModeStatus {
    pub fn new(selection: ModeSelection, active: bool, error: Option<String>) -> Self {
        Self {
            selection,
            active,
            error,
        }
    }

    pub const fn selection(&self) -> &ModeSelection {
        &self.selection
    }

    pub const fn active(&self) -> bool {
        self.active
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn update_activity(&mut self, active: bool, error: Option<String>) {
        self.active = active;
        self.error = error;
    }
}

/// A live sample of every connected robot.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TelemetryFrame {
    sequence: u64,
    /// Measured rate of the hardware control loop.
    loop_hz: f64,
    /// Requested rate of the hardware control loop.
    ///
    /// Keeping the target beside the measurement lets operators distinguish a
    /// deliberately slow configuration from a loop that cannot keep up with
    /// its motor buses.
    target_hz: f64,
    robots: Vec<RobotTelemetry>,
    /// Present only while a take is being captured.
    recording: Option<RecordingProgress>,
}

impl TelemetryFrame {
    pub fn new(sequence: u64, loop_hz: f64, target_hz: f64) -> Self {
        Self {
            sequence,
            loop_hz,
            target_hz,
            robots: Vec::new(),
            recording: None,
        }
    }
    pub fn with_robots(mut self, robots: Vec<RobotTelemetry>) -> Self {
        self.robots = robots;
        self
    }
    pub fn with_recording(mut self, recording: Option<RecordingProgress>) -> Self {
        self.recording = recording;
        self
    }
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    pub const fn loop_hz(&self) -> f64 {
        self.loop_hz
    }
    pub const fn target_hz(&self) -> f64 {
        self.target_hz
    }
    pub fn robots(&self) -> &[RobotTelemetry] {
        &self.robots
    }
    pub const fn recording(&self) -> Option<&RecordingProgress> {
        self.recording.as_ref()
    }
}

/// One robot's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotTelemetry {
    id: String,
    torque: bool,
    /// Hardware-owned verdict for excessive measured-versus-commanded error.
    tracking_lagging: bool,
    joints: Vec<JointTelemetry>,
    error: Option<String>,
    /// Present only while a calibration sweep is running for this robot.
    calibration: Option<CalibrationProgress>,
}

impl RobotTelemetry {
    pub fn new(id: impl Into<String>, torque: bool, joints: Vec<JointTelemetry>) -> Self {
        Self {
            id: id.into(),
            torque,
            tracking_lagging: false,
            joints,
            error: None,
            calibration: None,
        }
    }

    pub fn with_error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    pub const fn with_tracking_lagging(mut self, tracking_lagging: bool) -> Self {
        self.tracking_lagging = tracking_lagging;
        self
    }

    pub fn with_calibration(mut self, calibration: Option<CalibrationProgress>) -> Self {
        self.calibration = calibration;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn torque(&self) -> bool {
        self.torque
    }

    pub const fn tracking_lagging(&self) -> bool {
        self.tracking_lagging
    }

    pub fn joints(&self) -> &[JointTelemetry] {
        &self.joints
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub const fn calibration(&self) -> Option<&CalibrationProgress> {
        self.calibration.as_ref()
    }
}

/// One joint's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JointTelemetry {
    #[serde(flatten)]
    identity: JointIdentity,
    #[serde(flatten)]
    kinematics: JointKinematics,
    #[serde(flatten)]
    diagnostics: JointDiagnostics,
    /// Hardware-owned assessment against this motor's reported limits.
    condition: MotorCondition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JointIdentity {
    name: String,
    id: u32,
}

impl JointIdentity {
    pub fn new(name: impl Into<String>, id: u32) -> Self {
        Self {
            name: name.into(),
            id,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JointKinematics {
    raw: i64,
    degrees: Option<f64>,
    normalized: Option<f64>,
    target_degrees: Option<f64>,
    range: Option<config::MotorRange>,
}

impl JointKinematics {
    pub const fn new(raw: i64, degrees: Option<f64>, normalized: Option<f64>) -> Self {
        Self {
            raw,
            degrees,
            normalized,
            target_degrees: None,
            range: None,
        }
    }
    pub const fn with_calibrated_target(
        mut self,
        target_degrees: Option<f64>,
        range: Option<config::MotorRange>,
    ) -> Self {
        self.target_degrees = target_degrees;
        self.range = range;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MotorElectricalReadings {
    temperature_celsius: Option<f64>,
    voltage_volts: Option<f64>,
    current_amps: Option<f64>,
}

impl MotorElectricalReadings {
    pub const fn new(
        temperature_celsius: Option<f64>,
        voltage_volts: Option<f64>,
        current_amps: Option<f64>,
    ) -> Self {
        Self {
            temperature_celsius,
            voltage_volts,
            current_amps,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JointDiagnostics {
    #[serde(flatten)]
    electrical: MotorElectricalReadings,
    load: Option<f64>,
    speed_degrees_per_second: Option<f64>,
    temperature_rise_per_minute: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    faults: Vec<MotorFault>,
}

impl JointDiagnostics {
    pub fn new(
        electrical: MotorElectricalReadings,
        load: Option<f64>,
        speed_degrees_per_second: Option<f64>,
    ) -> Self {
        Self {
            electrical,
            load,
            speed_degrees_per_second,
            temperature_rise_per_minute: None,
            faults: Vec::new(),
        }
    }
    pub fn with_temperature_rise(mut self, temperature_rise_per_minute: Option<f64>) -> Self {
        self.temperature_rise_per_minute = temperature_rise_per_minute;
        self
    }
    pub fn with_faults(mut self, faults: Vec<MotorFault>) -> Self {
        self.faults = faults;
        self
    }

    pub fn assess(&self, limits: MotorLimits) -> MotorCondition {
        const CAUTION_LIMIT_FRACTION: f64 = 0.85;
        const CAUTION_VOLTAGE_MARGIN_VOLTS: f64 = 0.3;

        let readings = [
            self.electrical.temperature_celsius,
            self.electrical.voltage_volts,
            self.load,
        ];
        let declared = [
            limits.max_temperature_celsius(),
            limits.min_voltage_volts(),
            limits.max_voltage_volts(),
            limits.max_load(),
        ];
        let invalid = readings
            .into_iter()
            .chain(declared)
            .flatten()
            .any(|value| !value.is_finite())
            || limits
                .max_temperature_celsius()
                .is_some_and(|value| value <= 0.0)
            || limits.min_voltage_volts().is_some_and(|value| value <= 0.0)
            || limits.max_voltage_volts().is_some_and(|value| value <= 0.0)
            || limits.max_load().is_some_and(|value| value <= 0.0)
            || matches!(
                (limits.min_voltage_volts(), limits.max_voltage_volts()),
                (Some(minimum), Some(maximum)) if minimum >= maximum
            );
        if !self.faults.is_empty() || invalid {
            return MotorCondition::Fault;
        }
        let hot = self
            .electrical
            .temperature_celsius
            .zip(limits.max_temperature_celsius())
            .is_some_and(|(value, limit)| value >= limit * CAUTION_LIMIT_FRACTION);
        let strained = self
            .load
            .zip(limits.max_load())
            .is_some_and(|(value, limit)| value.abs() >= limit * CAUTION_LIMIT_FRACTION);
        let supply = self.electrical.voltage_volts.is_some_and(|volts| {
            limits
                .min_voltage_volts()
                .is_some_and(|minimum| volts <= minimum + CAUTION_VOLTAGE_MARGIN_VOLTS)
                || limits
                    .max_voltage_volts()
                    .is_some_and(|maximum| volts >= maximum - CAUTION_VOLTAGE_MARGIN_VOLTS)
        });
        if hot || strained || supply {
            MotorCondition::Caution
        } else {
            MotorCondition::Nominal
        }
    }
}

impl JointTelemetry {
    pub fn new(
        identity: JointIdentity,
        kinematics: JointKinematics,
        diagnostics: JointDiagnostics,
    ) -> Self {
        Self {
            identity,
            kinematics,
            diagnostics,
            condition: MotorCondition::Nominal,
        }
    }
    pub const fn with_condition(mut self, condition: MotorCondition) -> Self {
        self.condition = condition;
        self
    }
    pub fn name(&self) -> &str {
        &self.identity.name
    }
    pub const fn id(&self) -> u32 {
        self.identity.id
    }
    pub const fn raw(&self) -> i64 {
        self.kinematics.raw
    }
    pub const fn degrees(&self) -> Option<f64> {
        self.kinematics.degrees
    }
    pub const fn normalized(&self) -> Option<f64> {
        self.kinematics.normalized
    }
    pub const fn target_degrees(&self) -> Option<f64> {
        self.kinematics.target_degrees
    }
    pub const fn range(&self) -> Option<config::MotorRange> {
        self.kinematics.range
    }
    pub const fn temperature_celsius(&self) -> Option<f64> {
        self.diagnostics.electrical.temperature_celsius
    }
    pub const fn voltage_volts(&self) -> Option<f64> {
        self.diagnostics.electrical.voltage_volts
    }
    pub const fn current_amps(&self) -> Option<f64> {
        self.diagnostics.electrical.current_amps
    }
    pub const fn load(&self) -> Option<f64> {
        self.diagnostics.load
    }
    pub const fn speed_degrees_per_second(&self) -> Option<f64> {
        self.diagnostics.speed_degrees_per_second
    }
    pub const fn temperature_rise_per_minute(&self) -> Option<f64> {
        self.diagnostics.temperature_rise_per_minute
    }
    pub fn faults(&self) -> &[MotorFault] {
        &self.diagnostics.faults
    }
}

/// A fault a motor reports about itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorFault {
    Overheat,
    Overload,
    Voltage,
    /// A fault bit the motor set that this build does not have a name for.
    ///
    /// Carried rather than dropped: an operator who can see that something is
    /// wrong can act on it, and a fault silently swallowed because it was
    /// unfamiliar is the one that ends a session.
    Unrecognized,
}

impl MotorFault {
    /// How the studio says this on the page.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overheat => "overheating",
            Self::Overload => "overloaded",
            Self::Voltage => "supply voltage",
            Self::Unrecognized => "motor fault",
        }
    }
}

/// How a motor is doing against its own limits.
///
/// Ordered worst-last so an arm's condition is the maximum of its motors'.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorCondition {
    #[default]
    Nominal,
    Caution,
    Fault,
}

impl JointTelemetry {
    pub const fn condition(&self) -> MotorCondition {
        self.condition
    }
}

/// Live progress of a calibration sweep.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CalibrationProgress {
    joints: Vec<CalibrationJointProgress>,
    /// True once every joint has moved far enough to record a range.
    ready: bool,
}

impl CalibrationProgress {
    pub fn new(joints: Vec<CalibrationJointProgress>, ready: bool) -> Self {
        Self { joints, ready }
    }
    pub fn joints(&self) -> &[CalibrationJointProgress] {
        &self.joints
    }
    pub const fn ready(&self) -> bool {
        self.ready
    }
}

/// One joint's progress through its calibration sweep.
///
/// Carries its own name rather than relying on position, so the UI never has
/// to assume two independently built lists are in the same order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationJointProgress {
    name: String,
    min: i64,
    position: i64,
    max: i64,
    encoder_counts_per_turn: i64,
    ready: bool,
}

impl CalibrationJointProgress {
    pub fn new(name: impl Into<String>, min: i64, position: i64) -> Self {
        Self {
            name: name.into(),
            min,
            position,
            max: position,
            encoder_counts_per_turn: 0,
            ready: false,
        }
    }
    pub const fn with_max(mut self, max: i64) -> Self {
        self.max = max;
        self
    }
    pub const fn with_ready(mut self, ready: bool) -> Self {
        self.ready = ready;
        self
    }
    pub const fn with_encoder_counts_per_turn(mut self, encoder_counts_per_turn: i64) -> Self {
        self.encoder_counts_per_turn = encoder_counts_per_turn;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn min(&self) -> i64 {
        self.min
    }
    pub const fn position(&self) -> i64 {
        self.position
    }
    pub const fn max(&self) -> i64 {
        self.max
    }
    pub const fn encoder_counts_per_turn(&self) -> i64 {
        self.encoder_counts_per_turn
    }
    pub const fn ready(&self) -> bool {
        self.ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MotorAssessmentTest {
        diagnostics: JointDiagnostics,
        limits: MotorLimits,
    }

    impl MotorAssessmentTest {
        fn new(
            electrical: MotorElectricalReadings,
            load: Option<f64>,
            limits: MotorLimits,
        ) -> Self {
            Self {
                diagnostics: JointDiagnostics::new(electrical, load, Some(0.0)),
                limits,
            }
        }

        fn limits() -> MotorLimits {
            MotorLimits::default()
                .with_max_temperature_celsius(Some(70.0))
                .with_voltage_range(Some(4.5), Some(9.0))
                .with_max_load(Some(1.0))
        }

        fn with_faults(mut self, faults: Vec<MotorFault>) -> Self {
            self.diagnostics = self.diagnostics.with_faults(faults);
            self
        }

        fn condition(self) -> MotorCondition {
            self.diagnostics.assess(self.limits)
        }
    }

    #[test]
    fn mode_selection_round_trips_through_its_tag() {
        let cases = [
            (ModeSelection::Idle, r#"{"mode":"idle"}"#),
            (
                ModeSelection::Manual {
                    robot: "arm".to_owned(),
                },
                r#"{"mode":"manual","robot":"arm"}"#,
            ),
            (
                ModeSelection::Teleoperate {
                    leader: "left".to_owned(),
                    follower: "right".to_owned(),
                },
                r#"{"mode":"teleoperate","leader":"left","follower":"right"}"#,
            ),
            (
                ModeSelection::Policy {
                    robot: "right".to_owned(),
                    session: "rollout-1".to_owned(),
                },
                r#"{"mode":"policy","robot":"right","session":"rollout-1"}"#,
            ),
        ];

        for (mode, json) in cases {
            assert_eq!(serde_json::to_string(&mode).unwrap(), json);
            assert_eq!(serde_json::from_str::<ModeSelection>(json).unwrap(), mode);
        }
    }

    #[test]
    fn an_arms_condition_is_the_worst_of_its_motors() {
        assert_eq!(
            [
                MotorCondition::Nominal,
                MotorCondition::Fault,
                MotorCondition::Caution,
            ]
            .into_iter()
            .max(),
            Some(MotorCondition::Fault)
        );
    }

    #[test]
    fn reported_limits_and_faults_determine_motor_condition() {
        assert_eq!(
            MotorAssessmentTest::new(
                MotorElectricalReadings::new(Some(30.0), Some(7.4), Some(0.2)),
                Some(0.1),
                MotorAssessmentTest::limits(),
            )
            .condition(),
            MotorCondition::Nominal
        );
        assert_eq!(
            MotorAssessmentTest::new(
                MotorElectricalReadings::new(Some(59.5), Some(7.4), Some(0.2)),
                Some(-0.9),
                MotorAssessmentTest::limits(),
            )
            .condition(),
            MotorCondition::Caution
        );
        assert_eq!(
            MotorAssessmentTest::new(
                MotorElectricalReadings::new(Some(30.0), Some(7.4), Some(0.2)),
                Some(0.1),
                MotorAssessmentTest::limits(),
            )
            .with_faults(vec![MotorFault::Overheat])
            .condition(),
            MotorCondition::Fault
        );
    }

    #[test]
    fn missing_limits_do_not_invent_thresholds_but_invalid_evidence_faults() {
        assert_eq!(
            MotorAssessmentTest::new(
                MotorElectricalReadings::new(Some(200.0), Some(7.4), Some(0.2)),
                Some(0.1),
                MotorLimits::default(),
            )
            .condition(),
            MotorCondition::Nominal
        );
        assert_eq!(
            MotorAssessmentTest::new(
                MotorElectricalReadings::new(Some(f64::NAN), Some(7.4), Some(0.2)),
                Some(0.1),
                MotorLimits::default(),
            )
            .condition(),
            MotorCondition::Fault
        );
    }
}

mod requests;
pub use requests::RobotGoal;

pub mod config;

/// Current native hardware observations, independent of saved robot configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HardwareStatus {
    mode: ModeStatus,
    cameras: Vec<CameraStatus>,
    issues: Vec<String>,
}
impl HardwareStatus {
    pub fn new(mode: ModeStatus, cameras: Vec<CameraStatus>, issues: Vec<String>) -> Self {
        Self {
            mode,
            cameras,
            issues,
        }
    }
    pub fn into_parts(self) -> (ModeStatus, Vec<CameraStatus>, Vec<String>) {
        (self.mode, self.cameras, self.issues)
    }
}
