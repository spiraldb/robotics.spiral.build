//! Opening, pausing, and closing the one take a robot connector records: each
//! consequential step in order, with the rollback its failure owes.
use std::collections::BTreeSet;

use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::{ControlSource, EpisodeArm, EpisodeInfo, RecordingStart};
use spiral_robotics_hardware::{CaptureClock, HardwareState, ModeSelection};
use spiral_robotics_recording::{
    ArmCondition, EpisodeArmCollection as _, EpisodeRecordingRequest, EpisodeWriter, McapRecording,
};

use super::session::{OpenTake, TakeChange};
use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::robotics::video::{
    CameraCaptureSpec, CameraRecording, VideoCapture, VideoCaptureRequest,
};

/// What a new take records: the dataset it joins, what drives its arms, and
/// whether their cameras too.
pub(super) struct TakeRequest {
    dataset_id: String,
    control: ControlSource,
    video: bool,
}

impl From<RecordingStart> for TakeRequest {
    fn from(request: RecordingStart) -> Self {
        let (dataset_id, control, video) = request.into_parts();
        Self {
            dataset_id,
            control,
            video,
        }
    }
}

impl TakeRequest {
    fn validated(self) -> Result<Self, OperationError> {
        let dataset_id = DatasetId::try_from(self.dataset_id.as_str())
            .map(String::from)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let ControlSource::Teleoperate { leader, follower } = &self.control;
        if leader.is_empty() || follower.is_empty() || leader == follower {
            return Err(OperationError::invalid_input(
                "Recording needs distinct leader and follower identities",
            ));
        }
        Ok(Self { dataset_id, ..self })
    }

    /// The arms the take records: the pair a teleoperation moves.
    fn arms(&self) -> Vec<String> {
        let ControlSource::Teleoperate { leader, follower } = &self.control;
        vec![leader.clone(), follower.clone()]
    }

    /// Where the take came from, as its episode says it.
    fn source(&self) -> String {
        let ControlSource::Teleoperate { leader, follower } = &self.control;
        format!("Teleoperation — {leader} → {follower}")
    }
}

/// A take's episode writer before the control loop accepts it.
struct TakeDraft {
    writer: EpisodeWriter,
    arms: Vec<EpisodeArm>,
    summary: EpisodeInfo,
}

pub(crate) struct TakeOperations<'a> {
    state: &'a RobotConnectorContext,
}

impl<'a> TakeOperations<'a> {
    pub(crate) fn new(state: &'a RobotConnectorContext) -> Self {
        Self { state }
    }

    /// Start the request's control, then open one take, and close it whenever
    /// the control loop ends it on its own. Once the control source runs, any
    /// failure stops it again; a failed rollback is reported, because an
    /// unnoticed live control source is safety-relevant.
    pub(super) async fn start(&self, request: TakeRequest) -> Result<EpisodeInfo, OperationError> {
        let mut take = self.state.recording_session().try_change()?;
        if take.is_open() || self.state.hardware().state().recording().is_some() {
            return Err(OperationError::conflict(
                "a take is already recording; stop it first",
            ));
        }
        let request = request.validated()?;
        let sample_hz = self.admitted_sample_hz(&request)?;
        self.start_control(&request.control).await?;
        match self.open(&request, sample_hz).await {
            Ok((summary, capture)) => {
                take.open(OpenTake::new(summary.id().to_owned(), capture));
                let (state, episode) = (self.state.clone(), summary.id().to_owned());
                tokio::spawn(async move {
                    TakeOperations::new(&state)
                        .close_when_the_loop_ends(&episode)
                        .await;
                });
                Ok(summary)
            }
            Err(error) => match self.stop_control().await {
                Ok(()) => Err(error),
                Err(rollback) => Err(OperationError::Conflict(format!(
                    "{error} Teleoperation also could not stop: {rollback}"
                ))),
            },
        }
    }

