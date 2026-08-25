//! Pure operating policy for robots, episodes, and motor health.

/// Follow error at which an arm is no longer considered settled.
pub const CAUTION_FOLLOW_ERROR_DEGREES: f64 = 5.0;

/// Fraction of an EEPROM-reported limit at which the operator should be warned.
pub const CAUTION_LIMIT_FRACTION: f64 = 0.85;

/// Quiet margin inside a motor's voltage window.
pub const CAUTION_VOLTAGE_MARGIN_VOLTS: f64 = 0.3;

/// Shortest operator-approved policy rollout.
pub const MIN_ROLLOUT_DURATION_SECONDS: u32 = 1;

/// Initial policy rollout duration shown to an operator.
pub const DEFAULT_ROLLOUT_DURATION_SECONDS: u32 = 30;

/// Longest continuous policy rollout an operator may approve.
pub const MAX_ROLLOUT_DURATION_SECONDS: u32 = 600;

/// Maximum predicted-action prefix consumed before rollout observes and replans.
pub const MAX_ROLLOUT_ACTION_STEPS: usize = 20;

/// A policy rollout duration admitted by the product safety window.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RolloutDuration(u32);

impl RolloutDuration {
    pub const fn new(seconds: u32) -> Option<Self> {
        if seconds >= MIN_ROLLOUT_DURATION_SECONDS && seconds <= MAX_ROLLOUT_DURATION_SECONDS {
            Some(Self(seconds))
        } else {
            None
        }
    }

    pub const fn seconds(self) -> u32 {
        self.0
    }
}

/// Plain robot facts required to select a teleoperation pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleoperationCandidate {
    id: String,
    robot_type: Option<String>,
    readiness: RobotReadiness,
    joints: Vec<String>,
}

/// The independent conditions required before a robot may teleoperate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RobotReadiness {
    connected: bool,
    calibrated: bool,
    configuration_valid: bool,
}

impl RobotReadiness {
    pub const fn new(connected: bool, calibrated: bool, configuration_valid: bool) -> Self {
        Self {
            connected,
            calibrated,
            configuration_valid,
        }
    }

    const fn ready(self) -> bool {
        self.connected && self.calibrated && self.configuration_valid
    }
}

impl TeleoperationCandidate {
    pub fn new(id: impl Into<String>, readiness: RobotReadiness, joints: Vec<String>) -> Self {
        Self {
            id: id.into(),
            robot_type: None,
            readiness,
            joints,
        }
    }

    pub fn with_robot_type(mut self, robot_type: Option<String>) -> Self {
        self.robot_type = robot_type;
        self
    }

    fn ready(&self) -> bool {
        self.readiness.ready()
    }

    fn has_role(&self, role: &str) -> bool {
        self.robot_type
            .as_deref()
            .is_some_and(|kind| kind.to_ascii_lowercase().contains(role))
    }

    fn compatible_with(&self, other: &Self) -> bool {
        self.joints.iter().any(|joint| other.joints.contains(joint))
    }
}

/// Candidate robots considered together for deterministic pair selection.
#[derive(Clone, Copy, Debug)]
pub struct TeleoperationCandidates<'a>(&'a [TeleoperationCandidate]);

impl<'a> From<&'a [TeleoperationCandidate]> for TeleoperationCandidates<'a> {
    fn from(candidates: &'a [TeleoperationCandidate]) -> Self {
        Self(candidates)
    }
}

/// Choose the best ready, compatible leader/follower pair.
///
/// Saved roles are preferences. Workspace order is the deterministic fallback
/// when otherwise compatible arms have no role metadata.
impl TeleoperationCandidates<'_> {
    pub fn best_pair(self) -> Option<(String, String)> {
        let ready = self
            .0
            .iter()
            .filter(|robot| robot.ready())
            .collect::<Vec<_>>();

        ready
            .iter()
            .enumerate()
            .flat_map(|(leader_index, leader)| {
                ready
                    .iter()
                    .enumerate()
                    .filter(move |(follower_index, follower)| {
                        leader_index != *follower_index && leader.compatible_with(follower)
                    })
                    .map(move |(follower_index, follower)| {
                        let role_penalty = u8::from(!leader.has_role("leader"))
                            + u8::from(!follower.has_role("follower"));
                        (
                            (role_penalty, leader_index, follower_index),
                            (leader.id.clone(), follower.id.clone()),
                        )
                    })
            })
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, pair)| pair)
    }
}

/// Plain motor readings used by the health rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotorHealthReadings {
    faulted: bool,
    temperature_celsius: Option<f64>,
    voltage_volts: Option<f64>,
    load: Option<f64>,
}

