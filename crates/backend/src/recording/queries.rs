//! Project the current recording buffer for Live take review.

use crate::OperationError;
use crate::projects::context::ProjectContext;
use spiral_robotics_common::recording::{
    EpisodeArmScalarSamples, EpisodeInspectionArm, EpisodeInspectionSnapshot,
    EpisodeOptionalFrameMatrix,
};
use spiral_robotics_storage::episodes::{EpisodeId, EpisodeInspection};

pub(crate) struct EpisodeQueries<'a> {
    state: &'a ProjectContext,
}

impl<'a> EpisodeQueries<'a> {
    pub(super) fn new(state: &'a ProjectContext) -> Self {
        Self { state }
    }

    pub(super) fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let episode = Self::episode_id(&episode)?;
        self.state
            .recording_buffer()
            .inspection(episode.as_str())
            .map_err(OperationError::from)
            .map(InspectionProjection::into_snapshot)
    }

    fn episode_id(episode: &str) -> Result<String, OperationError> {
        EpisodeId::try_from(episode).map(String::from).map_err(|_| {
            OperationError::invalid_input(
                "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
            )
        })
    }
}

/// Projects a stored inspection into the wire snapshot.
///
/// Both types are foreign to this crate, so the row-major to frame-major
/// transposition hangs off the stored value as a local extension trait rather
/// than a `From` impl.
trait InspectionProjection {
    fn into_snapshot(self) -> EpisodeInspectionSnapshot;
}

impl InspectionProjection for EpisodeInspection {
    fn into_snapshot(self) -> EpisodeInspectionSnapshot {
        let inspection = self;
        let (episode, summary) = inspection.into_parts();
        let samples = episode.samples();
        let arms = episode
            .start()
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let joint_count = arm.joints().len();
                let normalized = samples
                    .iter()
                    .flat_map(|sample| sample.normalized()[arm_index].iter().copied())
                    .collect();
                let raw = samples
                    .iter()
                    .flat_map(|sample| sample.raw()[arm_index].iter().copied())
                    .collect();
                // The MCAP preserves normalized positions and raw encoder readings.
                // Never reinterpret historical samples with today's calibration.
                let degrees = vec![None; samples.len() * joint_count];
                let velocity = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .velocity()
                        .and_then(|arms| arms.get(index))
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                let effort = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .effort()
                        .and_then(|arms| arms.get(index))
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                let action = OptionalFrameMatrix::new(samples, joint_count, |sample, index| {
                    sample
                        .action()
                        .get(index)
                        .and_then(Option::as_ref)
                        .map(Vec::as_slice)
                })
                .for_arm(arm_index);
                EpisodeInspectionArm::new(
                    arm.id(),
                    arm.joints().to_vec(),
                    EpisodeArmScalarSamples::new(normalized, raw).with_degrees(degrees),
                )
                .with_velocity(velocity)
                .with_effort(effort)
                .with_action(action)
            })
            .collect();
        EpisodeInspectionSnapshot::new(
            summary,
            samples.iter().map(|sample| sample.seconds()).collect(),
            samples
                .iter()
                .map(|sample| sample.follow_error_degrees())
                .collect(),
        )
        .with_arms(arms)
    }
}

struct OptionalFrameMatrix<'a, F> {
    samples: &'a [spiral_robotics_storage::episodes::Sample],
    joint_count: usize,
    values_at: F,
}

impl<'samples, F> OptionalFrameMatrix<'samples, F>
where
    F: for<'sample> Fn(
        &'sample spiral_robotics_storage::episodes::Sample,
        usize,
    ) -> Option<&'sample [Option<f64>]>,
{
    fn new(
        samples: &'samples [spiral_robotics_storage::episodes::Sample],
        joint_count: usize,
        values_at: F,
    ) -> Self {
        Self {
            samples,
            joint_count,
            values_at,
        }
    }

    fn for_arm(self, arm_index: usize) -> EpisodeOptionalFrameMatrix {
        let mut present = Vec::with_capacity(self.samples.len());
        let mut values = Vec::with_capacity(self.samples.len().saturating_mul(self.joint_count));
        for sample in self.samples {
            match (self.values_at)(sample, arm_index) {
                Some(frame) => {
                    debug_assert_eq!(frame.len(), self.joint_count);
                    present.push(true);
                    values.extend(frame.iter().copied());
                }
                None => {
                    present.push(false);
                    values.extend(std::iter::repeat_n(None, self.joint_count));
                }
            }
        }
        EpisodeOptionalFrameMatrix::new(present, values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_requires_exact_episode_identities() {
        assert_eq!(
            EpisodeQueries::episode_id("episode_000001")
                .unwrap()
                .as_str(),
            "episode_000001"
        );
        assert!(matches!(
            EpisodeQueries::episode_id(" episode"),
            Err(OperationError::InvalidInput(_))
        ));
        for invalid in ["", "../episode", "episode name", "episode/"] {
            assert!(matches!(
                EpisodeQueries::episode_id(invalid),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }
}
