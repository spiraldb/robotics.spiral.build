//! Hardware-native command, telemetry, recording, and replay state.
//!
//! These values describe the control runtime without depending on HTTP or the
//! browser wire contract. The backend projects published state into
//! `spiral-robotics-api` DTOs, while storage consumes the narrow recording
//! boundary types.

/// Follow error at which an arm is no longer considered settled.
pub(crate) const CAUTION_FOLLOW_ERROR_DEGREES: f64 = 5.0;

const CAUTION_LIMIT_FRACTION: f64 = 0.85;
const CAUTION_VOLTAGE_MARGIN_VOLTS: f64 = 0.3;

/// Serial settings for one motor bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortSettings {
    path: String,
    baud_rate: u32,
}

impl PortSettings {
    pub fn new(path: impl Into<String>, baud_rate: u32) -> Result<Self, crate::HardwareInputError> {
        let path = path.into();
        if path.trim().is_empty() {
            return Err(crate::HardwareInputError::new(
                "serial port path must not be empty",
            ));
        }
        if baud_rate == 0 {
            return Err(crate::HardwareInputError::new(
                "serial port baud rate must be greater than zero",
            ));
        }
        Ok(Self { path, baud_rate })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn baud_rate(&self) -> u32 {
        self.baud_rate
    }
}

/// Whether one configured motor bus is currently open.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectionStatus {
    connected: bool,
    error: Option<String>,
}

impl ConnectionStatus {
    pub(crate) fn new(connected: bool, error: Option<String>) -> Self {
        Self { connected, error }
    }

    pub const fn connected(&self) -> bool {
        self.connected
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Exclusive control owner selected for the hardware runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ModeSelection {
    #[default]
    Idle,
    Manual {
        robot: String,
    },
    Teleoperate {
        leader: String,
        follower: String,
    },
    Replay {
        robot: String,
        episode: String,
    },
    Policy {
        robot: String,
        session: String,
    },
}

/// Selected control owner and its current activity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ModeStatus {
    selection: ModeSelection,
    active: bool,
    error: Option<String>,
}

impl ModeStatus {
    pub(crate) fn new(selection: ModeSelection, active: bool, error: Option<String>) -> Self {
        Self {
            selection,
            active,
            error,
        }
    }

    pub(crate) const fn selection(&self) -> &ModeSelection {
        &self.selection
    }

    pub(crate) const fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn update_activity(&mut self, active: bool, error: Option<String>) {
        self.active = active;
        self.error = error;
    }
}

/// Optional voltage limits reported by one motor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorVoltageRange {
    minimum: Option<f64>,
    maximum: Option<f64>,
}

impl MotorVoltageRange {
    pub(crate) const fn new(minimum: Option<f64>, maximum: Option<f64>) -> Self {
        Self { minimum, maximum }
    }
}

/// The physical limits one motor reports from its own configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotorLimits {
    max_temperature_celsius: Option<f64>,
    voltage: MotorVoltageRange,
    max_load: Option<f64>,
}

impl MotorLimits {
    pub(crate) const fn from_hardware_limits(
        max_temperature_celsius: Option<f64>,
        voltage: MotorVoltageRange,
        max_load: Option<f64>,
    ) -> Self {
        Self {
            max_temperature_celsius,
            voltage,
            max_load,
        }
    }

    pub const fn max_temperature_celsius(self) -> Option<f64> {
        self.max_temperature_celsius
    }

    pub const fn min_voltage_volts(self) -> Option<f64> {
        self.voltage.minimum
    }

    pub const fn max_voltage_volts(self) -> Option<f64> {
        self.voltage.maximum
    }

    pub const fn max_load(self) -> Option<f64> {
        self.max_load
    }
}

/// A fault bit reported by one motor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotorFault {
    Overheat,
    Overload,
    Voltage,
    Unrecognized,
}

/// Hardware-owned assessment of one motor against its reported limits.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum MotorCondition {
    #[default]
    Nominal,
    Caution,
    Fault,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MotorElectricalReadings {
    temperature_celsius: Option<f64>,
    voltage_volts: Option<f64>,
    current_amps: Option<f64>,
}

