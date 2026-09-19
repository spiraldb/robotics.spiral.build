//! Recording and replay capability actions.
//!
//! The ports in this module describe consequences, not implementation tools:
//! an HTTP adapter may use SpiralDB, cameras, and a control-thread actor,
//! while another delivery mechanism can provide different adapters without
//! changing session ordering, validation, or rollback policy.

pub(crate) mod commands;
pub(crate) mod http;
pub(crate) mod supervisor;

use std::collections::BTreeMap;

use spiral_robotics_common::identity::DatasetId;

use crate::OperationError;
use crate::data::catalog_lease::CatalogLeaseToken;
use crate::recording::episode_arms::RecordedArms;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartRecordingCommand {
    pair: TeleoperationPair,
    dataset_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeleoperationPair {
    leader: String,
    follower: String,
}

/// Live facts needed to admit one new take.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordingContext {
    connected_arms: Vec<String>,
    sample_hz: f64,
}

/// Consequences needed by the recording-session use cases.
///
/// `MediaCapture` and `MediaOutput` remain opaque so video libraries and media
/// formats cannot leak into capability policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordingResult {
    episode: String,
}

impl StartRecordingCommand {
    fn new(dataset_id: impl Into<String>, pair: TeleoperationPair) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            pair,
        }
    }
}

impl TeleoperationPair {
    fn new(leader: impl Into<String>, follower: impl Into<String>) -> Self {
        Self {
            leader: leader.into(),
            follower: follower.into(),
        }
    }
    fn leader(&self) -> &str {
        &self.leader
    }
    fn follower(&self) -> &str {
        &self.follower
    }
}

impl RecordingContext {
    fn new(connected_arms: Vec<String>, sample_hz: f64) -> Self {
        Self {
            connected_arms,
            sample_hz,
        }
    }
}

impl RecordingResult {
    fn new(episode: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
        }
    }
    fn episode(&self) -> &str {
        &self.episode
    }
}

pub(crate) struct RecordingDraftRequest {
    dataset_id: String,
    source: String,
    arms: Vec<String>,
    sample_hz: f64,
}

impl RecordingDraftRequest {
    fn new(dataset_id: String, source: String) -> Self {
        Self {
            dataset_id,
            source,
            arms: Vec::new(),
            sample_hz: 0.0,
        }
    }

    fn with_capture(mut self, arms: Vec<String>, sample_hz: f64) -> Self {
        self.arms = arms;
        self.sample_hz = sample_hz;
        self
    }

    fn into_parts(self) -> (String, String, Vec<String>, f64) {
        (self.dataset_id, self.source, self.arms, self.sample_hz)
    }
}

pub(crate) trait RecordingPort {
    type RecordingDraft;
    type MediaCapture;
    type MediaOutput;
    type Finalization;

    type CaptureChange;

    /// Atomically claim the one recording-start transaction.
    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError>;
    fn bind_recording_lease(
        &self,
        token: CatalogLeaseToken,
        take: (String, Option<Self::MediaCapture>),
    );
    fn claim_finalization(&self) -> Result<Option<Self::Finalization>, OperationError>;
    fn finalization_episode(&self, operation: &Self::Finalization) -> String;
    fn complete_recording(&self, operation: Self::Finalization, media: Self::MediaOutput);
    fn release_recording_lease(&self, token: CatalogLeaseToken);
    fn recording_active(&self) -> bool;
    fn recording_context(&self) -> RecordingContext;

    async fn enter_teleoperation(&self, pair: &TeleoperationPair) -> Result<(), OperationError>;
    async fn enter_idle(&self) -> Result<(), OperationError>;
    async fn create_recording_draft(
        &self,
        request: RecordingDraftRequest,
    ) -> Result<Self::RecordingDraft, OperationError>;
    fn draft_episode(&self, draft: &Self::RecordingDraft) -> String;
    async fn start_recording_media(
        &self,
        episode: &str,
        arms: &[String],
    ) -> Result<Option<Self::MediaCapture>, OperationError>;
    async fn begin_recording(
        &self,
        draft: Self::RecordingDraft,
    ) -> Result<RecordingResult, OperationError>;
    async fn abandon_recording_media(&self, capture: Self::MediaCapture);

    async fn stop_recording(&self, keep: bool) -> Result<RecordingResult, OperationError>;
    async fn finish_recording_media(&self, operation: &mut Self::Finalization)
    -> Self::MediaOutput;
    async fn commit_recording(
        &self,
        episode: String,
        media: &Self::MediaOutput,
    ) -> Result<(), OperationError>;

