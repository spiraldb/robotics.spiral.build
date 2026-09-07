//! Episode catalog reads and inspection snapshots.

use crate::OperationError;
use crate::project_session::context::ProjectContext;
use crate::robots::workspace::RobotRecord;
use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::{
    EpisodeArmScalarSamples, EpisodeInspectionArm, EpisodeInspectionSnapshot, EpisodeListItem,
    EpisodeOptionalFrameMatrix, EpisodeVideoAssetMetadata, EpisodeVideoFormat,
    EpisodeVideoIdentity, EpisodeVideoTimeline,
};
use spiral_robotics_storage::episodes::{EpisodeId, EpisodeInspection};

pub(crate) struct EpisodeQueries<'a> {
    state: &'a ProjectContext,
}

impl<'a> EpisodeQueries<'a> {
    pub(crate) fn new(state: &'a ProjectContext) -> Self {
        Self { state }
    }

    pub(crate) async fn authorize_asset(&self, episode: String) -> Result<(), OperationError> {
        let episode = Self::episode_id(&episode)?;
        self.state
            .episodes()
            .get(episode.as_str())
            .await
            .map(|_| ())
            .map_err(OperationError::from)
    }

    pub(crate) async fn list(
        &self,
        dataset_id: String,
    ) -> Result<Vec<EpisodeListItem>, OperationError> {
        let dataset_id = Self::dataset_id(&dataset_id)?;
        Ok(self
            .state
            .episodes()
            .list_in_dataset(dataset_id.as_str())
            .await)
    }

    pub(crate) async fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let episode = Self::episode_id(&episode)?;
        let (inspection, saved_robots) = tokio::join!(
            self.state.episodes().inspection(episode.as_str()),
            self.state.workspace().list(),
        );
        Ok(inspection
            .map_err(OperationError::from)?
            .into_snapshot(&saved_robots))
    }

    fn episode_id(episode: &str) -> Result<String, OperationError> {
        EpisodeId::try_from(episode).map(String::from).map_err(|_| {
            OperationError::invalid_input(
                "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
            )
        })
    }

    fn dataset_id(dataset_id: &str) -> Result<String, OperationError> {
        DatasetId::try_from(dataset_id)
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "a dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'",
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
    fn into_snapshot(self, saved_robots: &[RobotRecord]) -> EpisodeInspectionSnapshot;
}

impl InspectionProjection for EpisodeInspection {
    fn into_snapshot(self, saved_robots: &[RobotRecord]) -> EpisodeInspectionSnapshot {
        let inspection = self;
        let (episode, summary, videos) = inspection.into_parts();
        let samples = episode.samples();
        let arms = episode
            .start()
            .arms()
            .iter()
            .enumerate()
            .map(|(arm_index, arm)| {
                let joint_count = arm.joints().len();
                let calibration = saved_robots
                    .iter()
                    .find(|robot| robot.id() == arm.id())
                    .cloned()
                    .and_then(|record| record.calibration().ok().flatten());
                let normalized = samples
                    .iter()
                    .flat_map(|sample| sample.normalized()[arm_index].iter().copied())
                    .collect();
                let raw = samples
                    .iter()
                    .flat_map(|sample| sample.raw()[arm_index].iter().copied())
                    .collect();
                let degrees = samples
                    .iter()
                    .flat_map(|sample| {
                        sample.raw()[arm_index]
                            .iter()
                            .zip(arm.joints())
                            .map(|(raw, joint)| {
                                calibration
                                    .as_ref()
                                    .and_then(|calibration| calibration.motors().get(joint))
                                    .map(|motor| motor.degrees_from_center(*raw))
                            })
                    })
                    .collect();
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
        .with_videos(
            videos
                .into_iter()
                .map(|video| {
                    let size = video.size();
                    let timing = video.timing();
                    let segment = video.segment();
                    EpisodeVideoAssetMetadata::new(
                        EpisodeVideoIdentity::new(
                            video.stream_id(),
                            video.camera_id(),
                            segment.index(),
                        ),
                        EpisodeVideoFormat::new(size.width(), size.height(), timing.fps()),
                        EpisodeVideoTimeline::new(
                            timing.frames(),
                            video.duration_seconds(),
                            segment.timeline_offset_seconds(),
                        ),
                    )
                })
                .collect(),
        )
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
    fn catalog_reads_normalize_datasets_and_require_exact_episode_identities() {
        assert_eq!(
            EpisodeQueries::dataset_id(" pick-place ").unwrap().as_str(),
            "pick-place"
        );
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
            assert!(matches!(
                EpisodeQueries::dataset_id(invalid),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }
}
