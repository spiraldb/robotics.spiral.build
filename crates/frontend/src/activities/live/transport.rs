//! What Live's Timeline controls do, and how the recorded frames behind them
//! are asked for.
//!
//! Live is the one Timeline whose clock is not its own: while a robot is
//! replaying an episode the elapsed time arrives from the server, and the
//! shared timeline renders it through the read-only [`PlaybackClock`]
//! projection `TimelineState` takes. So the phase below is a report, not a
//! command — [`LiveTransport`] turns a press on a shared control into the one
//! recording, preview, or robot-sync consequence that phase implies.

use dioxus::prelude::*;
use spiral_robotics_common::recording::{
    EpisodeInfo, RecordingProgress, ReplayPhase, ReplayProgress, ReplayPurpose,
};
use spiral_robotics_common::robots::ModeSelection;

use super::recording_actions::{RecordingCompletion, TimelineSeekBody};
use super::recording_review::{
    EpisodePlaybackTime, EpisodePreview, EpisodePreviewRequest, EpisodePreviewSignals,
};
use super::session::LiveSession;
use super::timeline::{TimelineSubject, TimelineTarget};
use crate::components::timeline::{KeyboardFocus, SeekPhase};
use crate::panels::recorded_source::RecordedSource;
use crate::shell::notifications::MessageSource;

/// Durable Timeline state for as long as the Live workspace is mounted.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TimelineController {
    playhead: Signal<f64>,
    preview: Signal<Option<(String, f64)>>,
    follow_live: Signal<bool>,
    robot_sync_target: Signal<Option<TimelineTarget>>,
}

impl TimelineController {
    pub(super) fn use_controller() -> Self {
        use_hook(|| Self {
            playhead: Signal::new(0.0),
            preview: Signal::new(None),
            follow_live: Signal::new(true),
            robot_sync_target: Signal::new(None),
        })
    }

    pub(super) const fn playhead_signal(self) -> Signal<f64> {
        self.playhead
    }

    pub(super) const fn follow_live_signal(self) -> Signal<bool> {
        self.follow_live
    }

    pub(super) const fn robot_sync_target_signal(self) -> Signal<Option<TimelineTarget>> {
        self.robot_sync_target
    }

    fn start_preview(mut self, episode: String, duration_seconds: f64) {
        if (self.playhead)() >= duration_seconds {
            self.playhead.set(0.0);
        }
        self.preview.set(Some((episode, duration_seconds)));
    }

    pub(super) fn stop_preview(mut self) {
        self.preview.set(None);
    }

    fn preview_target(&self) -> Option<(String, f64)> {
        self.preview.read().clone()
    }

    /// Keep the preview clock mounted, but run it only while its workspace is active.
    fn use_clock(self, is_active: ReadSignal<bool>, on_sample: EventHandler<(String, f64)>) {
        let mut playhead = self.playhead;
        crate::components::timeline::PlaybackClock::use_ticker(
            is_active,
            use_callback(move |elapsed| {
                let Some((episode, duration)) = self.preview_target() else {
                    return;
                };
                let mut clock = crate::components::timeline::PlaybackClock::new(duration);
                clock.seek(playhead());
                clock.advance(elapsed);
                let next = clock.seconds();
                playhead.set(next);
                on_sample.call((episode, next));
                if next >= duration {
                    self.stop_preview();
                }
            }),
        );
    }

    /// Who is driving the clock right now: the server's replay, if it owns the
    /// transport, and otherwise this workspace's own preview.
    pub(super) fn phase(self, replay: Option<&ReplayProgress>) -> LivePhase {
        replay
            .filter(|replay| replay.owns_timeline_transport())
            .map(|replay| {
                if replay.paused() {
                    LivePhase::RobotSyncPaused
                } else if replay.phase() == ReplayPhase::Seeking {
                    LivePhase::RobotSyncSeeking
                } else {
                    LivePhase::RobotSyncPlaying
                }
            })
            .unwrap_or_else(|| {
                if self.preview.read().is_some() {
                    LivePhase::Previewing
                } else {
                    LivePhase::Stopped
                }
            })
    }
}

