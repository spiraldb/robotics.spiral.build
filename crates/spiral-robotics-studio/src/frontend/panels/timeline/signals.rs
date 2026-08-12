//! Bounded signal samples consumed by the Timeline panel.
//!
//! This module is deliberately a data boundary, not a view. The event stream
//! writes telemetry here and the shared Timeline adapts the store into its
//! fixed six-lane model. Keeping rendering and interaction out of the store
//! prevents a second timeline implementation from growing beside the one users
//! actually operate.

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
use spiral_robotics_core::api::TelemetryFrame;
use spiral_robotics_core::arm;

/// Thirty minutes at the studio's 20 Hz telemetry publish rate.
#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
const MAX_SAMPLES: usize = 36_000;

/// Drop samples in chunks so trimming remains amortized.
#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
const TRIM_CHUNK: usize = 3_600;

const SIGNAL_COLORS: [&str; 6] = [
    "#2a78d6", // shoulder pan
    "#eb6834", // shoulder lift
    "#1baf7a", // elbow flex
    "#eda100", // wrist flex
    "#e87ba4", // wrist roll
    "#008300", // gripper
];

/// Stable joint colour keyed by the canonical SO-101 joint name.
pub(crate) fn signal_color(name: &str) -> &'static str {
    let slot = arm::SO101_JOINTS
        .iter()
        .position(|joint| joint.name == name)
        .unwrap_or_else(|| {
            name.bytes().fold(0usize, |hash, byte| {
                hash.wrapping_mul(31).wrapping_add(usize::from(byte))
            }) % SIGNAL_COLORS.len()
        });
    SIGNAL_COLORS[slot % SIGNAL_COLORS.len()]
}

/// One joint's measured angle at every sample in a take.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct JointSignal {
    pub(crate) name: String,
    /// `None` means that no calibrated angle was reported for that sample.
    pub(crate) values: Vec<Option<f32>>,
}

/// One arm's joint signals, in the order first reported by that arm.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ArmSignals {
    pub(crate) id: String,
    pub(crate) joints: Vec<JointSignal>,
}

/// Samples for the active take, or the last take after recording stops.
///
/// All arms share one time column because the control loop samples them on the
/// same frame. Every joint vector is kept index-aligned with that column.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SignalStore {
    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    take: Option<String>,
    times: Vec<f32>,
    arms: Vec<ArmSignals>,
    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    live: bool,
    /// Consecutive telemetry frames that reported no active recording.
    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    idle: u8,
}

impl SignalStore {
    pub(crate) fn arms(&self) -> &[ArmSignals] {
        &self.arms
    }

    pub(crate) fn times(&self) -> &[f32] {
        &self.times
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    pub(crate) fn live(&self) -> bool {
        self.live
    }

    /// Fold one telemetry frame into the active take.
    //
    // A single idle frame can be a publication race between recorder state and
    // telemetry. Two consecutive idle frames end the take while preserving it
    // for inspection in the Timeline.
    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    pub(crate) fn observe(&mut self, frame: &TelemetryFrame) {
        let Some(recording) = frame.recording.as_ref() else {
            self.idle = self.idle.saturating_add(1);
            if self.idle >= 2 {
                self.live = false;
            }
            return;
        };
        self.idle = 0;

        // Episode ids can repeat after a server restart, so the clock and live
        // transition also participate in take identity.
        let different_take = self.take.as_deref() != Some(recording.episode.as_str());
        let resumed = !self.live;
        let rewound = self
            .times
            .last()
            .is_some_and(|last| (recording.elapsed_seconds as f32) < *last);
        if different_take || resumed || rewound {
            self.reset(&recording.episode);
        }
        self.live = true;

        self.times.push(recording.elapsed_seconds as f32);
        let sample_count = self.times.len();

        for robot in &frame.robots {
            let arm = match self.arms.iter_mut().position(|arm| arm.id == robot.id) {
                Some(index) => &mut self.arms[index],
                None => {
                    self.arms.push(ArmSignals {
                        id: robot.id.clone(),
                        joints: Vec::new(),
                    });
                    self.arms.last_mut().expect("an arm was just appended")
                }
            };

            for joint in &robot.joints {
                let signal = match arm
                    .joints
                    .iter_mut()
                    .find(|signal| signal.name == joint.name)
                {
                    Some(signal) => signal,
                    None => {
                        arm.joints.push(JointSignal {
                            name: joint.name.clone(),
                            // An arm or joint appearing mid-take begins with a
                            // gap; its first reading is never backdated.
                            values: vec![None; sample_count - 1],
                        });
                        arm.joints.last_mut().expect("a joint was just appended")
                    }
                };
                signal
                    .values
                    .push(joint.degrees.map(|degrees| degrees as f32));
            }
        }

        // Missing readings become explicit gaps while every signal stays
        // aligned to the shared time column.
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.resize(sample_count, None);
            }
        }