    /// Stop the take and its control source whatever happens, then save its media.
    /// The take is held throughout, so the control loop ending it on its own
    /// cannot race this stop.
    pub(super) async fn stop(&self, keep: bool) -> Result<EpisodeInfo, OperationError> {
        let take = self.state.recording_session().try_change()?;
        if !take.is_open() {
            return Err(OperationError::conflict(
                "no recording is available to stop",
            ));
        }
        let recording = self.stopped_episode(keep).await;
        let stopped = self.stop_control().await;
        self.close(take).await?;
        match (recording, stopped) {
            (Ok(summary), Ok(())) => Ok(summary),
            (Ok(_), Err(control_error)) => Err(OperationError::Conflict(format!(
                "the recording ended, but teleoperation could not stop: {control_error}"
            ))),
            (Err(recording_error), Ok(())) => Err(recording_error),
            (Err(recording_error), Err(control_error)) => Err(OperationError::Conflict(format!(
                "{recording_error}; teleoperation also could not stop: {control_error}"
            ))),
        }
    }

    /// Pause joint and media capture in a fail-safe order, or resume both.
    pub(super) async fn set_paused(&self, paused: bool) -> Result<(), OperationError> {
        let mut take = self.state.recording_session().try_change()?;
        if !take.is_open() {
            return Err(OperationError::conflict(
                "recording is not available for pause",
            ));
        }
        let hardware = self.state.hardware();
        if paused {
            if let Some(capture) = take.capture() {
                capture.request_pause();
            }
            let paused = hardware.set_recording_paused(true).await;
            self.finish_media_segment(&mut take).await;
            paused.map(|_| ()).map_err(OperationError::conflict)
        } else {
            // A cancelled pause may have left encoder work behind. Drain it
            // before hardware and camera clocks resume.
            self.finish_media_segment(&mut take).await;
            let active_seconds = hardware
                .set_recording_paused(false)
                .await
                .map_err(OperationError::conflict)?;
            if let Some(capture) = take.capture()
                && let Err(error) = capture.resume(active_seconds)
            {
                self.state
                    .cameras()
                    .note_issue(format!("episode video could not resume: {error}"));
            }
            Ok(())
        }
    }

    /// Close `episode` when the control loop ends it on its own — a robot
    /// detaches, or the writer fails. An explicit stop closes it first, which
    /// makes this a no-op.
    async fn close_when_the_loop_ends(&self, episode: &str) {
        if self.control_loop_ended(episode).await {
            self.close_ended(Some(episode)).await;
        }
    }

    /// Whether the control loop ended `episode`; false if it stopped
    /// publishing first.
    async fn control_loop_ended(&self, episode: &str) -> bool {
        let shown = |state: &HardwareState| {
            state
                .recording()
                .is_some_and(|recording| recording.episode() == episode)
        };
        let mut hardware = self.state.hardware().subscribe();
        // The loop publishes an accepted take at the end of its cycle, so the
        // first publish after this subscription shows it unless it has ended.
        if !shown(&hardware.borrow_and_update()) && hardware.changed().await.is_err() {
            return false;
        }
        hardware.wait_for(|state| !shown(state)).await.is_ok()
    }

    /// Commit a take the control loop has already ended: `episode`, or at
    /// shutdown whichever take is open.
    pub(crate) async fn close_ended(&self, episode: Option<&str>) {
        let take = self.state.recording_session().change().await;
        if episode.is_some_and(|episode| take.episode() != Some(episode)) {
            return;
        }
        if let Err(error) = self.close(take).await {
            tracing::error!("could not save automatically stopped episode media: {error}");
            self.state
                .cameras()
                .note_issue(format!("episode media could not be saved: {error}"));
        }
    }

    /// The control loop's sample rate, once both arms are connected.
    fn admitted_sample_hz(&self, request: &TakeRequest) -> Result<f64, OperationError> {
        let hardware = self.state.hardware().state();
        let sample_hz = if hardware.target_hz() > 0.0 {
            (hardware.target_hz() * 1_000.0).round() / 1_000.0
        } else {
            spiral_robotics_hardware::CONTROL_HZ as f64
        };
        if !sample_hz.is_finite() || sample_hz <= 0.0 {
            return Err(OperationError::Internal(
                "the control loop reported an invalid sample rate".to_owned(),
            ));
        }
        for arm in request.arms() {
            if !hardware
                .connections()
                .get(&arm)
                .is_some_and(|connection| connection.connected())
            {
                return Err(OperationError::InvalidInput(format!(
                    "robot {arm:?} is not connected, so it cannot be recorded"
                )));
            }
        }
        Ok(sample_hz)
    }

