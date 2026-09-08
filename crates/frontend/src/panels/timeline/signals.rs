//! Presentation of the fetched recording window.
pub(crate) use spiral_robotics_common::recording_signals::RecordingSignals as SignalStore;
use spiral_robotics_viewport as arm;
const SIGNAL_COLORS: [&str; 6] = [
    "#2a78d6", // shoulder pan
    "#eb6834", // shoulder lift
    "#1baf7a", // elbow flex
    "#eda100", // wrist flex
    "#e87ba4", // wrist roll
    "#008300", // gripper
];

pub(crate) trait SignalNamePresentation {
    /// Stable joint colour keyed by the canonical SO-101 joint name.
    fn signal_color(&self) -> &'static str;
}

impl SignalNamePresentation for str {
    fn signal_color(&self) -> &'static str {
        let slot = arm::SO101_JOINTS
            .iter()
            .position(|joint| joint.name() == self)
            .unwrap_or_else(|| {
                self.bytes().fold(0usize, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(usize::from(byte))
                }) % SIGNAL_COLORS.len()
            });
        SIGNAL_COLORS[slot % SIGNAL_COLORS.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn joint_colours_are_stable_when_a_motor_is_missing() {
        assert_eq!("shoulder_pan".signal_color(), SIGNAL_COLORS[0]);
        assert_eq!("elbow_flex".signal_color(), SIGNAL_COLORS[2]);
        assert_eq!("gripper".signal_color(), SIGNAL_COLORS[5]);
        assert_eq!("extra_axis".signal_color(), "extra_axis".signal_color());
    }
}