impl MotorElectricalReadings {
    pub(crate) const fn new(
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

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct JointDiagnostics {
    electrical: MotorElectricalReadings,
    load: Option<f64>,
    speed_degrees_per_second: Option<f64>,
    temperature_rise_per_minute: Option<f64>,
    faults: Vec<MotorFault>,
}

impl JointDiagnostics {
    pub(crate) fn new(
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

    pub(crate) fn with_temperature_rise(
        mut self,
        temperature_rise_per_minute: Option<f64>,
    ) -> Self {
        self.temperature_rise_per_minute = temperature_rise_per_minute;
        self
    }

    pub(crate) fn with_faults(mut self, faults: Vec<MotorFault>) -> Self {
        self.faults = faults;
        self
    }

    pub(crate) fn assess(self, limits: MotorLimits) -> AssessedJointDiagnostics {
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
            return AssessedJointDiagnostics::new(self, MotorCondition::Fault);
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
        let condition = if hot || strained || supply {
            MotorCondition::Caution
        } else {
            MotorCondition::Nominal
        };
        AssessedJointDiagnostics::new(self, condition)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AssessedJointDiagnostics {
    readings: JointDiagnostics,
    condition: MotorCondition,
}

impl AssessedJointDiagnostics {
    const fn new(readings: JointDiagnostics, condition: MotorCondition) -> Self {
        Self {
            readings,
            condition,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JointIdentity {
    name: String,
    id: u32,
}

impl JointIdentity {
    pub(crate) fn new(name: impl Into<String>, id: u32) -> Self {
        Self {
            name: name.into(),
            id,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JointRange {
    min: i64,
    center: i64,
    max: i64,
}

impl JointRange {
    pub(crate) const fn new(min: i64, center: i64, max: i64) -> Self {
        Self { min, center, max }
    }

    pub const fn min(self) -> i64 {
        self.min
    }

    pub const fn center(self) -> i64 {
        self.center
    }

    pub const fn max(self) -> i64 {
        self.max
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct JointKinematics {
    raw: i64,
    degrees: Option<f64>,
    normalized: Option<f64>,
    goal_raw: Option<i64>,
    target_raw: Option<i64>,
    target_degrees: Option<f64>,
    range: Option<JointRange>,
}

impl JointKinematics {
    pub(crate) const fn new(raw: i64, degrees: Option<f64>, normalized: Option<f64>) -> Self {
        Self {
            raw,
            degrees,
            normalized,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
        }
    }

    pub(crate) const fn with_raw_targets(
        mut self,
        goal_raw: Option<i64>,
        target_raw: Option<i64>,
    ) -> Self {
        self.goal_raw = goal_raw;
        self.target_raw = target_raw;
        self
    }

    pub(crate) const fn with_calibrated_target(
        mut self,
        target_degrees: Option<f64>,
        range: Option<JointRange>,
    ) -> Self {
        self.target_degrees = target_degrees;
        self.range = range;
        self
    }
}

/// One hardware-native joint sample.
#[derive(Clone, Debug, PartialEq)]
pub struct JointTelemetry {
    identity: JointIdentity,
    kinematics: JointKinematics,
    diagnostics: AssessedJointDiagnostics,
}

impl JointTelemetry {
    pub(crate) fn new(
        identity: JointIdentity,
        kinematics: JointKinematics,
        diagnostics: AssessedJointDiagnostics,
    ) -> Self {
        Self {
            identity,
            kinematics,
            diagnostics,
        }
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

    pub const fn goal_raw(&self) -> Option<i64> {
        self.kinematics.goal_raw
    }

    pub const fn target_raw(&self) -> Option<i64> {
        self.kinematics.target_raw
    }

    pub fn target_degrees(&self) -> Option<f64> {
        self.kinematics.target_degrees
    }

    pub fn range(&self) -> Option<JointRange> {
        self.kinematics.range
    }

    pub const fn temperature_celsius(&self) -> Option<f64> {
        self.diagnostics.readings.electrical.temperature_celsius
    }

    pub const fn voltage_volts(&self) -> Option<f64> {
        self.diagnostics.readings.electrical.voltage_volts
    }

    pub const fn current_amps(&self) -> Option<f64> {
        self.diagnostics.readings.electrical.current_amps
    }

    pub const fn load(&self) -> Option<f64> {
        self.diagnostics.readings.load
    }

    pub const fn speed_degrees_per_second(&self) -> Option<f64> {
        self.diagnostics.readings.speed_degrees_per_second
    }

    pub const fn temperature_rise_per_minute(&self) -> Option<f64> {
        self.diagnostics.readings.temperature_rise_per_minute
    }

    pub fn faults(&self) -> &[MotorFault] {
        &self.diagnostics.readings.faults
    }

    pub const fn condition(&self) -> MotorCondition {
        self.diagnostics.condition
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalibrationJointProgress {
    name: String,
    min: i64,
    position: i64,
    max: i64,
    encoder_counts_per_turn: i64,
}

impl CalibrationJointProgress {
    pub(crate) fn new(name: impl Into<String>, min: i64, position: i64) -> Self {
        Self {
            name: name.into(),
            min,
            position,
            max: position,
            encoder_counts_per_turn: crate::calibration::MotorSpec::encoder_counts_per_turn(),
        }
    }

    pub(crate) const fn with_max(mut self, max: i64) -> Self {
        self.max = max;
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

    pub fn ready(&self) -> bool {
        self.max.saturating_sub(self.min) >= crate::calibration::MINIMUM_CALIBRATION_TRAVEL
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationProgress {
    joints: Vec<CalibrationJointProgress>,
    ready: bool,
}

impl CalibrationProgress {
    pub(crate) fn new(joints: Vec<CalibrationJointProgress>, ready: bool) -> Self {
        Self { joints, ready }
    }

    pub fn joints(&self) -> &[CalibrationJointProgress] {
        &self.joints
    }

    pub const fn ready(&self) -> bool {
        self.ready
    }
}

/// One arm's live state.
#[derive(Clone, Debug, PartialEq)]
pub struct RobotTelemetry {
    id: String,
    torque: bool,
    tracking_lagging: bool,
    joints: Vec<JointTelemetry>,
    error: Option<String>,
    calibration: Option<CalibrationProgress>,
}

impl RobotTelemetry {
    pub(crate) fn new(id: impl Into<String>, torque: bool, joints: Vec<JointTelemetry>) -> Self {
        Self {
            id: id.into(),
            torque,
            tracking_lagging: false,
            joints,
            error: None,
            calibration: None,
        }
    }

    pub(crate) fn with_error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    pub(crate) const fn with_tracking_lagging(mut self, tracking_lagging: bool) -> Self {
        self.tracking_lagging = tracking_lagging;
        self
    }

    pub(crate) fn with_calibration(mut self, calibration: Option<CalibrationProgress>) -> Self {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpisodeArm {
    id: String,
    joints: Vec<String>,
}

impl EpisodeArm {
    pub fn new(id: impl Into<String>, joints: Vec<String>) -> Self {
        Self {
            id: id.into(),
            joints,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn joints(&self) -> &[String] {
        &self.joints
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordingIdentity {
    episode: String,
    dataset_id: String,
    arms: Vec<String>,
}

impl RecordingIdentity {
    pub(crate) fn new(
        episode: impl Into<String>,
        dataset_id: impl Into<String>,
        arms: Vec<String>,
    ) -> Self {
        Self {
            episode: episode.into(),
            dataset_id: dataset_id.into(),
            arms,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RecordingClock {
    frames: u64,
    elapsed_seconds: f64,
    wall_elapsed_seconds: f64,
    paused_seconds: f64,
}

impl RecordingClock {
    pub(crate) const fn new(frames: u64, elapsed_seconds: f64, wall_elapsed_seconds: f64) -> Self {
        Self {
            frames,
            elapsed_seconds,
            wall_elapsed_seconds,
            paused_seconds: 0.0,
        }
    }

    pub(crate) const fn with_paused_seconds(mut self, paused_seconds: f64) -> Self {
        self.paused_seconds = paused_seconds;
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RecordingState {
    paused: bool,
    max_follow_error_degrees: Option<f64>,
    error: Option<String>,
}

impl RecordingState {
    pub(crate) fn new(
        paused: bool,
        max_follow_error_degrees: Option<f64>,
        error: Option<String>,
    ) -> Self {
        Self {
            paused,
            max_follow_error_degrees,
            error,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingProgress {
    identity: RecordingIdentity,
    clock: RecordingClock,
    state: RecordingState,
}

impl RecordingProgress {
    pub(crate) fn new(
        identity: RecordingIdentity,
        clock: RecordingClock,
        state: RecordingState,
    ) -> Self {
        Self {
            identity,
            clock,
            state,
        }
    }

    pub fn episode(&self) -> &str {
        &self.identity.episode
    }

    pub fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }

    pub fn arms(&self) -> &[String] {
        &self.identity.arms
    }

    pub const fn frames(&self) -> u64 {
        self.clock.frames
    }

    pub const fn elapsed_seconds(&self) -> f64 {
        self.clock.elapsed_seconds
    }

    pub const fn wall_elapsed_seconds(&self) -> f64 {
        self.clock.wall_elapsed_seconds
    }

    pub const fn paused_seconds(&self) -> f64 {
        self.clock.paused_seconds
    }

    pub const fn paused(&self) -> bool {
        self.state.paused
    }

    pub const fn max_follow_error_degrees(&self) -> Option<f64> {
        self.state.max_follow_error_degrees
    }

    pub fn error(&self) -> Option<&str> {
        self.state.error.as_deref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordingGap {
    after_seconds: f64,
    duration_seconds: f64,
}

impl RecordingGap {
    pub(crate) const fn new(after_seconds: f64, duration_seconds: f64) -> Self {
        Self {
            after_seconds,
            duration_seconds,
        }
    }

    pub const fn after_seconds(self) -> f64 {
        self.after_seconds
    }

    pub const fn duration_seconds(self) -> f64 {
        self.duration_seconds
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplayPurpose {
    #[default]
    Timeline,
    PhysicalSeek,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplayPhase {
    Seeking,
    #[default]
    Playing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReplayIdentity {
    episode: String,
    robot: String,
}

impl ReplayIdentity {
    pub(crate) fn new(episode: impl Into<String>, robot: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
            robot: robot.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ReplayTimeline {
    frame: u64,
    frames: u64,
    elapsed_seconds: f64,
    duration_seconds: f64,
}

impl ReplayTimeline {
    pub(crate) const fn new(frame: u64, frames: u64, elapsed_seconds: f64) -> Self {
        Self {
            frame,
            frames,
            elapsed_seconds,
            duration_seconds: 0.0,
        }
    }

    pub(crate) const fn with_duration_seconds(mut self, duration_seconds: f64) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReplayTransport {
    purpose: ReplayPurpose,
    phase: ReplayPhase,
    paused: bool,
}

impl ReplayTransport {
    pub(crate) const fn new(purpose: ReplayPurpose, phase: ReplayPhase, paused: bool) -> Self {
        Self {
            purpose,
            phase,
            paused,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayProgress {
    identity: ReplayIdentity,
    timeline: ReplayTimeline,
    transport: ReplayTransport,
}

impl ReplayProgress {
    pub(crate) fn new(
        identity: ReplayIdentity,
        timeline: ReplayTimeline,
        transport: ReplayTransport,
    ) -> Self {
        Self {
            identity,
            timeline,
            transport,
        }
    }

    pub fn episode(&self) -> &str {
        &self.identity.episode
    }

    pub fn robot(&self) -> &str {
        &self.identity.robot
    }

    pub const fn frame(&self) -> u64 {
        self.timeline.frame
    }

    pub const fn frames(&self) -> u64 {
        self.timeline.frames
    }

    pub const fn elapsed_seconds(&self) -> f64 {
        self.timeline.elapsed_seconds
    }

    pub const fn duration_seconds(&self) -> f64 {
        self.timeline.duration_seconds
    }

    pub const fn purpose(&self) -> ReplayPurpose {
        self.transport.purpose
    }

    pub const fn phase(&self) -> ReplayPhase {
        self.transport.phase
    }

    pub const fn paused(&self) -> bool {
        self.transport.paused
    }
}

/// Narrow durable result returned to the hardware thread after storage commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedRecording {
    episode: String,
    frames: u64,
}

impl CompletedRecording {
    pub fn new(episode: impl Into<String>, frames: u64) -> Self {
        Self {
            episode: episode.into(),
            frames,
        }
    }

    pub fn episode(&self) -> &str {
        &self.episode
    }

    pub(crate) const fn frames(&self) -> u64 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(
        electrical: MotorElectricalReadings,
        load: Option<f64>,
        faults: Vec<MotorFault>,
        limits: MotorLimits,
    ) -> MotorCondition {
        JointTelemetry::new(
            JointIdentity::new("shoulder_pan", 1),
            JointKinematics::new(2048, None, None),
            JointDiagnostics::new(electrical, load, Some(0.0))
                .with_faults(faults)
                .assess(limits),
        )
        .condition()
    }

    fn limits() -> MotorLimits {
        MotorLimits::from_hardware_limits(
            Some(70.0),
            MotorVoltageRange::new(Some(4.5), Some(9.0)),
            Some(1.0),
        )
    }

    #[test]
    fn reported_limits_and_faults_determine_motor_condition() {
        assert_eq!(
            condition(
                MotorElectricalReadings::new(Some(30.0), Some(7.4), Some(0.2)),
                Some(0.1),
                Vec::new(),
                limits(),
            ),
            MotorCondition::Nominal
        );
        assert_eq!(
            condition(
                MotorElectricalReadings::new(
                    Some(70.0 * CAUTION_LIMIT_FRACTION),
                    Some(7.4),
                    Some(0.2),
                ),
                Some(-0.9),
                Vec::new(),
                limits(),
            ),
            MotorCondition::Caution
        );
        assert_eq!(
            condition(
                MotorElectricalReadings::new(Some(30.0), Some(7.4), Some(0.2)),
                Some(0.1),
                vec![MotorFault::Overheat],
                limits(),
            ),
            MotorCondition::Fault
        );
    }

    #[test]
    fn missing_limits_do_not_invent_thresholds_but_invalid_evidence_faults() {
        assert_eq!(
            condition(
                MotorElectricalReadings::new(Some(200.0), Some(7.4), Some(0.2)),
                Some(0.1),
                Vec::new(),
                MotorLimits::default(),
            ),
            MotorCondition::Nominal
        );
        assert_eq!(
            condition(
                MotorElectricalReadings::new(Some(f64::NAN), Some(7.4), Some(0.2)),
                Some(0.1),
                Vec::new(),
                MotorLimits::default(),
            ),
            MotorCondition::Fault
        );
    }
}