/// Physical seeks are consequences of the Timeline playhead, not transport
/// that may replace or reposition it while the arm is travelling to a pose.
pub(crate) trait TimelineReplay {
    fn owns_timeline_transport(&self) -> bool;
    fn matches_timeline_subject(&self, subject_episode: Option<&str>) -> bool;
    #[cfg(test)]
    fn drives_recorded_media(&self) -> bool;
}

impl TimelineReplay for ReplayProgress {
    fn owns_timeline_transport(&self) -> bool {
        self.purpose() == ReplayPurpose::Timeline
    }

    fn matches_timeline_subject(&self, subject_episode: Option<&str>) -> bool {
        subject_episode == Some(self.episode())
    }

    #[cfg(test)]
    fn drives_recorded_media(&self) -> bool {
        self.owns_timeline_transport() && self.phase() == ReplayPhase::Playing && !self.paused()
    }
}

/// Whether the Timeline clock is stopped, previewing recorded data, or being
/// driven by the server-owned consequence of robot synchronization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum LivePhase {
    #[default]
    Stopped,
    Previewing,
    RobotSyncSeeking,
    RobotSyncPlaying,
    RobotSyncPaused,
}

/// What pressing the shared play control means in a phase. Robot sync takes
/// over the control entirely once armed: the same button that previews
/// recorded frames starts, pauses, and resumes the robot instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportIntent {
    StartPreview,
    StopPreview,
    StartReplay,
    PauseReplay,
    ResumeReplay,
}

impl LivePhase {
    const fn press_play(self, robot_sync: bool) -> TransportIntent {
        match self {
            Self::Previewing => TransportIntent::StopPreview,
            Self::RobotSyncSeeking | Self::RobotSyncPlaying => TransportIntent::PauseReplay,
            Self::RobotSyncPaused => TransportIntent::ResumeReplay,
            Self::Stopped if robot_sync => TransportIntent::StartReplay,
            Self::Stopped => TransportIntent::StartPreview,
        }
    }

    pub(super) const fn playing(self) -> bool {
        matches!(self, Self::Previewing) || self.robot_sync_active()
    }

    /// The robot is moving to the Timeline's tune, so a scrub must stop it.
    pub(super) const fn robot_sync_active(self) -> bool {
        matches!(self, Self::RobotSyncSeeking | Self::RobotSyncPlaying)
    }
}

/// Live's clock as one render sees it: where the playhead is, who is driving
/// it, and the two operator options that change what driving it means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LiveClock {
    seconds: f64,
    phase: LivePhase,
    follow_live: bool,
    robot_sync_armed: bool,
}

impl LiveClock {
    pub(super) const fn new(seconds: f64, phase: LivePhase) -> Self {
        Self {
            seconds,
            phase,
            follow_live: false,
            robot_sync_armed: false,
        }
    }

    pub(super) const fn with_options(mut self, follow_live: bool, robot_sync_armed: bool) -> Self {
        self.follow_live = follow_live;
        self.robot_sync_armed = robot_sync_armed;
        self
    }
}

/// Why each Timeline control is or is not available for this subject.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TimelineCapabilities {
    record: Result<(), String>,
    seek: Result<(), String>,
    /// Whether full-episode playback may drive the compatible robot.
    synced_playback: Result<(), String>,
    /// Whether this Timeline subject may be armed to drive the robot.
    robot_sync: Result<(), String>,
}

impl TimelineCapabilities {
    fn new(record: Result<(), String>, seek: Result<(), String>) -> Self {
        Self {
            record,
            seek,
            synced_playback: Err("Robot playback is unavailable".into()),
            robot_sync: Err("Robot sync is unavailable".into()),
        }
    }

    fn with_robot_controls(
        mut self,
        synced_playback: Result<(), String>,
        robot_sync: Result<(), String>,
    ) -> Self {
        self.synced_playback = synced_playback;
        self.robot_sync = robot_sync;
        self
    }
}

/// The recorded frames behind the Timeline, and the two throttles that keep a
/// scrubbing playhead from asking for one snapshot per rendered frame.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct LivePreview {
    session: LiveSession,
    signals: EpisodePreviewSignals,
    sampled: Signal<Option<(String, u64)>>,
    interactive: Signal<Option<(String, u64)>>,
}

