//! Backend-retained recording chart samples and cursor-based reads.
use crate::robots::TelemetryFrame;
use serde::{Deserialize, Serialize};
/// Thirty minutes at the hardware's 20 Hz publication rate.
const MAX_SAMPLES: usize = 36_000;
const TRIM_CHUNK: usize = 3_600;
/// One joint's measured angle at every sample in a take.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JointSignal {
    name: String,
    /// `None` means that no calibrated angle was reported for that sample.
    values: Vec<Option<f32>>,
}

impl JointSignal {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn values(&self) -> &[Option<f32>] {
        &self.values
    }
}

/// One arm's joint signals, in the order first reported by that arm.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArmSignals {
    id: String,
    joints: Vec<JointSignal>,
}

impl ArmSignals {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn joints(&self) -> &[JointSignal] {
        &self.joints
    }
}

/// Samples for the active take, or the last take after recording stops.
///
/// All arms share one time column because the control loop samples them on the
/// same frame. Every joint vector is kept index-aligned with that column.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordingSignals {
    take: Option<String>,
    times: Vec<f32>,
    arms: Vec<ArmSignals>,
    live: bool,
    /// Consecutive telemetry frames that reported no active recording.
    idle: u8,
    generation: u64,
    offset: u64,
}

impl RecordingSignals {
    pub fn arms(&self) -> &[ArmSignals] {
        &self.arms
    }

    pub fn times(&self) -> &[f32] {
        &self.times
    }

    pub fn live(&self) -> bool {
        self.live
    }

    /// Fold one telemetry frame into the active take.
    //
    // A single idle frame can be a publication race between recorder state and
    // telemetry. Two consecutive idle frames end the take while preserving it
    // for inspection in the Timeline.
    pub fn observe(&mut self, frame: &TelemetryFrame) {
        let Some(recording) = frame.recording() else {
            self.idle = self.idle.saturating_add(1);
            if self.idle >= 2 {
                self.live = false;
            }
            return;
        };
        self.idle = 0;

        // Episode ids can repeat after a server restart, so the clock and live
        // transition also participate in take identity.
        let different_take = self.take.as_deref() != Some(recording.episode());
        let resumed = !self.live;
        let rewound = self
            .times
            .last()
            .is_some_and(|last| (recording.elapsed_seconds() as f32) < *last);
        if different_take || resumed || rewound {
            self.reset(recording.episode());
        }
        self.live = true;

        self.times.push(recording.elapsed_seconds() as f32);
        let sample_count = self.times.len();

        for robot in frame.robots() {
            let arm = match self.arms.iter_mut().position(|arm| arm.id == robot.id()) {
                Some(index) => &mut self.arms[index],
                None => {
                    self.arms.push(ArmSignals {
                        id: robot.id().to_owned(),
                        joints: Vec::new(),
                    });
                    self.arms.last_mut().expect("an arm was just appended")
                }
            };

            for joint in robot.joints() {
                let signal = match arm
                    .joints
                    .iter_mut()
                    .find(|signal| signal.name == joint.name())
                {
                    Some(signal) => signal,
                    None => {
                        arm.joints.push(JointSignal {
                            name: joint.name().to_owned(),
                            // An arm or joint appearing mid-take begins with a
                            // gap; its first reading is never backdated.
                            values: vec![None; sample_count - 1],
                        });
                        arm.joints.last_mut().expect("a joint was just appended")
                    }
                };
                signal
                    .values
                    .push(joint.degrees().map(|degrees| degrees as f32));
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

    fn reset(&mut self, take: &str) {
        self.generation += 1;
        self.offset = 0;
        self.take = Some(take.to_owned());
        self.times.clear();
        self.arms.clear();
        self.idle = 0;
    }

    fn trim(&mut self) {
        self.offset += TRIM_CHUNK as u64;
        self.times.drain(..TRIM_CHUNK);
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.drain(..TRIM_CHUNK);
            }
        }
    }
}

/// The next sample wanted by a client, scoped to a take generation.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordingCursor {
    generation: u64,
    offset: u64,
}

/// A bounded page of backend-retained samples. Offset gaps explicitly replace
/// the local window rather than silently joining unrelated samples.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingWindow(RecordingSignals);

impl RecordingWindow {
    /// Empty polls must not invalidate an unchanged chart projection.
    pub fn changes(&self, current: &RecordingSignals) -> bool {
        self.0.cursor() != current.cursor()
            || self.0.take != current.take
            || self.0.live != current.live
    }
}

