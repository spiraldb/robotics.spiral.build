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
use crate::data::episode_arms::RecordedArms;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartRecordingCommand {
    dataset_id: String,
    arms: Vec<String>,
    note: Option<String>,
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
    media_finalizing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StopRecordingCommand {
    keep: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SetRecordingPausedCommand {
    paused: bool,
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
    pub(crate) fn new(
        dataset_id: impl Into<String>,
        arms: Vec<String>,
        note: Option<String>,
    ) -> Self {
        Self {
            dataset_id: dataset_id.into(),
            arms,
            note,
        }
    }
    pub(crate) fn into_parts(self) -> (String, Vec<String>, Option<String>) {
        (self.dataset_id, self.arms, self.note)
    }
}

impl TeleoperationPair {
    pub(crate) fn new(leader: impl Into<String>, follower: impl Into<String>) -> Self {
        Self {
            leader: leader.into(),
            follower: follower.into(),
        }
    }
    pub(crate) fn leader(&self) -> &str {
        &self.leader
    }
    pub(crate) fn follower(&self) -> &str {
        &self.follower
    }
}

impl RecordingContext {
    pub(crate) fn new(connected_arms: Vec<String>, sample_hz: f64, media_finalizing: bool) -> Self {
        Self {
            connected_arms,
            sample_hz,
            media_finalizing,
        }
    }
}

impl StopRecordingCommand {
    pub(crate) const fn new(keep: bool) -> Self {
        Self { keep }
    }
    pub(crate) const fn keep(self) -> bool {
        self.keep
    }
}

impl SetRecordingPausedCommand {
    pub(crate) const fn new(paused: bool) -> Self {
        Self { paused }
    }
}

impl RecordingResult {
    pub(crate) fn new(episode: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
        }
    }
    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }
}

pub(crate) struct RecordingDraftRequest {
    command: StartRecordingCommand,
    source: String,
    arms: Vec<String>,
    sample_hz: f64,
}