impl LivePreview {
    pub(super) fn use_preview(session: LiveSession) -> Self {
        let signals = EpisodePreviewSignals::use_signals();
        // Throttling is local to the mounted screen; remounting must sample again.
        let preview = Self {
            session,
            signals,
            sampled: use_signal(|| None),
            interactive: use_signal(|| None),
        };
        let on_sample = use_callback(move |(episode, seconds)| preview.sample(episode, seconds));
        session
            .timeline_controller()
            .use_clock(session.telemetry().is_active_signal(), on_sample);
        preview.use_recording_clock();
        preview
    }

    pub(super) const fn session(self) -> LiveSession {
        self.session
    }

    fn sample(mut self, episode: String, seconds: f64) {
        let Some(recording) = self
            .session
            .telemetry()
            .recording()
            .filter(|recording| recording.paused() && recording.episode() == episode)
        else {
            return;
        };
        let target = (episode.clone(), seconds.replay_slot());
        if self.sampled.peek().as_ref() == Some(&target) {
            return;
        }
        self.sampled.set(Some(target));
        self.signals.request(EpisodePreviewRequest::new(
            episode,
            Some(recording.frames()),
            seconds,
        ));
    }

    fn use_recording_clock(self) {
        let timeline = self.session.timeline_controller();
        let mut playhead = timeline.playhead_signal();
        let follow_live = timeline.follow_live_signal();
        use_effect(move || {
            let recording = self.session.telemetry().recording();
            if let Some(recording) = recording.as_ref().filter(|recording| !recording.paused()) {
                if follow_live() {
                    playhead.set(recording.elapsed_seconds());
                }
                timeline.stop_preview();
                return;
            }
            if recording.is_none() {
                timeline.stop_preview();
                return;
            }
            if let Some(replay) = self.session.telemetry().replay().filter(|replay| {
                replay.owns_timeline_transport()
                    && replay.matches_timeline_subject(
                        recording.as_ref().map(|recording| recording.episode()),
                    )
            }) {
                playhead.set(replay.elapsed_seconds());
                timeline.stop_preview();
                self.sample(replay.episode().to_owned(), replay.elapsed_seconds());
            }
        });
    }

    pub(super) fn use_source_identity(mut self, subject: &TimelineSubject) {
        let identity = subject
            .recording()
            .filter(|recording| recording.paused())
            .map(|recording| {
                (
                    recording.episode().to_owned(),
                    recording.frames(),
                    recording.elapsed_seconds(),
                )
            });
        use_effect(use_reactive((&identity,), move |(identity,)| {
            // Finalization, resume, and source revision changes invalidate in-flight reads.
            self.signals.discard();
            self.sampled.set(None);
            self.interactive.set(None);
            if let Some((episode, frames, seconds)) = identity {
                let mut playhead = self.session.timeline_controller().playhead_signal();
                playhead.set(seconds);
                self.signals
                    .request(EpisodePreviewRequest::new(episode, Some(frames), seconds));
            }
        }));
    }

    pub(super) fn source(
        self,
        subject: &TimelineSubject,
    ) -> Option<RecordedSource<EpisodePreview>> {
        self.signals
            .source()
            .filter(|source| Some(source.episode()) == subject.recorded_episode())
    }

    /// Ask for the frame under a moving playhead, at most once per render slot
    /// while scrubbing and always once the scrub is committed.
    fn request_scrub(mut self, episode: &str, seconds: f64, committed: bool) {
        let target = (episode.to_owned(), seconds.replay_slot());
        if !committed && self.interactive.peek().as_ref() == Some(&target) {
            return;
        }
        self.interactive.set(Some(target));
        self.signals.request(EpisodePreviewRequest::new(
            episode.to_owned(),
            None,
            seconds,
        ));
    }

    /// The take these frames came from is finished; nothing in flight is wanted.
    fn forget_recorded(mut self) {
        self.signals.discard();
        self.interactive.set(None);
    }
}

