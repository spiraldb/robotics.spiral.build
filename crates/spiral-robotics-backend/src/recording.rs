//! Recording and replay capability actions.
//!
//! The ports in this module describe consequences, not implementation tools:
//! an HTTP adapter may use SpiralDB, cameras, and a control-thread actor,
//! while another delivery mechanism can provide different adapters without
//! changing session ordering, validation, or rollback policy.

use std::collections::BTreeMap;

use spiral_robotics_core::identity::DatasetId;

use crate::data::catalog_lease::CatalogLeaseToken;
use crate::data::episode_arms::RecordedArms;
use spiral_robotics_core::error::OperationError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRecordingCommand {
    pub dataset_id: String,
    pub arms: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleoperationPair {
    pub leader: String,
    pub follower: String,
}

/// Live facts needed to admit one new take.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingContext {
    pub connected_arms: Vec<String>,
    pub sample_hz: f64,
    pub media_finalizing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopRecordingCommand {
    pub keep: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetRecordingPausedCommand {
    pub paused: bool,
}

/// Consequences needed by the recording-session use cases.
///
/// `MediaCapture` and `MediaOutput` remain opaque so video libraries and media
/// formats cannot leak into capability policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingResult {
    pub episode: String,
}

pub trait RecordingPort {
    type RecordingDraft;
    type MediaCapture;
    type MediaOutput;

    /// Atomically claim the one recording-start transaction.
    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError>;
    fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String);
    fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError>;
    fn release_recording_lease(&self, token: CatalogLeaseToken);
    fn recording_active(&self) -> bool;
    fn default_teleoperation_pair(&self) -> Option<TeleoperationPair>;
    fn recording_context(&self) -> RecordingContext;

    async fn enter_teleoperation(&self, pair: &TeleoperationPair) -> Result<(), OperationError>;
    async fn enter_idle(&self) -> Result<(), OperationError>;
    fn create_recording_draft(
        &self,
        command: StartRecordingCommand,
        source: String,
        arms: Vec<String>,
        sample_hz: f64,
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
    async fn retain_recording_media(&self, capture: Option<Self::MediaCapture>);
    async fn abandon_recording_media(&self, capture: Self::MediaCapture);

    async fn take_recording_media(&self) -> Option<Self::MediaCapture>;
    fn media_episode(&self, capture: &Self::MediaCapture) -> String;
    async fn stop_recording(
        &self,
        command: StopRecordingCommand,
    ) -> Result<RecordingResult, OperationError>;
    async fn finish_recording_media(&self, capture: Self::MediaCapture) -> Self::MediaOutput;
    fn empty_media_output(&self) -> Self::MediaOutput;
    fn commit_recording(
        &self,
        episode: String,
        media: Self::MediaOutput,
    ) -> Result<(), OperationError>;

    async fn request_media_pause(&self);
    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError>;
    async fn finish_media_pause(&self);
    async fn resume_media(&self, active_seconds: f64);
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
    fn retain_for_recording(&mut self, episode: String) {
        let token = self.token.take().expect("recording reservation is live");
        self.port.bind_recording_lease(token, episode);
    }
}

impl<P: RecordingPort + ?Sized> Drop for RecordingStartReservation<'_, P> {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            self.port.release_recording_lease(token);
        }
    }
}

struct RecordingStopReservation<'a, P: RecordingPort + ?Sized> {
    port: &'a P,
    token: Option<CatalogLeaseToken>,
}

impl<P: RecordingPort + ?Sized> Drop for RecordingStopReservation<'_, P> {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            self.port.release_recording_lease(token);
        }
    }
}

/// Enter the configured teleoperation session, then open one take.
///
/// If any edge assembly fails after the mode transition, returning to idle is
/// mandatory compensation. A failed rollback is retained in the operator
/// error because an unnoticed live teleoperation session is safety-relevant.
pub async fn start_recording<P: RecordingPort + ?Sized>(
    port: &P,
    command: StartRecordingCommand,
) -> Result<RecordingResult, OperationError> {
    let Some(token) = port.reserve_recording_start()? else {
        return Err(OperationError::Conflict(
            "another recording is already opening; wait for it to finish".to_owned(),
        ));
    };
    let mut reservation = RecordingStartReservation::new(port, token);
    let result = start_reserved(port, command).await;
    if let Ok(recording) = &result {
        reservation.retain_for_recording(recording.episode.clone());
    }
    result
}