        if self.times.len() > MAX_SAMPLES {
            self.trim();
        }
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    fn reset(&mut self, take: &str) {
        self.take = Some(take.to_owned());
        self.times.clear();
        self.arms.clear();
        self.idle = 0;
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    fn trim(&mut self) {
        self.times.drain(..TRIM_CHUNK);
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.drain(..TRIM_CHUNK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_core::api::{JointTelemetry, RecordingProgress, RobotTelemetry};

    fn joint(name: &str, degrees: Option<f64>) -> JointTelemetry {
        JointTelemetry {
            name: name.to_owned(),
            id: 1,
            raw: 0,
            degrees,
            normalized: None,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
            temperature_celsius: None,
            voltage_volts: None,
            current_amps: None,
            load: None,
            speed_degrees_per_second: None,
            temperature_rise_per_minute: None,
            faults: Vec::new(),
        }
    }

    fn frame(
        episode: Option<&str>,
        elapsed: f64,
        arms: Vec<(&str, Vec<JointTelemetry>)>,
    ) -> TelemetryFrame {
        TelemetryFrame {
            sequence: 0,
            loop_hz: 60.0,
            target_hz: 60.0,
            robots: arms
                .into_iter()
                .map(|(id, joints)| RobotTelemetry {
                    id: id.to_owned(),
                    torque: false,
                    joints,
                    error: None,
                    calibration: None,
                })
                .collect(),
            recording: episode.map(|episode| RecordingProgress {
                episode: episode.to_owned(),
                dataset_id: "default".to_owned(),
                frames: 1,
                elapsed_seconds: elapsed,
                wall_elapsed_seconds: elapsed,
                paused_seconds: 0.0,
                paused: false,
                arms: Vec::new(),
                max_follow_error_degrees: None,
                error: None,
            }),
            replay: None,
        }
    }

    fn reading(store: &SignalStore, arm: &str, joint: &str, at: usize) -> Option<f32> {
        store
            .arms
            .iter()
            .find(|candidate| candidate.id == arm)?
            .joints
            .iter()
            .find(|candidate| candidate.name == joint)?
            .values
            .get(at)
            .copied()
            .flatten()
    }

    #[test]
    fn idle_telemetry_is_not_recorded() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            None,
            0.0,
            vec![("leader", vec![joint("gripper", Some(4.0))])],
        ));

        assert!(store.times.is_empty());
        assert!(!store.live());
    }

    #[test]
    fn a_finished_take_stays_available_and_the_next_one_replaces_it() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("episode_000001"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("episode_000001"),
            0.05,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));

        store.observe(&frame(None, 0.0, vec![]));
        store.observe(&frame(None, 0.0, vec![]));
        assert_eq!(store.times.len(), 2);
        assert!(!store.live());

        store.observe(&frame(
            Some("episode_000002"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(9.0))])],
        ));
        assert_eq!(store.times.len(), 1);
        assert_eq!(store.take.as_deref(), Some("episode_000002"));
    }

    #[test]
    fn one_idle_frame_does_not_split_a_take() {
        let mut store = SignalStore::default();
        for sample in 0..4 {
            store.observe(&frame(
                Some("take"),
                f64::from(sample),
                vec![("leader", vec![joint("gripper", Some(1.0))])],
            ));
        }
        store.observe(&frame(None, 0.0, vec![]));
        assert!(store.live());
        store.observe(&frame(
            Some("take"),
            4.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        assert_eq!(store.times.len(), 5);
    }

    #[test]
    fn a_rewound_clock_replaces_samples_even_when_the_id_repeats() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("take"),
            9.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("take"),
            0.1,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));

        assert_eq!(store.times(), [0.1]);
    }

    #[test]
    fn a_joint_arriving_mid_take_is_padded_not_backdated() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("take"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("take"),
            0.05,
            vec![
                ("leader", vec![joint("gripper", Some(2.0))]),
                ("follower", vec![joint("gripper", Some(7.0))]),
            ],
        ));

        assert_eq!(reading(&store, "follower", "gripper", 0), None);
        assert_eq!(reading(&store, "follower", "gripper", 1), Some(7.0));
    }

    #[test]
    fn a_missing_joint_reading_leaves_an_aligned_gap() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("take"),
            0.0,
            vec![(
                "leader",
                vec![joint("gripper", Some(1.0)), joint("wrist_roll", Some(3.0))],
            )],
        ));
        store.observe(&frame(
            Some("take"),
            0.05,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));

        assert_eq!(reading(&store, "leader", "wrist_roll", 1), None);
        assert_eq!(reading(&store, "leader", "gripper", 1), Some(2.0));
        assert!(
            store
                .arms()
                .iter()
                .flat_map(|arm| &arm.joints)
                .all(|joint| joint.values.len() == store.times().len())
        );
    }

    #[test]
    fn a_long_take_is_trimmed_from_the_front() {
        let mut store = SignalStore::default();
        for sample in 0..=MAX_SAMPLES {
            store.observe(&frame(
                Some("take"),
                sample as f64 * 0.05,
                vec![("leader", vec![joint("gripper", Some(sample as f64))])],
            ));
        }

        assert_eq!(store.times.len(), MAX_SAMPLES + 1 - TRIM_CHUNK);
        assert_eq!(
            reading(&store, "leader", "gripper", 0),
            Some(TRIM_CHUNK as f32)
        );
    }

    #[test]
    fn joint_colours_are_stable_when_a_motor_is_missing() {
        assert_eq!(signal_color("shoulder_pan"), SIGNAL_COLORS[0]);
        assert_eq!(signal_color("elbow_flex"), SIGNAL_COLORS[2]);
        assert_eq!(signal_color("gripper"), SIGNAL_COLORS[5]);
        assert_eq!(signal_color("extra_axis"), signal_color("extra_axis"));
    }
}