/// One press on a Timeline control, resolved against the subject the panel was
/// rendered for.
///
/// This replaces the parallel action vocabulary Live used to send across the
/// panel boundary as data and re-validate arm by arm: the subject travels with
/// the verbs, so each verb states its own consequence and the only check left
/// is the one that is real — that the recording has not moved on since the
/// render that drew the control.
#[derive(Clone, PartialEq)]
pub(super) struct LiveTransport {
    preview: LivePreview,
    subject: TimelineSubject,
    clock: LiveClock,
    capabilities: TimelineCapabilities,
}

impl LiveTransport {
    pub(super) fn new(preview: LivePreview, subject: TimelineSubject, clock: LiveClock) -> Self {
        let capabilities = preview.session().live_timeline_capabilities(&subject);
        Self {
            preview,
            subject,
            clock,
            capabilities,
        }
    }

    pub(super) const fn subject(&self) -> &TimelineSubject {
        &self.subject
    }

    pub(super) const fn seconds(&self) -> f64 {
        self.clock.seconds
    }

    pub(super) const fn phase(&self) -> LivePhase {
        self.clock.phase
    }

    pub(super) const fn follow_live(&self) -> bool {
        self.clock.follow_live
    }

    /// Whether the operator has asked this subject to drive the robot, which is
    /// what the toggle shows whether or not the robot can currently accept it.
    pub(super) const fn robot_sync_armed(&self) -> bool {
        self.clock.robot_sync_armed
    }

    /// Why recording, scrubbing, or arming the robot is unavailable, if it is.
    pub(super) fn record_blocker(&self) -> Option<String> {
        self.capabilities.record.clone().err()
    }

    pub(super) fn seek_blocker(&self) -> Option<String> {
        self.capabilities.seek.clone().err()
    }

    pub(super) fn robot_sync_blocker(&self) -> Option<String> {
        self.capabilities.robot_sync.clone().err()
    }

    /// Armed *and* currently permitted: the toggle can stay lit while the robot
    /// is busy, but nothing may be sent to it until the gate also passes.
    pub(super) fn robot_sync_engaged(&self) -> bool {
        self.clock.robot_sync_armed && self.capabilities.robot_sync.is_ok()
    }

    /// Why the play control is unavailable, if it is. Once the robot owns the
    /// transport it answers for itself; before that, arming it raises the bar
    /// from "this episode can be scrubbed" to "this robot can be driven".
    pub(super) fn play_blocker(&self) -> Option<String> {
        match self.clock.phase {
            LivePhase::RobotSyncSeeking
            | LivePhase::RobotSyncPlaying
            | LivePhase::RobotSyncPaused => None,
            LivePhase::Stopped if self.robot_sync_engaged() => {
                self.capabilities.synced_playback.clone().err()
            }
            LivePhase::Previewing | LivePhase::Stopped => self.seek_blocker(),
        }
    }

    const fn session(&self) -> LiveSession {
        self.preview.session()
    }

    fn controller(&self) -> TimelineController {
        self.session().timeline_controller()
    }

    /// The subject this panel was rendered for, if the robot is still on it.
    ///
    /// A take can finish between the render that drew a control and the click
    /// that presses it; a stale click must not reach whatever replaced it.
    fn subject_now(&self) -> Option<&TimelineSubject> {
        let recording = self.session().telemetry().recording();
        if self.subject.episode_id() == recording.as_ref().map(RecordingProgress::episode) {
            return Some(&self.subject);
        }
        self.session()
            .project()
            .report("The Timeline subject changed before the action was applied.");
        None
    }

    fn recording_episode(&self) -> Option<String> {
        self.subject_now()
            .and_then(TimelineSubject::episode_id)
            .map(str::to_owned)
    }

    pub(super) fn start_capture(&self) {
        if self.subject_now().is_some() {
            self.session().start_recording();
        }
    }

    pub(super) fn set_capture_paused(&self, paused: bool) {
        let Some(episode) = self.recording_episode() else {
            return;
        };
        self.session().set_recording_paused(&episode, paused);
        self.controller().follow_live_signal().set(!paused);
        if !paused {
            self.preview.signals.clear();
        }
    }

    /// Finish the take on the clock, keeping it or throwing it away.
    pub(super) fn finish_capture(&self, keep: bool) {
        let Some(episode) = self.recording_episode() else {
            return;
        };
        let preview = self.preview;
        self.session().stop_recording(
            &episode,
            RecordingCompletion::new(keep, move |_: EpisodeInfo| preview.forget_recorded()),
        );
    }