async fn start_reserved<P: RecordingPort + ?Sized>(
    port: &P,
    mut command: StartRecordingCommand,
) -> Result<RecordingResult, OperationError> {
    if port.recording_active() {
        return Err(OperationError::Conflict(
            "a take is already recording; stop it first".to_owned(),
        ));
    }
    command.dataset_id = validate_dataset_id(&command.dataset_id)?;
    command.note = command
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);

    let pair = port.default_teleoperation_pair().ok_or_else(|| {
        OperationError::InvalidInput(
            "connect and calibrate two compatible arms before recording".to_owned(),
        )
    })?;
    let context = port.recording_context();
    if context.media_finalizing {
        return Err(OperationError::Conflict(
            "the previous episode's video is still being finalized; try again shortly".to_owned(),
        ));
    }
    if !context.sample_hz.is_finite() || context.sample_hz <= 0.0 {
        return Err(OperationError::Internal(
            "the control loop reported an invalid sample rate".to_owned(),
        ));
    }
    let arms = if command.arms.is_empty() {
        vec![pair.leader.clone(), pair.follower.clone()]
    } else {
        command.arms.clone()
    };
    if arms.is_empty() {
        return Err(OperationError::InvalidInput(
            "connect at least one arm before recording a take".to_owned(),
        ));
    }
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
    command.arms = arms.clone();
    let source = format!("Teleoperation — {} → {}", pair.leader, pair.follower);

    port.enter_teleoperation(&pair).await?;
    let open = async {
        let draft =
            port.create_recording_draft(command, source, arms.clone(), context.sample_hz)?;
        let episode = port.draft_episode(&draft);
        let media = port.start_recording_media(&episode, &arms).await?;
        match port.begin_recording(draft).await {
            Ok(recording) => {
                port.retain_recording_media(media).await;
                Ok(recording)
            }
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
pub async fn stop_recording<P: RecordingPort + ?Sized>(
    port: &P,
    command: StopRecordingCommand,
) -> Result<RecordingResult, OperationError> {
    // The recording acquired this lease before allocating its episode ID. Hold
    // it until the take and media manifest are both finalized, and release it
    // on every success or failure path.
    let _reservation = RecordingStopReservation {
        port,
        token: port.claim_recording_lease()?,
    };
    let capture = port.take_recording_media().await;
    let capture_episode = capture.as_ref().map(|capture| port.media_episode(capture));
    let recording = port.stop_recording(command).await;
    let idle = port.enter_idle().await;
    let media = match capture {
        Some(capture) => port.finish_recording_media(capture).await,
        None => port.empty_media_output(),
    };

    match (recording, idle) {
        (Ok(summary), Ok(())) => {
            port.commit_recording(summary.episode.clone(), media)?;
            Ok(summary)
        }
        (Ok(summary), Err(mode_error)) => {
            port.commit_recording(summary.episode, media)?;
            Err(OperationError::Conflict(format!(
                "the recording ended, but teleoperation could not stop: {mode_error}"
            )))
        }
        (Err(recording_error), Ok(())) => {
            if let Some(episode) = capture_episode {
                port.commit_recording(episode, media)?;
            }
            Err(recording_error)
        }
        (Err(recording_error), Err(mode_error)) => {
            if let Some(episode) = capture_episode {
                port.commit_recording(episode, media)?;
            }
            Err(OperationError::Conflict(format!(
                "{recording_error}; teleoperation also could not stop: {mode_error}"
            )))
        }
    }
}

/// Pause/resume joint and media capture in a fail-safe order.
pub async fn set_recording_paused<P: RecordingPort + ?Sized>(
    port: &P,
    command: SetRecordingPausedCommand,
) -> Result<(), OperationError> {
    if command.paused {
        port.request_media_pause().await;
        let paused = port.set_recording_paused(true).await;
        port.finish_media_pause().await;
        paused?;
    } else {
        let active_seconds = port.set_recording_paused(false).await?;
        port.resume_media(active_seconds).await;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartReplayCommand {
    pub episode: String,
    pub robot: String,
    pub source_arm: Option<String>,
    pub start_seconds: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetReplayPausedCommand {
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayEpisode {
    pub id: String,
    pub arms: Vec<String>,
    pub sample_hz: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayTrajectory {
    pub frames: Vec<BTreeMap<String, f64>>,
    pub start_frame: usize,
    pub resolved_start_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayPlan {
    pub robot: String,
    pub episode: String,
    pub frames: Vec<BTreeMap<String, f64>>,
    pub sample_hz: f64,
    pub start_frame: usize,
    pub resolved_start_seconds: f64,
}

pub trait ReplayPort {
    fn replay_episode(&self, episode: &str) -> Result<ReplayEpisode, OperationError>;
    async fn load_trajectory(
        &self,
        episode: &str,
        source_arm: &str,
        start_seconds: f64,
    ) -> Result<ReplayTrajectory, OperationError>;
    async fn start_replay(&self, plan: ReplayPlan) -> Result<(), OperationError>;
    async fn set_replay_paused(&self, paused: bool) -> Result<(), OperationError>;
}

/// Resolve the recorded arm, load the validated trajectory, then admit replay.
pub async fn start_replay<P: ReplayPort + ?Sized>(
    port: &P,
    command: StartReplayCommand,
) -> Result<(), OperationError> {
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
    let summary = port.replay_episode(&command.episode)?;
    let source = RecordedArms::new(&summary.id, &summary.arms, &command.robot)
        .select(command.source_arm.as_deref())?;
    let trajectory = port
        .load_trajectory(&command.episode, &source, command.start_seconds)
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
    port.start_replay(ReplayPlan {
        robot: command.robot,
        episode: command.episode,
        frames: trajectory.frames,
        sample_hz: summary.sample_hz,
        start_frame: trajectory.start_frame,
        resolved_start_seconds: trajectory.resolved_start_seconds,
    })
    .await
}

pub async fn set_replay_paused<P: ReplayPort + ?Sized>(
    port: &P,
    command: SetReplayPausedCommand,
) -> Result<(), OperationError> {
    port.set_replay_paused(command.paused).await
}

fn validate_dataset_id(value: &str) -> Result<String, OperationError> {
    DatasetId::try_from(value)
        .map(String::from)
        .map_err(|error| OperationError::InvalidInput(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};

    struct FakePort {
        events: Mutex<Vec<String>>,
        catalog: CatalogMutationLease,
        recording_token: Mutex<Option<CatalogLeaseToken>>,
        start_available: bool,
        active: bool,
        pair: Option<TeleoperationPair>,
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
                catalog: CatalogMutationLease::default(),
                recording_token: Mutex::new(None),
                start_available: true,
                active: false,
                pair: Some(TeleoperationPair {
                    leader: "leader".to_owned(),
                    follower: "follower".to_owned(),
                }),
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
            let token = self
                .catalog
                .try_acquire(CatalogLeaseOwner::Recording)
                .expect("test recording lease");
            assert!(
                self.catalog
                    .bind_recording(token, "episode_000001".to_owned())
            );
            *self.recording_token.lock().unwrap() = Some(token);
        }
    }

    impl RecordingPort for FakePort {
        type RecordingDraft = String;
        type MediaCapture = String;
        type MediaOutput = Vec<String>;

        fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
            self.events.lock().unwrap().push("reserve-start".to_owned());
            Ok(self
                .start_available
                .then(|| self.catalog.try_acquire(CatalogLeaseOwner::Recording))
                .flatten())
        }

        fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String) {
            assert!(self.catalog.bind_recording(token, episode));
            *self.recording_token.lock().unwrap() = Some(token);
        }

        fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
            Ok(self.catalog.claim_recording().map(|claim| claim.token))
        }

        fn release_recording_lease(&self, token: CatalogLeaseToken) {
            self.events.lock().unwrap().push("release-start".to_owned());
            assert!(self.catalog.release(token));
            self.recording_token.lock().unwrap().take();
        }

        fn recording_active(&self) -> bool {
            self.active
        }

        fn default_teleoperation_pair(&self) -> Option<TeleoperationPair> {
            self.pair.clone()
        }

        fn recording_context(&self) -> RecordingContext {
            RecordingContext {
                connected_arms: vec!["leader".to_owned(), "follower".to_owned()],
                sample_hz: 60.0,
                media_finalizing: false,
            }
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

        fn create_recording_draft(
            &self,
            command: StartRecordingCommand,
            _source: String,
            arms: Vec<String>,
            sample_hz: f64,
        ) -> Result<Self::RecordingDraft, OperationError> {
            self.events.lock().unwrap().push(format!(
                "draft:{}:{}:{sample_hz}",
                command.arms.join("+"),
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

        async fn retain_recording_media(&self, _capture: Option<Self::MediaCapture>) {
            self.events.lock().unwrap().push("retain-media".to_owned());
        }

        async fn abandon_recording_media(&self, _capture: Self::MediaCapture) {
            self.events.lock().unwrap().push("abandon-media".to_owned());
        }

        async fn take_recording_media(&self) -> Option<Self::MediaCapture> {
            self.events.lock().unwrap().push("take-media".to_owned());
            Some("episode_000001".to_owned())
        }

        fn media_episode(&self, capture: &Self::MediaCapture) -> String {
            capture.clone()
        }

        async fn stop_recording(
            &self,
            _command: StopRecordingCommand,
        ) -> Result<RecordingResult, OperationError> {
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

        async fn finish_recording_media(&self, _capture: Self::MediaCapture) -> Self::MediaOutput {
            self.events.lock().unwrap().push("finish-media".to_owned());
            vec!["video".to_owned()]
        }

        fn empty_media_output(&self) -> Self::MediaOutput {
            Vec::new()
        }

        fn commit_recording(
            &self,
            _episode: String,
            _media: Self::MediaOutput,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("commit".to_owned());
            self.commit_error.clone().map_or(Ok(()), Err)
        }

        async fn request_media_pause(&self) {
            self.events.lock().unwrap().push("request-pause".to_owned());
        }

        async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("hardware-pause:{paused}"));
            self.pause_error.clone().map_or(Ok(1.25), Err)
        }

        async fn finish_media_pause(&self) {
            self.events.lock().unwrap().push("finish-pause".to_owned());
        }

        async fn resume_media(&self, active_seconds: f64) {
            self.events
                .lock()
                .unwrap()
                .push(format!("resume-media:{active_seconds}"));
        }
    }

    impl ReplayPort for FakePort {
        fn replay_episode(&self, _episode: &str) -> Result<ReplayEpisode, OperationError> {
            self.events.lock().unwrap().push("episode".to_owned());
            Ok(self.episode.clone())
        }

        async fn load_trajectory(
            &self,
            _episode: &str,
            source_arm: &str,
            _start_seconds: f64,
        ) -> Result<ReplayTrajectory, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("load:{source_arm}"));
            Ok(ReplayTrajectory {
                frames: vec![
                    BTreeMap::from([("joint".to_owned(), 0.25)]),
                    BTreeMap::from([("joint".to_owned(), 0.4)]),
                    BTreeMap::from([("joint".to_owned(), 0.5)]),
                ],
                start_frame: 2,
                resolved_start_seconds: 0.2,
            })
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
            dataset_id: " default ".to_owned(),
            arms: Vec::new(),
            note: Some("  useful take  ".to_owned()),
        }
    }

    #[tokio::test]
    async fn start_enters_teleoperation_before_opening_the_resolved_pair() {
        let port = FakePort::default();
        start_recording(&port, start_command()).await.unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:leader+follower:leader+follower:60",
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
        let error = start_recording(&port, start_command()).await.unwrap_err();
        assert!(error.to_string().contains("Spiral unavailable"));
        assert!(error.to_string().contains("control stopped"));
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:leader+follower:leader+follower:60",
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
            start_recording(&port, start_command()).await,
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
        start_recording(&port, start_command()).await.unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "reserve-start",
                "teleoperate",
                "draft:leader+follower:leader+follower:60",
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
        stop_recording(&port, StopRecordingCommand { keep: true })
            .await
            .unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "take-media",
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
        let error = stop_recording(&port, StopRecordingCommand { keep: true })
            .await
            .unwrap_err();
        assert!(error.message().contains("committed"));
        assert_eq!(
            *port.events.lock().unwrap(),
            [
                "take-media",
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
        set_recording_paused(&port, SetRecordingPausedCommand { paused: true })
            .await
            .unwrap_err();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["request-pause", "hardware-pause:true", "finish-pause"]
        );
    }

    #[tokio::test]
    async fn resume_restarts_hardware_before_media_at_the_active_clock() {
        let port = FakePort::default();
        set_recording_paused(&port, SetRecordingPausedCommand { paused: false })
            .await
            .unwrap();
        assert_eq!(
            *port.events.lock().unwrap(),
            ["hardware-pause:false", "resume-media:1.25"]
        );
    }

    #[tokio::test]
    async fn replay_resolves_the_source_before_loading_or_driving() {
        let port = FakePort::default();
        start_replay(
            &port,
            StartReplayCommand {
                episode: "episode_000001".to_owned(),
                robot: "follower".to_owned(),
                source_arm: None,
                start_seconds: 0.2,
            },
        )
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
            let error = start_replay(
                &port,
                StartReplayCommand {
                    episode: "episode_000001".to_owned(),
                    robot: "follower".to_owned(),
                    source_arm: None,
                    start_seconds,
                },
            )
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
            let error = start_replay(
                &port,
                StartReplayCommand {
                    episode: "episode_000001".to_owned(),
                    robot: "follower".to_owned(),
                    source_arm: None,
                    start_seconds: 0.2,
                },
            )
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
        set_replay_paused(&port, SetReplayPausedCommand { paused: true })
            .await
            .unwrap();
        assert_eq!(*port.events.lock().unwrap(), ["replay-pause:true"]);
    }
}