/// Hardware-declared limits used by the health rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotorHealthLimits {
    max_temperature_celsius: Option<f64>,
    min_voltage_volts: Option<f64>,
    max_voltage_volts: Option<f64>,
    max_load: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum MotorHealth {
    #[default]
    Nominal,
    Caution,
    Fault,
}

/// Judge one motor from its own fault bits and declared operating limits.
///
/// Missing optional measurements remain unknown and do not invent a fault.
/// A present non-finite measurement or limit is different: it violates this
/// boundary's physical-unit contract, so the existing fail-safe `Fault` state
/// is used rather than reporting a motor with unusable health evidence as
/// nominal.
impl MotorHealthReadings {
    pub const fn new(faulted: bool) -> Self {
        Self {
            faulted,
            temperature_celsius: None,
            voltage_volts: None,
            load: None,
        }
    }

    pub fn with_temperature_celsius(mut self, temperature_celsius: Option<f64>) -> Self {
        self.temperature_celsius = temperature_celsius;
        self
    }

    pub fn with_voltage_volts(mut self, voltage_volts: Option<f64>) -> Self {
        self.voltage_volts = voltage_volts;
        self
    }

    pub fn with_load(mut self, load: Option<f64>) -> Self {
        self.load = load;
        self
    }

    pub fn assess(self, limits: MotorHealthLimits) -> MotorHealth {
        let readings = self;
        let readings_are_invalid = [
            readings.temperature_celsius,
            readings.voltage_volts,
            readings.load,
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite());
        let limits_are_invalid = [
            limits.max_temperature_celsius,
            limits.min_voltage_volts,
            limits.max_voltage_volts,
            limits.max_load,
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite());
        let limits_are_physically_invalid = limits
            .max_temperature_celsius
            .is_some_and(|value| value <= 0.0)
            || limits.min_voltage_volts.is_some_and(|value| value <= 0.0)
            || limits.max_voltage_volts.is_some_and(|value| value <= 0.0)
            || limits.max_load.is_some_and(|value| value <= 0.0)
            || matches!(
                (limits.min_voltage_volts, limits.max_voltage_volts),
                (Some(minimum), Some(maximum)) if minimum >= maximum
            );
        if readings.faulted
            || readings_are_invalid
            || limits_are_invalid
            || limits_are_physically_invalid
        {
            return MotorHealth::Fault;
        }

        let hot = match (readings.temperature_celsius, limits.max_temperature_celsius) {
            (Some(celsius), Some(limit)) => celsius >= limit * CAUTION_LIMIT_FRACTION,
            _ => false,
        };
        let strained = match (readings.load, limits.max_load) {
            (Some(load), Some(limit)) => load.abs() >= limit * CAUTION_LIMIT_FRACTION,
            _ => false,
        };
        let supply = readings.voltage_volts.is_some_and(|volts| {
            limits
                .min_voltage_volts
                .is_some_and(|min| volts <= min + CAUTION_VOLTAGE_MARGIN_VOLTS)
                || limits
                    .max_voltage_volts
                    .is_some_and(|max| volts >= max - CAUTION_VOLTAGE_MARGIN_VOLTS)
        });

        if hot || strained || supply {
            MotorHealth::Caution
        } else {
            MotorHealth::Nominal
        }
    }
}

impl MotorHealthLimits {
    pub fn with_max_temperature_celsius(mut self, max_temperature_celsius: Option<f64>) -> Self {
        self.max_temperature_celsius = max_temperature_celsius;
        self
    }

    pub fn with_voltage_range(mut self, voltage_range: Option<(f64, f64)>) -> Self {
        (self.min_voltage_volts, self.max_voltage_volts) = voltage_range.unzip();
        self
    }

    pub fn with_min_voltage_volts(mut self, min_voltage_volts: Option<f64>) -> Self {
        self.min_voltage_volts = min_voltage_volts;
        self
    }

    pub fn with_max_voltage_volts(mut self, max_voltage_volts: Option<f64>) -> Self {
        self.max_voltage_volts = max_voltage_volts;
        self
    }

    pub fn with_max_load(mut self, max_load: Option<f64>) -> Self {
        self.max_load = max_load;
        self
    }
}

/// A completed take needs both a valid footer and at least one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpisodeOutcome {
    frames: u64,
    has_problem: bool,
}

impl EpisodeOutcome {
    pub const fn new(frames: u64, has_problem: bool) -> Self {
        Self {
            frames,
            has_problem,
        }
    }

    pub const fn is_usable(self) -> bool {
        !self.has_problem && self.frames > 0
    }
}

/// Lifecycle activity shared by training and rollout entities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationLifecycle {
    Pending,
    Running,
    Stopping,
    Finished,
}