impl RecordingDraftRequest {
    fn new(command: StartRecordingCommand, source: String) -> Self {
        Self {
            command,
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

    pub(crate) fn into_parts(self) -> (StartRecordingCommand, String, Vec<String>, f64) {
        (self.command, self.source, self.arms, self.sample_hz)
    }
}

pub(crate) trait RecordingPort {
    type RecordingDraft;
    type MediaCapture;
    type MediaOutput;

    /// Atomically claim the one recording-start transaction.
    fn reserve_recording_start(&self) -> Result<Option<CatalogLeaseToken>, OperationError>;
    fn bind_recording_lease(&self, token: CatalogLeaseToken, episode: String);
    fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError>;
    fn release_recording_lease(&self, token: CatalogLeaseToken);
    fn recording_active(&self) -> bool;
    async fn default_teleoperation_pair(&self) -> Option<TeleoperationPair>;
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
    async fn commit_recording(
        &self,
        episode: String,
        media: Self::MediaOutput,
    ) -> Result<(), OperationError>;

    async fn request_media_pause(&self);
    async fn set_recording_paused(&self, paused: bool) -> Result<f64, OperationError>;
    async fn finish_media_pause(&self) -> Result<(), OperationError>;
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

pub(crate) struct RecordingActions<'a, P: RecordingPort + ?Sized> {
    port: &'a P,
}

impl<'a, P: RecordingPort + ?Sized> RecordingActions<'a, P> {
    pub(crate) const fn new(port: &'a P) -> Self {
        Self { port }
    }

    /// Enter the configured teleoperation session, then open one take.
    ///
    /// If any edge assembly fails after the mode transition, returning to idle is
    /// mandatory compensation. A failed rollback is retained in the operator
    /// error because an unnoticed live teleoperation session is safety-relevant.
    pub(crate) async fn start(
        &self,
        command: StartRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let Some(token) = self.port.reserve_recording_start()? else {
            return Err(OperationError::Conflict(
                "another recording is already opening; wait for it to finish".to_owned(),
            ));
        };
        let mut reservation = RecordingStartReservation::new(self.port, token);
        let result = self.start_reserved(command).await;
        if let Ok(recording) = &result {
            reservation.retain_for_recording(recording.episode.clone());
        }
        result
    }

    async fn start_reserved(
        &self,
        mut command: StartRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let port = self.port;
        if port.recording_active() {
            return Err(OperationError::Conflict(
                "a take is already recording; stop it first".to_owned(),
            ));
        }
        command.dataset_id = DatasetId::try_from(command.dataset_id.as_str())
            .map(String::from)
            .map_err(|error| OperationError::invalid_input(error.to_string()))?;
        command.note = command
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(str::to_owned);

        let pair = port.default_teleoperation_pair().await.ok_or_else(|| {
            OperationError::InvalidInput(
                "connect and calibrate two compatible arms before recording".to_owned(),
            )
        })?;
        let context = port.recording_context();
        if context.media_finalizing {
            return Err(OperationError::Conflict(
                "the previous episode's video is still being finalized; try again shortly"
                    .to_owned(),
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
            let request = RecordingDraftRequest::new(command, source)
                .with_capture(arms.clone(), context.sample_hz);
            let draft = port.create_recording_draft(request).await?;
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
    pub(crate) async fn stop(
        &self,
        command: StopRecordingCommand,
    ) -> Result<RecordingResult, OperationError> {
        let port = self.port;
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
                port.commit_recording(summary.episode.clone(), media)
                    .await?;
                Ok(summary)
            }
            (Ok(summary), Err(mode_error)) => {
                port.commit_recording(summary.episode, media).await?;
                Err(OperationError::Conflict(format!(
                    "the recording ended, but teleoperation could not stop: {mode_error}"
                )))
            }
            (Err(recording_error), Ok(())) => {
                if let Some(episode) = capture_episode {
                    port.commit_recording(episode, media).await?;
                }
                Err(recording_error)
            }
            (Err(recording_error), Err(mode_error)) => {
                if let Some(episode) = capture_episode {
                    port.commit_recording(episode, media).await?;
                }
                Err(OperationError::Conflict(format!(
                    "{recording_error}; teleoperation also could not stop: {mode_error}"
                )))
            }
        }
    }

    /// Pause/resume joint and media capture in a fail-safe order.
    pub(crate) async fn set_paused(
        &self,
        command: SetRecordingPausedCommand,
    ) -> Result<(), OperationError> {
        if command.paused {
            self.port.request_media_pause().await;
            let paused = self.port.set_recording_paused(true).await;
            let media = self.port.finish_media_pause().await;
            paused?;
            media?;
        } else {
            let active_seconds = self.port.set_recording_paused(false).await?;
            self.port.resume_media(active_seconds).await;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SetReplayPausedCommand {
    paused: bool,
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
    pub(crate) fn new(
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
    pub(crate) fn starting_at(mut self, start_seconds: f64) -> Self {
        self.start_seconds = start_seconds;
        self
    }
}

impl SetReplayPausedCommand {
    pub(crate) const fn new(paused: bool) -> Self {
        Self { paused }
    }
}

impl ReplayEpisode {
    pub(crate) fn new(id: impl Into<String>, arms: Vec<String>, sample_hz: f64) -> Self {
        Self {
            id: id.into(),
            arms,
            sample_hz,
        }
    }
}

impl ReplayTrajectory {
    pub(crate) fn new(frames: Vec<BTreeMap<String, f64>>, start_frame: usize) -> Self {
        Self {
            frames,
            start_frame,
            resolved_start_seconds: 0.0,
        }
    }
    pub(crate) fn starting_at(mut self, resolved_start_seconds: f64) -> Self {
        self.resolved_start_seconds = resolved_start_seconds;
        self
    }
}

impl ReplayPlan {
    pub(crate) fn into_parts(
        self,
    ) -> (String, String, Vec<BTreeMap<String, f64>>, f64, usize, f64) {
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

    pub(crate) fn episode(&self) -> &str {
        self.episode
    }

    pub(crate) fn source_arm(&self) -> &str {
        self.source_arm
    }

    pub(crate) fn start_seconds(&self) -> f64 {
        self.start_seconds
    }
}

pub(crate) struct ReplayActions<'a, P: ReplayPort + ?Sized> {
    port: &'a P,
}

impl<'a, P: ReplayPort + ?Sized> ReplayActions<'a, P> {
    pub(crate) const fn new(port: &'a P) -> Self {
        Self { port }
    }

    /// Resolve the recorded arm, load the validated trajectory, then admit replay.
    pub(crate) async fn start(&self, command: StartReplayCommand) -> Result<(), OperationError> {
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

    pub(crate) async fn set_paused(
        &self,
        command: SetReplayPausedCommand,
    ) -> Result<(), OperationError> {
        self.port.set_replay_paused(command.paused).await
    }
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

        fn bind_recording_lease(&self, token: CatalogLeaseToken, _episode: String) {
            *self.recording_token.lock().unwrap() = Some(token);
        }

        fn claim_recording_lease(&self) -> Result<Option<CatalogLeaseToken>, OperationError> {
            Ok(self.recording_token.lock().unwrap().take())
        }

        fn release_recording_lease(&self, token: CatalogLeaseToken) {
            self.events.lock().unwrap().push("release-start".to_owned());
            assert!(self.catalog.release(token));
            self.recording_token.lock().unwrap().take();
        }

        fn recording_active(&self) -> bool {
            self.active
        }

        async fn default_teleoperation_pair(&self) -> Option<TeleoperationPair> {
            self.pair.clone()
        }

        fn recording_context(&self) -> RecordingContext {
            RecordingContext::new(
                vec!["leader".to_owned(), "follower".to_owned()],
                60.0,
                false,
            )
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
            let (command, _source, arms, sample_hz) = request.into_parts();
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

        async fn commit_recording(
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

        async fn finish_media_pause(&self) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("finish-pause".to_owned());
            self.commit_error.clone().map_or(Ok(()), Err)
        }

        async fn resume_media(&self, active_seconds: f64) {
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
            dataset_id: " default ".to_owned(),
            arms: Vec::new(),
            note: Some("  useful take  ".to_owned()),
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
        RecordingActions::new(&port)
            .stop(StopRecordingCommand { keep: true })
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
        let error = RecordingActions::new(&port)
            .stop(StopRecordingCommand { keep: true })
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
        RecordingActions::new(&port)
            .set_paused(SetRecordingPausedCommand { paused: true })
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
        let error = RecordingActions::new(&port)
            .set_paused(SetRecordingPausedCommand { paused: true })
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
        RecordingActions::new(&port)
            .set_paused(SetRecordingPausedCommand { paused: false })
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
        ReplayActions::new(&port)
            .set_paused(SetReplayPausedCommand { paused: true })
            .await
            .unwrap();
        assert_eq!(*port.events.lock().unwrap(), ["replay-pause:true"]);
    }
}