    /// Start `control`: the follower mirrors the leader.
    async fn start_control(&self, control: &ControlSource) -> Result<(), OperationError> {
        let ControlSource::Teleoperate { leader, follower } = control;
        self.set_mode(ModeSelection::Teleoperate {
            leader: leader.clone(),
            follower: follower.clone(),
        })
        .await
    }

    /// Stop the control source: leave teleoperation for idle.
    async fn stop_control(&self) -> Result<(), OperationError> {
        self.set_mode(ModeSelection::Idle).await
    }

    async fn set_mode(&self, mode: ModeSelection) -> Result<(), OperationError> {
        self.state
            .hardware()
            .set_mode(mode)
            .await
            .map_err(OperationError::conflict)
    }

    /// Draft the episode, start its cameras, and hand the take to the control
    /// loop. Cameras started for a take the loop refuses are finished again.
    async fn open(
        &self,
        request: &TakeRequest,
        sample_hz: f64,
    ) -> Result<(EpisodeInfo, Option<VideoCapture>), OperationError> {
        let draft = self.draft(request, sample_hz).await?;
        let capture = if request.video {
            self.capture(draft.summary.id(), &request.arms(), draft.writer.clock())
                .await?
        } else {
            None
        };
        if let Err(error) = self
            .state
            .hardware()
            .start_recording(draft.writer, draft.arms)
            .await
        {
            if let Some(capture) = capture {
                let _ = capture.finish().await;
            }
            return Err(OperationError::conflict(error));
        }
        Ok((draft.summary, capture))
    }

    async fn stopped_episode(&self, keep: bool) -> Result<EpisodeInfo, OperationError> {
        let completion = self
            .state
            .hardware()
            .stop_recording(keep)
            .await
            .map_err(OperationError::conflict)?;
        self.state
            .recording_buffer()
            .completed_episode(completion.episode())
            .map(|episode| episode.summary())
            .map_err(OperationError::from)
    }

    /// Save the take's episode and media as its recording file, then keep its
    /// media for review. Media stays reviewable when saving fails.
    async fn close(&self, mut take: TakeChange<'_>) -> Result<(), OperationError> {
        let Some(episode) = take.episode().map(str::to_owned) else {
            return Ok(());
        };
        self.finish_media_segment(&mut take).await;
        let cameras = take
            .capture()
            .map(|capture| capture.completed_assets())
            .unwrap_or_default();
        let saved = self.save(&episode, &cameras);
        take.close(cameras);
        saved
    }

    fn save(&self, episode: &str, cameras: &[CameraRecording]) -> Result<(), OperationError> {
        let episode = self.state.recording_buffer().completed_episode(episode)?;
        let summary = episode.summary();
        let bytes = McapRecording::encode(&episode, cameras).map_err(OperationError::Internal)?;
        self.state
            .recording_files()
            .save(summary.dataset_id(), summary.id(), &bytes)
            .map_err(OperationError::Internal)
    }

    async fn finish_media_segment(&self, take: &mut TakeChange<'_>) {
        if let Some(capture) = take.capture() {
            for failure in capture.finish_pause().await {
                self.state.cameras().note_issue(failure);
            }
        }
    }