impl OperationLifecycle {
    pub const fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Stopping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn teleoperation_candidate(id: &str, role: Option<&str>) -> TeleoperationCandidate {
        TeleoperationCandidate::new(
            id,
            RobotReadiness::new(true, true, true),
            vec!["shoulder".to_owned()],
        )
        .with_robot_type(role.map(str::to_owned))
    }

    #[test]
    fn pairing_prefers_roles_and_requires_readiness_and_compatibility() {
        let fallback = teleoperation_candidate("first", None);
        let follower = teleoperation_candidate("follower", Some("so101_follower"));
        let leader = teleoperation_candidate("leader", Some("so101_leader"));
        let candidates = [fallback, follower, leader];
        assert_eq!(
            TeleoperationCandidates::from(candidates.as_slice()).best_pair(),
            Some(("leader".to_owned(), "follower".to_owned()))
        );

        let invalid = TeleoperationCandidate::new(
            "invalid",
            RobotReadiness::new(true, true, false),
            vec!["shoulder".to_owned()],
        )
        .with_robot_type(Some("leader".to_owned()));
        assert_eq!(
            TeleoperationCandidates::from([invalid].as_slice()).best_pair(),
            None
        );
    }

    #[test]
    fn motor_faults_outrank_limits_and_limit_margins_warn() {
        let limits = MotorHealthLimits {
            max_temperature_celsius: Some(70.0),
            ..MotorHealthLimits::default()
        };
        assert_eq!(
            MotorHealthReadings {
                temperature_celsius: Some(70.0 * CAUTION_LIMIT_FRACTION),
                ..MotorHealthReadings::default()
            }
            .assess(limits),
            MotorHealth::Caution
        );
        assert_eq!(
            MotorHealthReadings {
                faulted: true,
                ..MotorHealthReadings::default()
            }
            .assess(MotorHealthLimits::default()),
            MotorHealth::Fault
        );
    }

    #[test]
    fn present_non_finite_health_evidence_is_never_nominal() {
        let reading_cases = [
            MotorHealthReadings {
                temperature_celsius: Some(f64::NAN),
                ..MotorHealthReadings::default()
            },
            MotorHealthReadings {
                voltage_volts: Some(f64::INFINITY),
                ..MotorHealthReadings::default()
            },
            MotorHealthReadings {
                load: Some(f64::NEG_INFINITY),
                ..MotorHealthReadings::default()
            },
        ];
        for readings in reading_cases {
            assert_eq!(
                readings.assess(MotorHealthLimits::default()),
                MotorHealth::Fault
            );
        }

        let limit_cases = [
            MotorHealthLimits {
                max_temperature_celsius: Some(f64::NAN),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                min_voltage_volts: Some(f64::NEG_INFINITY),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                max_voltage_volts: Some(f64::INFINITY),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                max_load: Some(f64::NAN),
                ..MotorHealthLimits::default()
            },
        ];
        for limits in limit_cases {
            assert_eq!(
                MotorHealthReadings::default().assess(limits),
                MotorHealth::Fault
            );
        }

        assert_eq!(
            MotorHealthReadings::default().assess(MotorHealthLimits::default()),
            MotorHealth::Nominal,
            "absence remains distinct from an invalid present value"
        );
    }

    #[test]
    fn present_physically_invalid_limits_are_faults_even_without_readings() {
        let invalid = [
            MotorHealthLimits {
                max_temperature_celsius: Some(0.0),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                min_voltage_volts: Some(-1.0),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                max_voltage_volts: Some(0.0),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                max_load: Some(-0.1),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                min_voltage_volts: Some(12.0),
                max_voltage_volts: Some(5.0),
                ..MotorHealthLimits::default()
            },
            MotorHealthLimits {
                min_voltage_volts: Some(7.4),
                max_voltage_volts: Some(7.4),
                ..MotorHealthLimits::default()
            },
        ];
        for limits in invalid {
            assert_eq!(
                MotorHealthReadings::default().assess(limits),
                MotorHealth::Fault
            );
        }
    }

    #[test]
    fn rollout_duration_is_bounded_policy() {
        assert!(RolloutDuration::new(MIN_ROLLOUT_DURATION_SECONDS).is_some());
        assert!(RolloutDuration::new(DEFAULT_ROLLOUT_DURATION_SECONDS).is_some());
        assert!(RolloutDuration::new(MAX_ROLLOUT_DURATION_SECONDS).is_some());
        assert!(RolloutDuration::new(0).is_none());
        assert!(RolloutDuration::new(MAX_ROLLOUT_DURATION_SECONDS + 1).is_none());
    }
}