    pub(super) fn press_play(&self) {
        let Some(subject) = self.subject_now() else {
            return;
        };
        let Some(episode) = subject.episode_id().map(str::to_owned) else {
            return;
        };
        let controller = self.controller();
        let mut follow_live = controller.follow_live_signal();
        match self.clock.phase.press_play(self.robot_sync_engaged()) {
            TransportIntent::StartPreview => {
                controller.start_preview(episode, subject.duration_seconds());
                follow_live.set(false);
            }
            TransportIntent::StopPreview => controller.stop_preview(),
            TransportIntent::StartReplay => {
                controller.stop_preview();
                follow_live.set(false);
                if let Some(summary) = subject.episode() {
                    self.session()
                        .start_replay(summary, controller.playhead_signal()());
                }
            }
            TransportIntent::PauseReplay => self.session().set_replay_paused(&episode, true),
            TransportIntent::ResumeReplay => self.session().set_replay_paused(&episode, false),
        }
    }

    /// A scrub takes the transport back from the robot before it moves the
    /// playhead out from under it.
    pub(super) fn pause_robot_sync(&self) {
        if let Some(episode) = self.recording_episode() {
            self.session().set_replay_paused(&episode, true);
        }
    }

    pub(super) fn seek(&self, seconds: f64, phase: SeekPhase) {
        let Some(subject) = self.subject_now() else {
            return;
        };
        let Some(target) = subject.target() else {
            return;
        };
        let committed = phase == SeekPhase::Commit;
        let controller = self.controller();
        controller.stop_preview();
        controller.playhead_signal().set(seconds);
        if let Some(episode) = subject.recorded_episode() {
            self.preview.request_scrub(episode, seconds, committed);
        }
        if committed
            && self.robot_sync_engaged()
            && controller.robot_sync_target_signal().peek().as_ref() == Some(&target)
            && let Some((episode, robot, source_arm)) =
                self.session().timeline_seek_context(subject)
        {
            self.session()
                .physical_seek(episode, TimelineSeekBody::new(robot, source_arm, seconds));
        }
    }

    pub(super) fn set_follow_live(&self, follow: bool) {
        if self.subject_now().is_none() {
            return;
        }
        let controller = self.controller();
        controller.follow_live_signal().set(follow);
        if follow {
            controller.stop_preview();
            if let Some(recording) = self.session().telemetry().recording() {
                controller
                    .playhead_signal()
                    .set(recording.elapsed_seconds());
            }
        }
    }

    pub(super) fn set_robot_sync(&self, enabled: bool) {
        let Some(subject) = self.subject_now() else {
            return;
        };
        let Some(target) = subject.target() else {
            return;
        };
        let controller = self.controller();
        let mut armed = controller.robot_sync_target_signal();
        if enabled {
            controller.stop_preview();
            armed.set(Some(target));
            return;
        }
        armed.set(None);
        if let Some(replay) = self.session().telemetry().replay()
            && subject.episode_id() == Some(replay.episode())
        {
            self.session().stop_replay(replay.episode());
        }
    }

    /// Ask for this subject's recorded traces again after a failed read.
    pub(super) fn reload_recorded(&self) {
        let Some(subject) = self.subject_now() else {
            return;
        };
        let Some(episode) = subject.recorded_episode() else {
            return;
        };
        self.preview.signals.clear();
        self.preview.signals.request(EpisodePreviewRequest::new(
            episode.to_owned(),
            None,
            self.controller().playhead_signal()(),
        ));
    }

    pub(super) fn on_key(&self, event: KeyboardEvent) {
        let modifiers = event.modifiers();
        if modifiers.ctrl() || modifiers.meta() || modifiers.alt() || KeyboardFocus::editing() {
            return;
        }
        let recording = self.subject.recording().is_some();
        match event.key() {
            Key::Character(key) if key.eq_ignore_ascii_case("r") => {
                event.prevent_default();
                if recording {
                    self.finish_capture(true);
                } else {
                    self.start_capture();
                }
            }
            Key::Character(key) if key.eq_ignore_ascii_case("x") && recording => {
                event.prevent_default();
                if modifiers.shift() {
                    self.finish_capture(false);
                } else {
                    self.session().project().report_info_from(
                        MessageSource::Live,
                        "Press Shift+X to discard this take.",
                    );
                }
            }
            _ => {}
        }
    }
}