    fn claim_capture_change(&self) -> Result<Self::CaptureChange, OperationError>;
    fn request_media_pause(&self, operation: &Self::CaptureChange);
    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError>;
    async fn finish_media_pause(
        &self,
        operation: &mut Self::CaptureChange,
    ) -> Result<(), OperationError>;
    fn resume_media(&self, operation: &mut Self::CaptureChange, active_seconds: f64);
}

struct RecordingStartReservation<'a, P: RecordingPort + ?Sized> {
    port: &'a P,
    token: Option<CatalogLeaseToken>,
}

impl<'a, P: RecordingPort + ?Sized> RecordingStartReservation<'a, P> {
    fn new(port: &'a P, token: CatalogLeaseToken) -> Self {
        Self {
            port,
            token: Some(token),
        }
    }

    /// A successfully opened recording owns the shared catalog lease until its
    /// final episode and media rows have been committed.
    fn retain_for_recording(&mut self, take: (String, Option<P::MediaCapture>)) {
        let token = self.token.take().expect("recording reservation is live");
        self.port.bind_recording_lease(token, take);
    }
}

impl<P: RecordingPort + ?Sized> Drop for RecordingStartReservation<'_, P> {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            self.port.release_recording_lease(token);
        }
    }
}

pub(crate) struct RecordingActions<'a, P: RecordingPort + ?Sized> {
    port: &'a P,
}

impl<'a, P: RecordingPort + ?Sized> RecordingActions<'a, P> {
    const fn new(port: &'a P) -> Self {
        Self { port }
    }

    /// Enter the configured teleoperation session, then open one take.
    ///
    /// If any edge assembly fails after the mode transition, returning to idle is
    /// mandatory compensation. A failed rollback is retained in the operator
    /// error because an unnoticed live teleoperation session is safety-relevant.
    async fn start(
        &self,
        command: StartRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let Some(token) = self.port.reserve_recording_start()? else {
            return Err(OperationError::Conflict(
                "another recording is already opening; wait for it to finish".to_owned(),
            ));
        };
        let mut reservation = RecordingStartReservation::new(self.port, token);
        let (recording, capture) = self.start_reserved(command).await?;
        reservation.retain_for_recording((recording.episode.clone(), capture));
        Ok(recording)
    }

    async fn start_reserved(
        &self,
        mut command: StartRecordingCommand,
    ) -> Result<(RecordingResult, Option<P::MediaCapture>), OperationError> {
        let port = self.port;
        if port.recording_active() {
            return Err(OperationError::Conflict(
                "a take is already recording; stop it first".to_owned(),
            ));
        }
        command.dataset_id = DatasetId::try_from(command.dataset_id.as_str())
            .map(String::from)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        let pair = command.pair.clone();
        if pair.leader.is_empty() || pair.follower.is_empty() || pair.leader == pair.follower {
            return Err(OperationError::invalid_input(
                "Recording needs distinct leader and follower identities",
            ));
        }
        let context = port.recording_context();
        if !context.sample_hz.is_finite() || context.sample_hz <= 0.0 {
            return Err(OperationError::Internal(
                "the control loop reported an invalid sample rate".to_owned(),
            ));
        }
        let arms = vec![pair.leader.clone(), pair.follower.clone()];
        for arm in &arms {
            if !context
                .connected_arms
                .iter()
                .any(|connected| connected == arm)
            {
                return Err(OperationError::InvalidInput(format!(
                    "robot {arm:?} is not connected, so it cannot be recorded"
                )));
            }
        }
        let source = format!("Teleoperation — {} → {}", pair.leader, pair.follower);

        port.enter_teleoperation(&pair).await?;
        let open = async {
            let request = RecordingDraftRequest::new(command.dataset_id, source)
                .with_capture(arms.clone(), context.sample_hz);
            let draft = port.create_recording_draft(request).await?;
            let episode = port.draft_episode(&draft);
            let media = port.start_recording_media(&episode, &arms).await?;
            match port.begin_recording(draft).await {
                Ok(recording) => Ok((recording, media)),
                Err(error) => {
                    if let Some(capture) = media {
                        port.abandon_recording_media(capture).await;
                    }
                    Err(error)
                }
            }
        }
        .await;
        match open {
            Ok(summary) => Ok(summary),
            Err(error) => match port.enter_idle().await {
                Ok(()) => Err(error),
                Err(rollback) => Err(OperationError::Conflict(format!(
                    "{error} Teleoperation also could not stop: {rollback}"
                ))),
            },
        }
    }