    /// Draft the episode from live telemetry: the arms' joints and the
    /// motors' condition as the take opens.
    async fn draft(
        &self,
        request: &TakeRequest,
        sample_hz: f64,
    ) -> Result<TakeDraft, OperationError> {
        let hardware = self.state.hardware().state();
        let connected = hardware
            .robots()
            .iter()
            .filter(|robot| {
                hardware
                    .connections()
                    .get(robot.id())
                    .is_some_and(|connection| connection.connected())
            })
            .collect::<Vec<_>>();
        let joints = request
            .arms()
            .into_iter()
            .map(|id| {
                let robot = connected
                    .iter()
                    .find(|robot| robot.id() == id)
                    .ok_or_else(|| {
                        OperationError::Conflict(format!(
                            "robot {id:?} disconnected while the recording was opening"
                        ))
                    })?;
                let joints = robot
                    .joints()
                    .iter()
                    .map(|joint| joint.name().to_owned())
                    .collect::<Vec<_>>();
                Ok((id, joints))
            })
            .collect::<Result<Vec<_>, OperationError>>()?;
        let saved_robots = self.state.workspace().list().await;
        let arms = EpisodeArm::from_joint_lists(&joints)
            .map_err(OperationError::from)?
            .into_iter()
            .map(|arm| {
                let robot_type = saved_robots
                    .iter()
                    .find(|robot| robot.id() == arm.id())
                    .and_then(|robot| robot.config().robot_type())
                    .map(str::to_owned);
                EpisodeArm::new(arm.id(), robot_type, arm.joints().to_vec())
            })
            .collect::<Vec<_>>();

        // What the motors were like as the take opened. Recorded once here rather
        // than on every frame: these move over seconds, and the footer's extremes
        // close the story the header starts.
        let condition = arms
            .iter()
            .filter_map(|arm| {
                let robot = connected.iter().find(|robot| robot.id() == arm.id())?;
                let limits = hardware.limits().get(arm.id());
                let joint = |name: &str| robot.joints().iter().find(|joint| joint.name() == name);
                Some(
                    ArmCondition::new(arm.id().to_owned())
                        .with_measurements(
                            arm.joints()
                                .iter()
                                .map(|name| {
                                    joint(name).and_then(|joint| joint.temperature_celsius())
                                })
                                .collect(),
                            arm.joints()
                                .iter()
                                .map(|name| joint(name).and_then(|joint| joint.voltage_volts()))
                                .collect(),
                        )
                        .with_limits(
                            arm.joints()
                                .iter()
                                .map(|name| {
                                    limits
                                        .and_then(|limits| limits.get(name))
                                        .copied()
                                        .unwrap_or_default()
                                })
                                .collect(),
                        ),
                )
            })
            .collect();

        let writer = self
            .state
            .recording_buffer()
            .begin(
                EpisodeRecordingRequest::new(
                    request.dataset_id.clone(),
                    request.source(),
                    sample_hz,
                )
                .with_arms(arms.clone(), condition),
            )
            .map_err(OperationError::from)?;
        let summary = EpisodeInfo::new(writer.id().to_owned(), request.dataset_id.clone())
            .with_arms(arms.clone());
        Ok(TakeDraft {
            writer,
            arms,
            summary,
        })
    }

    /// Start the cameras bound to the take's robots.
    async fn capture(
        &self,
        episode: &str,
        arms: &[String],
        clock: CaptureClock,
    ) -> Result<Option<VideoCapture>, OperationError> {
        let records = self
            .state
            .workspace()
            .try_list()
            .await
            .map_err(OperationError::from)?;
        let statuses = self.state.cameras().statuses();
        let camera_status = statuses
            .iter()
            .map(|camera| (camera.id(), camera))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut seen = BTreeSet::new();
        let mut cameras = Vec::new();
        for robot in records
            .iter()
            .filter(|robot| arms.iter().any(|id| id == robot.id()))
        {
            for (role, camera) in robot.config().cameras() {
                if !seen.insert(camera.id().to_owned()) {
                    continue;
                }
                let Some(status) = camera_status.get(camera.id()) else {
                    continue;
                };
                cameras.push(CameraCaptureSpec::new(
                    format!("{}.camera.{role}", robot.id()),
                    camera.id(),
                    (status.width(), status.height(), status.fps()),
                ));
            }
        }
        VideoCapture::start(
            self.state.cameras().clone(),
            VideoCaptureRequest::new(episode, cameras, clock),
            self.state.encoder_driver(),
        )
        .await
        .map_err(|error| OperationError::InvalidInput(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::recording::{ControlSource, RecordingStart};

    use super::TakeRequest;

    fn request(control: ControlSource) -> TakeRequest {
        TakeRequest::from(RecordingStart::new("pick-place".to_owned(), control, true))
    }

    #[test]
    fn a_teleoperation_take_records_both_arms_it_pairs() {
        let take = request(ControlSource::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        })
        .validated()
        .unwrap();
        assert_eq!(take.arms(), ["leader", "follower"]);
        assert_eq!(take.source(), "Teleoperation — leader → follower");

        let same_arm = request(ControlSource::Teleoperate {
            leader: "follower".to_owned(),
            follower: "follower".to_owned(),
        });
        assert!(same_arm.validated().is_err());
    }
}