impl LiveSession {
    fn live_timeline_capabilities(self, subject: &TimelineSubject) -> TimelineCapabilities {
        let record = match self.recording_blocker() {
            Some(reason) => Err(reason),
            None => Ok(()),
        };
        let preview = if subject.recorded_episode().is_none() {
            Err("Select a completed episode or pause the current recording first.")
        } else if !subject.available() {
            Err(subject
                .problem()
                .unwrap_or("This Timeline subject is unavailable."))
        } else {
            Ok(())
        };
        let synced_playback = match subject.episode() {
            Some(episode) => self.replay_blocker(episode).map(Err).unwrap_or(Ok(())),
            None => Err("Select a completed episode to play it with robot sync.".into()),
        };
        let robot_sync = if !subject.is_paused_recording() && subject.episode().is_none() {
            Err("Pause or select a completed episode before synchronizing a robot.")
        } else if !self.telemetry().controls_available() {
            Err("Reconnect telemetry before synchronizing a robot.")
        } else if self.telemetry().mode_active()
            && !matches!(self.telemetry().mode(), ModeSelection::Replay { .. })
        {
            Err("Stop the active control mode before synchronizing a robot.")
        } else if self.timeline_seek_context(subject).is_none() {
            Err("Connect and calibrate a compatible follower to synchronize it.")
        } else {
            Ok(())
        };
        TimelineCapabilities::new(record, preview.map_err(str::to_owned))
            .with_robot_controls(synced_playback, robot_sync.map_err(str::to_owned))
    }

