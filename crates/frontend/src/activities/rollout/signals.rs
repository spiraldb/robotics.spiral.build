use crate::components::time_series::{
    TimelineSignalLane, TimelineSignalPoint, TimelineSignalSeries,
};
use spiral_robotics_viewport::SO101_JOINTS;
// Presentation of the fetched recording window.
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

/// Convert the studio's existing episode signal store into the timeline's fixed
/// six-lane view model. Each lane overlays the arms that reported that joint.
pub(crate) trait RecordingTimeline {
    fn timeline_lanes(&self) -> [TimelineSignalLane; 6];
}

impl RecordingTimeline for SignalStore {
    fn timeline_lanes(&self) -> [TimelineSignalLane; 6] {
        std::array::from_fn(|index| {
            let definition = SO101_JOINTS[index];
            let series = self
                .arms()
                .iter()
                .filter_map(|arm| {
                    let joint = arm
                        .joints()
                        .iter()
                        .find(|joint| joint.name() == definition.name())?;
                    let points = self
                        .times()
                        .iter()
                        .zip(joint.values())
                        .map(|(seconds, value)| {
                            TimelineSignalPoint::new(f64::from(*seconds), value.map(f64::from))
                        })
                        .collect();
                    Some(TimelineSignalSeries::new(arm.id(), points))
                })
                .collect();

            TimelineSignalLane::new(
                definition.name(),
                definition.label(),
                definition.name().signal_color(),
            )
            .with_series(series)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::time_series::TimelineSignalLanes;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn joint_colours_are_stable_when_a_motor_is_missing() {
        assert_eq!("shoulder_pan".signal_color(), SIGNAL_COLORS[0]);
        assert_eq!("elbow_flex".signal_color(), SIGNAL_COLORS[2]);
        assert_eq!("gripper".signal_color(), SIGNAL_COLORS[5]);
        assert_eq!("extra_axis".signal_color(), "extra_axis".signal_color());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn timeline_chart_uses_one_grid_per_canonical_lane() {
        let chart = SignalStore::default()
            .timeline_lanes()
            .chart_with(&crate::components::time_series::TraceChartLayout::sparkline(12.0));
        let option = serde_json::to_value(chart).expect("timeline chart should serialize");

        assert_eq!(option["grid"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["xAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["yAxis"].as_array().map(Vec::len), Some(6));
        assert_eq!(option["animation"], false);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn canonical_lane_helper_always_returns_six_joint_slots() {
        let lanes = SignalStore::default().timeline_lanes();
        assert_eq!(lanes.len(), 6);
        assert_eq!(lanes[0].id(), "shoulder_pan");
        assert_eq!(lanes[5].id(), "gripper");
    }
}