    /// Finish a take and independently attempt the safety transition to idle.
    ///
    /// Media is claimed before the hardware stop so an automatic-stop observer
    /// cannot race this explicit action. The idle transition is attempted even if
    /// finalizing the recording fails.
    async fn stop(&self, keep: bool) -> Result<RecordingResult, OperationError> {
        let port = self.port;
        let Some(mut operation) = port.claim_finalization()? else {
            return Err(OperationError::conflict(
                "no recording is available to stop",
            ));
        };
        let episode = port.finalization_episode(&operation);
        let recording = port.stop_recording(keep).await;
        let idle = port.enter_idle().await;
        let media = port.finish_recording_media(&mut operation).await;
        // The claim supplies identity even for camera-less takes if hardware stop
        // reports an error. Retain review media on publication errors as well.
        let committed = port.commit_recording(episode, &media).await;
        port.complete_recording(operation, media);
        committed?;
        match (recording, idle) {
            (Ok(summary), Ok(())) => Ok(summary),
            (Ok(_), Err(mode_error)) => Err(OperationError::Conflict(format!(
                "the recording ended, but teleoperation could not stop: {mode_error}"
            ))),
            (Err(recording_error), Ok(())) => Err(recording_error),
            (Err(recording_error), Err(mode_error)) => Err(OperationError::Conflict(format!(
                "{recording_error}; teleoperation also could not stop: {mode_error}"
            ))),
        }
    }