    /// The episode, follower, and source arm a physical seek would move, if
    /// this subject names all three.
    fn timeline_seek_context(
        self,
        subject: &TimelineSubject,
    ) -> Option<(String, String, Option<String>)> {
        let follower = self
            .telemetry()
            .connected_robots()
            .into_iter()
            .find(|robot| {
                robot.calibrated()
                    && robot
                        .robot_type()
                        .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
            })?;
        let (episode, arms) = match subject {
            TimelineSubject::Recording(recording) if recording.paused() => {
                (recording.episode().to_owned(), recording.arms().to_owned())
            }
            TimelineSubject::Episode { episode, .. } => (
                episode.id().to_owned(),
                episode
                    .arms()
                    .iter()
                    .map(|arm| arm.id().to_owned())
                    .collect(),
            ),
            TimelineSubject::Empty { .. } | TimelineSubject::Recording(_) => return None,
        };
        let source_arm = arms
            .iter()
            .find(|arm| **arm == follower.id())
            .or_else(|| {
                arms.iter()
                    .find(|arm| arm.to_ascii_lowercase().contains("follower"))
            })
            .or_else(|| arms.first())
            .cloned();
        Some((episode, follower.id().to_owned(), source_arm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::recording::{ReplayIdentity, ReplayTimeline, ReplayTransport};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn one_transport_routes_through_the_robot_only_when_sync_is_enabled() {
        assert_eq!(
            LivePhase::Stopped.press_play(false),
            TransportIntent::StartPreview
        );
        assert_eq!(
            LivePhase::Stopped.press_play(true),
            TransportIntent::StartReplay
        );
        assert_eq!(
            LivePhase::Previewing.press_play(false),
            TransportIntent::StopPreview
        );
        assert_eq!(
            LivePhase::RobotSyncPlaying.press_play(true),
            TransportIntent::PauseReplay
        );
        assert_eq!(
            LivePhase::RobotSyncSeeking.press_play(true),
            TransportIntent::PauseReplay
        );
        assert_eq!(
            LivePhase::RobotSyncPaused.press_play(true),
            TransportIntent::ResumeReplay
        );
        assert!(!LivePhase::Stopped.playing());
        assert!(LivePhase::Previewing.playing());
        assert!(!LivePhase::Previewing.robot_sync_active());
        assert!(LivePhase::RobotSyncSeeking.robot_sync_active());
        assert!(!LivePhase::RobotSyncPaused.playing());
    }

    #[derive(Clone, Default)]
    struct ClockHarness(std::rc::Rc<std::cell::RefCell<Option<ClockObservation>>>);

    struct ClockObservation {
        active: Signal<bool>,
        revision: Signal<u64>,
        timeline: TimelineController,
        samples: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl ClockHarness {
        fn render(self) -> Element {
            let active = use_signal(|| true);
            let revision = use_signal(|| 0_u64);
            let timeline = TimelineController::use_controller();
            let samples = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(0)));
            let sampled = samples.clone();
            let on_sample = use_callback(move |_| sampled.set(sampled.get() + 1));
            timeline.use_clock(active.into(), on_sample);
            *self.0.borrow_mut() = Some(ClockObservation {
                active,
                revision,
                timeline,
                samples,
            });
            rsx! { span { "{revision()}" } }
        }

        async fn tick(dom: &mut VirtualDom) {
            gloo_timers::future::TimeoutFuture::new(50).await;
            dom.render_immediate_to_vec();
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn hidden_preview_clock_stays_paused_through_rerenders_and_resumes_on_activation() {
        let harness = ClockHarness::default();
        let mut dom = VirtualDom::new_with_props(ClockHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let (mut active, mut revision, timeline, samples) = {
            let observed = harness.0.borrow();
            let observed = observed.as_ref().unwrap();
            (
                observed.active,
                observed.revision,
                observed.timeline,
                observed.samples.clone(),
            )
        };
        dom.in_runtime(|| timeline.start_preview("episode".into(), 60.0));
        dom.render_immediate_to_vec();
        ClockHarness::tick(&mut dom).await;
        assert!(samples.get() > 0);
        dom.in_runtime(|| active.set(false));
        dom.render_immediate_to_vec();
        let paused_samples = samples.get();
        ClockHarness::tick(&mut dom).await;
        dom.in_runtime(|| revision.set(1));
        dom.render_immediate_to_vec();
        ClockHarness::tick(&mut dom).await;
        assert_eq!(samples.get(), paused_samples);
        dom.in_runtime(|| active.set(true));
        dom.render_immediate_to_vec();
        ClockHarness::tick(&mut dom).await;
        assert!(samples.get() > paused_samples);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn preview_clock_bounds_delayed_ticks_and_samples_the_final_frame() {
        let mut clock = crate::components::timeline::PlaybackClock::new(2.0);
        clock.seek(1.0);
        clock.advance(100.0);
        assert_eq!(clock.seconds(), 1.25);
        clock.advance(-1.0);
        assert_eq!(clock.seconds(), 1.25);
        clock.seek(1.99);
        clock.advance(0.2);
        assert_eq!(clock.seconds(), 2.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_render_slots_bound_sample_fetches_to_thirty_hz() {
        assert_eq!(f64::NAN.replay_slot(), 0);
        assert_eq!(0.0_f64.replay_slot(), 0);
        assert_eq!(0.032_f64.replay_slot(), 0);
        assert_eq!(0.034_f64.replay_slot(), 1);
        assert_eq!(1.249_f64.replay_slot(), 37);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn physical_seek_never_takes_ownership_of_timeline_transport() {
        let mut replay = ReplayProgress::new(
            ReplayIdentity::new("take-1", "follower"),
            ReplayTimeline::new(0, 1, 0.0).with_duration_seconds(1.0 / 60.0),
            ReplayTransport::new(ReplayPurpose::PhysicalSeek, ReplayPhase::Seeking, false),
        );

        assert!(!replay.owns_timeline_transport());
        assert!(!replay.drives_recorded_media());
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Seeking,
            false,
        ));
        assert!(replay.owns_timeline_transport());
        assert!(!replay.drives_recorded_media());
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Playing,
            false,
        ));
        assert!(replay.drives_recorded_media());
        assert!(replay.matches_timeline_subject(Some("take-1")));
        assert!(!replay.matches_timeline_subject(Some("take-2")));
        assert!(!replay.matches_timeline_subject(None));
        replay.set_transport(ReplayTransport::new(
            ReplayPurpose::Timeline,
            ReplayPhase::Playing,
            true,
        ));
        assert!(!replay.drives_recorded_media());
    }
}
