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

/// Whether a requested policy rollout stays within the product safety window.
pub const fn rollout_duration_is_valid(seconds: u32) -> bool {
    seconds >= MIN_ROLLOUT_DURATION_SECONDS && seconds <= MAX_ROLLOUT_DURATION_SECONDS
}

/// Plain robot facts required to select a teleoperation pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RobotCandidate {
    pub id: String,
    pub robot_type: Option<String>,
    pub connected: bool,
    pub calibrated: bool,
    pub valid: bool,
    pub joints: Vec<String>,
}

impl RobotCandidate {
    fn ready(&self) -> bool {
        self.connected && self.calibrated && self.valid
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

/// Choose the best ready, compatible leader/follower pair.
///
/// Saved roles are preferences. Workspace order is the deterministic fallback
/// when otherwise compatible arms have no role metadata.
pub fn choose_teleoperation_pair(robots: &[RobotCandidate]) -> Option<(String, String)> {
    let ready = robots
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

/// Plain motor readings used by the health rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotorHealthReadings {
    pub faulted: bool,
    pub temperature_celsius: Option<f64>,
    pub voltage_volts: Option<f64>,
    pub load: Option<f64>,
}

/// Hardware-declared limits used by the health rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotorHealthLimits {
    pub max_temperature_celsius: Option<f64>,
    pub min_voltage_volts: Option<f64>,
    pub max_voltage_volts: Option<f64>,
    pub max_load: Option<f64>,
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
pub fn assess_motor_health(
    readings: MotorHealthReadings,
    limits: MotorHealthLimits,
) -> MotorHealth {
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

/// A completed take needs both a valid footer and at least one frame.
pub const fn episode_is_usable(frames: u64, has_problem: bool) -> bool {
    !has_problem && frames > 0
}

/// Lifecycle activity shared by training and rollout entities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Pending,
    Running,
    Stopping,
    Finished,
}

impl Lifecycle {
    pub const fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Stopping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn robot(id: &str, role: Option<&str>) -> RobotCandidate {
        RobotCandidate {
            id: id.to_owned(),
            robot_type: role.map(str::to_owned),
            connected: true,
            calibrated: true,
            valid: true,
            joints: vec!["shoulder".to_owned()],
        }
    }

    #[test]
    fn pairing_prefers_roles_and_requires_readiness_and_compatibility() {
        let fallback = robot("first", None);
        let follower = robot("follower", Some("so101_follower"));
        let leader = robot("leader", Some("so101_leader"));
        assert_eq!(
            choose_teleoperation_pair(&[fallback, follower, leader]),
            Some(("leader".to_owned(), "follower".to_owned()))
        );

        let mut invalid = robot("invalid", Some("leader"));
        invalid.valid = false;
        assert_eq!(choose_teleoperation_pair(&[invalid]), None);
    }

    #[test]
    fn motor_faults_outrank_limits_and_limit_margins_warn() {
        let limits = MotorHealthLimits {
            max_temperature_celsius: Some(70.0),
            ..MotorHealthLimits::default()
        };
        assert_eq!(
            assess_motor_health(
                MotorHealthReadings {
                    temperature_celsius: Some(70.0 * CAUTION_LIMIT_FRACTION),
                    ..MotorHealthReadings::default()
                },
                limits
            ),
            MotorHealth::Caution
        );
        assert_eq!(
            assess_motor_health(
                MotorHealthReadings {
                    faulted: true,
                    ..MotorHealthReadings::default()
                },
                MotorHealthLimits::default()
            ),
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
                assess_motor_health(readings, MotorHealthLimits::default()),
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
                assess_motor_health(MotorHealthReadings::default(), limits),
                MotorHealth::Fault
            );
        }

        assert_eq!(
            assess_motor_health(MotorHealthReadings::default(), MotorHealthLimits::default()),
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
                assess_motor_health(MotorHealthReadings::default(), limits),
                MotorHealth::Fault
            );
        }
    }

    #[test]
    fn rollout_duration_is_bounded_policy() {
        assert!(rollout_duration_is_valid(MIN_ROLLOUT_DURATION_SECONDS));
        assert!(rollout_duration_is_valid(DEFAULT_ROLLOUT_DURATION_SECONDS));
        assert!(rollout_duration_is_valid(MAX_ROLLOUT_DURATION_SECONDS));
        assert!(!rollout_duration_is_valid(0));
        assert!(!rollout_duration_is_valid(MAX_ROLLOUT_DURATION_SECONDS + 1));
    }
}