impl RecordingSignals {
    pub fn cursor(&self) -> RecordingCursor {
        RecordingCursor {
            generation: self.generation,
            offset: self.offset + self.times.len() as u64,
        }
    }
    pub fn clear(&mut self) {
        let generation = self.generation + 1;
        *self = Self {
            generation,
            ..Self::default()
        };
    }
    pub fn window(&self, cursor: RecordingCursor) -> RecordingWindow {
        let start = if cursor.generation == self.generation
            && cursor.offset >= self.offset
            && cursor.offset <= self.cursor().offset
        {
            (cursor.offset - self.offset) as usize
        } else {
            0
        };
        let end = (start + 1_000).min(self.times.len());
        RecordingWindow(Self {
            take: self.take.clone(),
            times: self.times[start..end].to_vec(),
            arms: self
                .arms
                .iter()
                .map(|arm| ArmSignals {
                    id: arm.id.clone(),
                    joints: arm
                        .joints
                        .iter()
                        .map(|joint| JointSignal {
                            name: joint.name.clone(),
                            values: joint.values[start..end].to_vec(),
                        })
                        .collect(),
                })
                .collect(),
            live: self.live,
            idle: self.idle,
            generation: self.generation,
            offset: self.offset + start as u64,
        })
    }
    pub fn apply_window(&mut self, window: RecordingWindow) {
        let next = window.0;
        if self.generation != next.generation || self.cursor().offset != next.offset {
            *self = next;
            return;
        }
        let previous_len = self.times.len();
        self.take = next.take;
        self.live = next.live;
        self.idle = next.idle;
        self.times.extend(next.times);
        for arm in next.arms {
            let index = self
                .arms
                .iter()
                .position(|existing| existing.id == arm.id)
                .unwrap_or_else(|| {
                    self.arms.push(ArmSignals {
                        id: arm.id,
                        joints: Vec::new(),
                    });
                    self.arms.len() - 1
                });
            let target = &mut self.arms[index];
            for joint in arm.joints {
                if let Some(existing) = target
                    .joints
                    .iter_mut()
                    .find(|existing| existing.name == joint.name)
                {
                    existing.values.extend(joint.values);
                } else {
                    let mut values = vec![None; previous_len];
                    values.extend(joint.values);
                    target.joints.push(JointSignal {
                        name: joint.name,
                        values,
                    });
                }
            }
        }
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.resize(self.times.len(), None);
            }
        }
        if self.times.len() > MAX_SAMPLES {
            self.trim();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{RecordingClock, RecordingIdentity, RecordingProgress, RecordingState};
    use crate::robots::{
        JointDiagnostics, JointIdentity, JointKinematics, JointTelemetry, RobotTelemetry,
    };

    fn joint(name: &str, degrees: Option<f64>) -> JointTelemetry {
        JointTelemetry::new(
            JointIdentity::new(name, 1),
            JointKinematics::new(0, degrees, None),
            JointDiagnostics::default(),
        )
    }

    fn frame(
        episode: Option<&str>,
        elapsed: f64,
        arms: Vec<(&str, Vec<JointTelemetry>)>,
    ) -> TelemetryFrame {
        TelemetryFrame::new(0, 60.0, 60.0)
            .with_robots(
                arms.into_iter()
                    .map(|(id, joints)| RobotTelemetry::new(id, false, joints))
                    .collect(),
            )
            .with_operations(
                episode.map(|episode| {
                    RecordingProgress::new(
                        RecordingIdentity::new(episode, "default", Vec::new()),
                        RecordingClock::new(1, elapsed, elapsed),
                        RecordingState::default(),
                    )
                }),
                None,
            )
    }

    fn reading(store: &RecordingSignals, arm: &str, joint: &str, at: usize) -> Option<f32> {
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
        let mut store = RecordingSignals::default();
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
    fn a_disconnected_browser_catches_up_in_bounded_pages_and_keeps_the_finished_take() {
        let mut backend = RecordingSignals::default();
        let mut frontend = RecordingSignals::default();
        for sample in 0..2_500 {
            backend.observe(&frame(
                Some("take"),
                sample as f64 * 0.05,
                vec![("leader", vec![joint("gripper", Some(sample as f64))])],
            ));
        }
        backend.observe(&frame(None, 0.0, vec![]));
        backend.observe(&frame(None, 0.0, vec![]));
        for _ in 0..3 {
            let window = backend.window(frontend.cursor());
            assert!(window.0.times.len() <= 1_000);
            frontend.apply_window(window);
        }
        assert_eq!(frontend, backend);
        let unchanged = backend.window(frontend.cursor());
        assert!(!unchanged.changes(&frontend));
        assert!(!frontend.live());
    }

    #[test]
    fn cursor_gaps_and_new_project_generations_replace_the_old_chart() {
        let mut backend = RecordingSignals::default();
        let mut frontend = RecordingSignals::default();
        backend.observe(&frame(Some("take"), 0.0, vec![]));
        frontend.apply_window(backend.window(frontend.cursor()));
        for sample in 1..=MAX_SAMPLES {
            backend.observe(&frame(Some("take"), sample as f64 * 0.05, vec![]));
        }
        frontend.apply_window(backend.window(frontend.cursor()));
        assert_eq!(frontend.times().first(), backend.times().first());
        assert_eq!(frontend.offset, backend.offset);
        backend.clear();
        frontend.apply_window(backend.window(frontend.cursor()));
        assert!(frontend.times().is_empty());
        assert!(!frontend.live());
        backend.observe(&frame(Some("take"), 0.0, vec![]));
        frontend.apply_window(backend.window(frontend.cursor()));
        assert_eq!(frontend.times(), &[0.0]);
    }
}