    /// Pause/resume joint and media capture in a fail-safe order.
    async fn set_paused(&self, paused: bool) -> Result<(), OperationError> {
        let mut operation = self.port.claim_capture_change()?;
        if paused {
            self.port.request_media_pause(&operation);
            let paused = self.port.set_recording_paused(true).await;
            let media = self.port.finish_media_pause(&mut operation).await;
            paused?;
            media?;
        } else {
            // A cancelled pause may have returned unfinished encoder work to the
            // session. Drain it before hardware and camera clocks resume.
            self.port.finish_media_pause(&mut operation).await?;
            let active_seconds = self.port.set_recording_paused(false).await?;
            self.port.resume_media(&mut operation, active_seconds);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StartReplayCommand {
    episode: String,
    robot: String,
    source_arm: Option<String>,
    start_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayEpisode {
    id: String,
    arms: Vec<String>,
    sample_hz: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayTrajectory {
    frames: Vec<BTreeMap<String, f64>>,
    start_frame: usize,
    resolved_start_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayPlan {
    robot: String,
    episode: String,
    frames: Vec<BTreeMap<String, f64>>,
    sample_hz: f64,
    start_frame: usize,
    resolved_start_seconds: f64,
}

impl StartReplayCommand {
    fn new(
        episode: impl Into<String>,
        robot: impl Into<String>,
        source_arm: Option<String>,
    ) -> Self {
        Self {
            episode: episode.into(),
            robot: robot.into(),
            source_arm,
            start_seconds: 0.0,
        }
    }
    fn starting_at(mut self, start_seconds: f64) -> Self {
        self.start_seconds = start_seconds;
        self
    }
}

impl ReplayEpisode {
    fn new(id: impl Into<String>, arms: Vec<String>, sample_hz: f64) -> Self {
        Self {
            id: id.into(),
            arms,
            sample_hz,
        }
    }
}

impl ReplayTrajectory {
    fn new(frames: Vec<BTreeMap<String, f64>>, start_frame: usize) -> Self {
        Self {
            frames,
            start_frame,
            resolved_start_seconds: 0.0,
        }
    }
    fn starting_at(mut self, resolved_start_seconds: f64) -> Self {
        self.resolved_start_seconds = resolved_start_seconds;
        self
    }
}

impl ReplayPlan {
    fn into_parts(self) -> (String, String, Vec<BTreeMap<String, f64>>, f64, usize, f64) {
        (
            self.robot,
            self.episode,
            self.frames,
            self.sample_hz,
            self.start_frame,
            self.resolved_start_seconds,
        )
    }
}

pub(crate) trait ReplayPort {
    async fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, OperationError>;
    async fn load_trajectory(
        &self,
        request: ReplayTrajectoryRequest<'_>,
    ) -> Result<ReplayTrajectory, OperationError>;
    async fn start_replay(&self, plan: ReplayPlan) -> Result<(), OperationError>;
    async fn set_replay_paused(&self, paused: bool) -> Result<(), OperationError>;
}

pub(crate) struct ReplayTrajectoryRequest<'a> {
    episode: &'a str,
    source_arm: &'a str,
    start_seconds: f64,
}

impl<'a> ReplayTrajectoryRequest<'a> {
    fn new(episode: &'a str, source_arm: &'a str, start_seconds: f64) -> Self {
        Self {
            episode,
            source_arm,
            start_seconds,
        }
    }

    fn episode(&self) -> &str {
        self.episode
    }

    fn source_arm(&self) -> &str {
        self.source_arm
    }

    fn start_seconds(&self) -> f64 {
        self.start_seconds
    }
}

pub(crate) struct ReplayActions<'a, P: ReplayPort + ?Sized> {
    port: &'a P,
}

impl<'a, P: ReplayPort + ?Sized> ReplayActions<'a, P> {
    const fn new(port: &'a P) -> Self {
        Self { port }
    }

    /// Resolve the recorded arm, load the validated trajectory, then admit replay.
    async fn start(&self, command: StartReplayCommand) -> Result<(), OperationError> {
        if command.episode.trim().is_empty() || command.robot.trim().is_empty() {
            return Err(OperationError::InvalidInput(
                "replay requires an episode and target robot".to_owned(),
            ));
        }
        if !command.start_seconds.is_finite() || command.start_seconds < 0.0 {
            return Err(OperationError::InvalidInput(
                "replay start time must be a finite, non-negative number of seconds".to_owned(),
            ));
        }
        let summary = self.port.replay_episode(&command.episode).await?;
        let source = RecordedArms::new(&summary.id, &summary.arms, &command.robot)
            .select(command.source_arm.as_deref())?;
        let trajectory = self
            .port
            .load_trajectory(ReplayTrajectoryRequest::new(
                &command.episode,
                &source,
                command.start_seconds,
            ))
            .await?;
        if !summary.sample_hz.is_finite() || summary.sample_hz <= 0.0 {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} has an invalid sample rate",
                command.episode
            )));
        }
        if trajectory.frames.is_empty() || trajectory.start_frame >= trajectory.frames.len() {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} has no replayable frames at the requested start time",
                command.episode
            )));
        }
        if !trajectory.resolved_start_seconds.is_finite()
            || trajectory.resolved_start_seconds < 0.0
            || trajectory
                .frames
                .iter()
                .any(|frame| frame.is_empty() || frame.values().any(|value| !value.is_finite()))
        {
            return Err(OperationError::InvalidInput(format!(
                "take {:?} contains an invalid replay trajectory",
                command.episode
            )));
        }
        self.port
            .start_replay(ReplayPlan {
                robot: command.robot,
                episode: command.episode,
                frames: trajectory.frames,
                sample_hz: summary.sample_hz,
                start_frame: trajectory.start_frame,
                resolved_start_seconds: trajectory.resolved_start_seconds,
            })
            .await
    }

    async fn set_paused(&self, paused: bool) -> Result<(), OperationError> {
        self.port.set_replay_paused(paused).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::data::catalog_lease::CatalogMutationLease;
    use crate::recording_session::{RecordingOperation, RecordingSessionManager};

    struct FakePort {
        events: Mutex<Vec<String>>,
        session: Arc<RecordingSessionManager>,
        start_available: bool,
        active: bool,
        open_error: Option<OperationError>,
        begin_error: Option<OperationError>,
        stop_error: Option<OperationError>,
        idle_error: Option<OperationError>,
        pause_error: Option<OperationError>,
        commit_error: Option<OperationError>,
        episode: ReplayEpisode,
    }

    impl Default for FakePort {
        fn default() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                session: Arc::new(RecordingSessionManager::new(Arc::new(
                    CatalogMutationLease::default(),
                ))),
                start_available: true,
                active: false,

                open_error: None,
                begin_error: None,
                stop_error: None,
                idle_error: None,
                pause_error: None,
                commit_error: None,
                episode: ReplayEpisode {
                    id: "episode_000001".to_owned(),
                    arms: vec!["leader".to_owned(), "follower".to_owned()],
                    sample_hz: 60.0,
                },
            }
        }
    }

    impl FakePort {
        fn seed_recording_lease(&self) {
            let token = self.session.reserve_start(|| false).unwrap().unwrap();
            self.session.begin(token, ("episode_000001".into(), None));
        }
    }

    impl RecordingPort for FakePort {
        type RecordingDraft = String;
        type MediaCapture = String;
        type MediaOutput = Vec<String>;
        type Finalization = RecordingOperation;
        type CaptureChange = RecordingOperation;

        fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
            self.events.lock().unwrap().push("reserve-start".to_owned());
            if self.start_available {
                self.session.reserve_start(|| false)
            } else {
                Ok(None)
            }
        }

        fn bind_recording_lease(&self, token: CatalogLeaseToken, take: (String, Option<String>)) {
            self.events.lock().unwrap().push("retain-media".to_owned());
            self.session.begin(token, (take.0, None));
        }

        fn claim_finalization(&self) -> Result<Option<Self::Finalization>, OperationError> {
            let operation = self.session.claim_finalization()?;
            if operation.is_some() {
                self.events
                    .lock()
                    .unwrap()
                    .push("claim-finalization".to_owned());
            }
            Ok(operation)
        }

        fn finalization_episode(&self, operation: &Self::Finalization) -> String {
            operation.episode().to_owned()
        }

        fn complete_recording(&self, operation: Self::Finalization, _media: Self::MediaOutput) {
            operation.complete(Vec::new());
            self.events.lock().unwrap().push("release-start".to_owned());
        }

        fn release_recording_lease(&self, token: CatalogLeaseToken) {
            self.events.lock().unwrap().push("release-start".to_owned());
            self.session.cancel_start(token);
        }

        fn recording_active(&self) -> bool {
            self.active
        }

        fn recording_context(&self) -> RecordingContext {
            RecordingContext::new(vec!["leader".to_owned(), "follower".to_owned()], 60.0)
        }

        async fn enter_teleoperation(
            &self,
            _pair: &TeleoperationPair,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("teleoperate".to_owned());
            Ok(())
        }

        async fn enter_idle(&self) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("idle".to_owned());
            self.idle_error.clone().map_or(Ok(()), Err)
        }

        async fn create_recording_draft(
            &self,
            request: RecordingDraftRequest,
        ) -> Result<Self::RecordingDraft, OperationError> {
            let (dataset_id, _source, arms, sample_hz) = request.into_parts();
            self.events.lock().unwrap().push(format!(
                "draft:{}:{}:{sample_hz}",
                dataset_id,
                arms.join("+")
            ));
            self.open_error
                .clone()
                .map_or_else(|| Ok("episode_000001".to_owned()), Err)
        }

        fn draft_episode(&self, draft: &Self::RecordingDraft) -> String {
            draft.clone()
        }

        async fn start_recording_media(
            &self,
            episode: &str,
            _arms: &[String],
        ) -> Result<Option<Self::MediaCapture>, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("start-media:{episode}"));
            Ok(Some(episode.to_owned()))
        }

        async fn begin_recording(
            &self,
            draft: Self::RecordingDraft,
        ) -> Result<RecordingResult, OperationError> {
            self.events.lock().unwrap().push("begin".to_owned());
            self.begin_error
                .clone()
                .map_or_else(|| Ok(RecordingResult { episode: draft }), Err)
        }

        async fn abandon_recording_media(&self, _capture: Self::MediaCapture) {
            self.events.lock().unwrap().push("abandon-media".to_owned());
        }

        async fn stop_recording(&self, _keep: bool) -> Result<RecordingResult, OperationError> {
            self.events.lock().unwrap().push("stop".to_owned());
            self.stop_error.clone().map_or_else(
                || {
                    Ok(RecordingResult {
                        episode: "episode_000001".to_owned(),
                    })
                },
                Err,
            )
        }

        async fn finish_recording_media(
            &self,
            _operation: &mut Self::Finalization,
        ) -> Self::MediaOutput {
            self.events.lock().unwrap().push("finish-media".to_owned());
            vec!["video".to_owned()]
        }

        async fn commit_recording(
            &self,
            _episode: String,
            _media: &Self::MediaOutput,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("commit".to_owned());
            self.commit_error.clone().map_or(Ok(()), Err)
        }

        fn claim_capture_change(&self) -> Result<Self::CaptureChange, OperationError> {
            self.session.claim_capture_change()
        }

        fn request_media_pause(&self, _operation: &Self::CaptureChange) {
            self.events.lock().unwrap().push("request-pause".to_owned());
        }

        async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("hardware-pause:{paused}"));
            self.pause_error.clone().map_or(Ok(1.25), Err)
        }

        async fn finish_media_pause(
            &self,
            _operation: &mut Self::CaptureChange,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("finish-pause".to_owned());
            self.commit_error.clone().map_or(Ok(()), Err)
        }

        fn resume_media(&self, _operation: &mut Self::CaptureChange, active_seconds: f64) {
            self.events
                .lock()
                .unwrap()
                .push(format!("resume-media:{active_seconds}"));
        }
    }

    impl ReplayPort for FakePort {
        async fn replay_episode(&self, _episode: &str) -> Result<ReplayEpisode, OperationError> {
            self.events.lock().unwrap().push("episode".to_owned());
            Ok(self.episode.clone())
        }

        async fn load_trajectory(
            &self,
            request: ReplayTrajectoryRequest<'_>,
        ) -> Result<ReplayTrajectory, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("load:{}", request.source_arm()));
            Ok(ReplayTrajectory::new(
                vec![
                    BTreeMap::from([("joint".to_owned(), 0.25)]),
                    BTreeMap::from([("joint".to_owned(), 0.4)]),
                    BTreeMap::from([("joint".to_owned(), 0.5)]),
                ],
                2,
            )
            .starting_at(0.2))
        }

        async fn start_replay(&self, plan: ReplayPlan) -> Result<(), OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("replay:{}:{}", plan.robot, plan.start_frame));
            Ok(())
        }

        async fn set_replay_paused(&self, paused: bool) -> Result<(), OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("replay-pause:{paused}"));
            Ok(())
        }
    }

    fn start_command() -> StartRecordingCommand {
        StartRecordingCommand {
            pair: TeleoperationPair::new("leader", "follower"),
            dataset_id: " default ".to_owned(),
        }
    }

    #[tokio::test]
    async fn start_enters_teleoperation_before_opening_the_resolved_pair() {
        let port = FakePort::default();
        RecordingActions::new(&port)
            .start(start_command())
            .await
            .unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:default:leader+follower:60",
                "start-media:episode_000001",
                "begin",
                "retain-media"
            ]
        );
    }

    #[tokio::test]
    async fn failed_open_rolls_back_and_reports_a_failed_safety_transition() {
        let port = FakePort {
            open_error: Some(OperationError::Internal("Spiral unavailable".to_owned())),
            idle_error: Some(OperationError::Conflict("control stopped".to_owned())),
            ..FakePort::default()
        };
        let error = RecordingActions::new(&port)
            .start(start_command())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Spiral unavailable"));
        assert!(error.to_string().contains("control stopped"));
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:default:leader+follower:60",
                "idle",
                "release-start"
            ]
        );
    }

    #[tokio::test]
    async fn only_one_start_transaction_can_enter_teleoperation() {
        let port = FakePort {
            start_available: false,
            ..FakePort::default()
        };
        assert!(matches!(
            RecordingActions::new(&port).start(start_command()).await,
            Err(OperationError::Conflict(_))
        ));
        assert_eq!(*port.events.lock().unwrap(), ["reserve-start"]);
    }

    #[tokio::test]
    async fn hardware_refusal_finishes_started_media_before_rollback() {
        let port = FakePort {
            begin_error: Some(OperationError::Conflict("robot disconnected".to_owned())),
            ..FakePort::default()
        };
        RecordingActions::new(&port)
            .start(start_command())
            .await
            .unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:default:leader+follower:60",
                "start-media:episode_000001",
                "begin",
                "abandon-media",
                "idle",
                "release-start"
            ]
        );
    }

    #[tokio::test]
    async fn stop_attempts_idle_and_finishes_media_even_when_recording_fails() {
        let port = FakePort {
            stop_error: Some(OperationError::Internal("writer failed".to_owned())),
            ..FakePort::default()
        };
        port.seed_recording_lease();
        RecordingActions::new(&port).stop(true).await.unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "claim-finalization",
                "stop",
                "idle",
                "finish-media",
                "commit",
                "release-start"
            ]
        );
    }

    #[tokio::test]
    async fn durable_commit_failure_is_returned_after_recording_and_media_finish() {
        let port = FakePort {
            commit_error: Some(OperationError::Internal(
                "video rows could not be committed".to_owned(),
            )),
            ..FakePort::default()
        };
        port.seed_recording_lease();
        let error = RecordingActions::new(&port).stop(true).await.unwrap_err();
        assert!(error.message().contains("committed"));
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "claim-finalization",
                "stop",
                "idle",
                "finish-media",
                "commit",
                "release-start"
            ]
        );
    }

    #[tokio::test]
    async fn pause_closes_media_even_when_the_writer_barrier_fails() {
        let port = FakePort {
            pause_error: Some(OperationError::Internal("flush failed".to_owned())),
            ..FakePort::default()
        };
        port.seed_recording_lease();
        RecordingActions::new(&port)
            .set_paused(true)
            .await
            .unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["request-pause", "hardware-pause:true", "finish-pause"]
        );
    }

    #[tokio::test]
    async fn pause_returns_the_combined_episode_and_media_commit_failure() {
        let port = FakePort {
            commit_error: Some(OperationError::Internal(
                "paused episode could not be committed".to_owned(),
            )),
            ..FakePort::default()
        };
        port.seed_recording_lease();
        let error = RecordingActions::new(&port)
            .set_paused(true)
            .await
            .unwrap_err();
        assert!(error.message().contains("could not be committed"));
        assert_eq!(
            *port.events.lock().unwrap(),
            ["request-pause", "hardware-pause:true", "finish-pause"]
        );
    }

    #[tokio::test]
    async fn resume_restarts_hardware_before_media_at_the_active_clock() {
        let port = FakePort::default();
        port.seed_recording_lease();
        RecordingActions::new(&port)
            .set_paused(false)
            .await
            .unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["finish-pause", "hardware-pause:false", "resume-media:1.25"]
        );
    }

    #[tokio::test]
    async fn pause_resume_and_stop_require_ownership_before_hardware_calls() {
        let port = FakePort::default();
        let actions = RecordingActions::new(&port);
        for paused in [true, false] {
            assert!(matches!(
                actions.set_paused(paused).await,
                Err(OperationError::Conflict(_))
            ));
        }
        assert!(port.events.lock().unwrap().is_empty());

        port.seed_recording_lease();
        let finalizing = port.session.claim_finalization().unwrap().unwrap();
        for paused in [true, false] {
            assert!(matches!(
                actions.set_paused(paused).await,
                Err(OperationError::Conflict(_))
            ));
        }
        assert!(matches!(
            actions.stop(true).await,
            Err(OperationError::Conflict(_))
        ));
        assert!(
            port.events.lock().unwrap().is_empty(),
            "rejected claims must not touch hardware"
        );
        drop(finalizing);
        actions.set_paused(true).await.unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["request-pause", "hardware-pause:true", "finish-pause"]
        );
    }

    #[tokio::test]
    async fn replay_resolves_the_source_before_loading_or_driving() {
        let port = FakePort::default();
        ReplayActions::new(&port)
            .start(StartReplayCommand {
                episode: "episode_000001".to_owned(),
                robot: "follower".to_owned(),
                source_arm: None,
                start_seconds: 0.2,
            })
            .await
            .unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["episode", "load:follower", "replay:follower:2"]
        );
    }

    #[tokio::test]
    async fn invalid_replay_time_never_reads_or_drives_the_episode() {
        let port = FakePort::default();
        for start_seconds in [-0.1, f64::NAN, f64::INFINITY] {
            let error = ReplayActions::new(&port)
                .start(StartReplayCommand {
                    episode: "episode_000001".to_owned(),
                    robot: "follower".to_owned(),
                    source_arm: None,
                    start_seconds,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::InvalidInput(_)));
        }
        assert!(port.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_loaded_replay_never_reaches_control() {
        for sample_hz in [0.0, f64::NAN, f64::INFINITY] {
            let mut port = FakePort::default();
            port.episode.sample_hz = sample_hz;
            let error = ReplayActions::new(&port)
                .start(StartReplayCommand {
                    episode: "episode_000001".to_owned(),
                    robot: "follower".to_owned(),
                    source_arm: None,
                    start_seconds: 0.2,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::InvalidInput(_)));
            assert!(
                !port
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.starts_with("replay:"))
            );
        }
    }

    #[tokio::test]
    async fn replay_pause_is_one_transport_neutral_action() {
        let port = FakePort::default();
        ReplayActions::new(&port).set_paused(true).await.unwrap();
        assert_eq!(*port.events.lock().unwrap(), ["replay-pause:true"]);
    }
}

mod catalog;
mod episode_arms;
mod queries;
mod seek;
