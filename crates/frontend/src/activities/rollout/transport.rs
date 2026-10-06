//! What Rollout's Timeline controls do, and how the recorded frames behind
//! them are asked for.
//!
//! While a take records, the elapsed time arrives from the server; once it is
//! paused, this workspace previews the recorded frames on its own clock.
//! [`TakeTransport`] turns a press on a shared control into the one recording
//! or preview consequence the current phase implies.

use dioxus::prelude::*;
use spiral_robotics_common::recording::{EpisodeInfo, RecordingProgress};

use super::recording_actions::RecordingCompletion;
use super::recording_review::{
    EpisodePlaybackTime, EpisodePreview, EpisodePreviewRequest, EpisodePreviewSignals,
};
use super::session::RolloutSession;
use super::timeline::TimelineSubject;
use crate::components::timeline::{KeyboardFocus, SeekPhase};
use crate::panels::recorded_source::RecordedSource;

/// Durable Timeline state for as long as the Rollout workspace is mounted.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TimelineController {
    playhead: Signal<f64>,
    preview: Signal<Option<(String, f64)>>,
    follow_live: Signal<bool>,
}

impl TimelineController {
    pub(super) fn use_controller() -> Self {
        use_hook(|| Self {
            playhead: Signal::new(0.0),
            preview: Signal::new(None),
            follow_live: Signal::new(true),
        })
    }

    pub(super) const fn playhead_signal(self) -> Signal<f64> {
        self.playhead
    }

    pub(super) const fn follow_live_signal(self) -> Signal<bool> {
        self.follow_live
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

    /// Whether this workspace's own preview is driving the clock.
    pub(super) fn previewing(self) -> bool {
        self.preview.read().is_some()
    }
}

/// The take's clock as one render sees it: where the playhead is, whether the
/// preview is driving it, and whether it follows the newest recorded sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TakeClock {
    seconds: f64,
    previewing: bool,
    follow_live: bool,
}

impl TakeClock {
    pub(super) const fn new(seconds: f64, previewing: bool) -> Self {
        Self {
            seconds,
            previewing,
            follow_live: false,
        }
    }

    pub(super) const fn with_follow_live(mut self, follow_live: bool) -> Self {
        self.follow_live = follow_live;
        self
    }
}

/// The recorded frames behind the Timeline, and the two throttles that keep a
/// scrubbing playhead from asking for one snapshot per rendered frame.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct TakePreview {
    session: RolloutSession,
    signals: EpisodePreviewSignals,
    sampled: Signal<Option<(String, u64)>>,
    interactive: Signal<Option<(String, u64)>>,
}

impl TakePreview {
    pub(super) fn use_preview(session: RolloutSession) -> Self {
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

    pub(super) const fn session(self) -> RolloutSession {
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
        let target = (episode.clone(), seconds.render_slot());
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
        let target = (episode.to_owned(), seconds.render_slot());
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
/// rendered for. The subject travels with the verbs, so each verb states its
/// own consequence and the only check left is that the recording has not moved
/// on since the render that drew the control.
#[derive(Clone, PartialEq)]
pub(super) struct TakeTransport {
    preview: TakePreview,
    subject: TimelineSubject,
    clock: TakeClock,
}

impl TakeTransport {
    pub(super) fn new(preview: TakePreview, subject: TimelineSubject, clock: TakeClock) -> Self {
        Self {
            preview,
            subject,
            clock,
        }
    }

    pub(super) const fn subject(&self) -> &TimelineSubject {
        &self.subject
    }

    pub(super) const fn seconds(&self) -> f64 {
        self.clock.seconds
    }

    /// Whether the preview is advancing the playhead.
    pub(super) const fn playing(&self) -> bool {
        self.clock.previewing
    }

    pub(super) const fn follow_live(&self) -> bool {
        self.clock.follow_live
    }

    /// Why recording or scrubbing is unavailable, if it is.
    pub(super) fn record_blocker(&self) -> Option<String> {
        self.session().recording_blocker()
    }

    /// What the Record button says it starts.
    pub(super) fn record_label(&self) -> &'static str {
        self.session().record_driver().record_label()
    }

    /// Whether Record is waiting for its driver to take the follower.
    pub(super) fn starting(&self) -> bool {
        self.session().next_take().starting()
    }

    /// Whether the take on the clock can pause: teleoperation can, a
    /// policy's run cannot.
    pub(super) fn pausable(&self) -> bool {
        !matches!(
            self.session().telemetry().mode(),
            spiral_robotics_common::robots::ModeSelection::Policy { .. }
        )
    }

    pub(super) fn seek_blocker(&self) -> Option<String> {
        self.subject.preview_blocker()
    }

    const fn session(&self) -> RolloutSession {
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
        if self.clock.previewing {
            controller.stop_preview();
        } else {
            controller.start_preview(episode, subject.duration_seconds());
            follow_live.set(false);
        }
    }

    pub(super) fn seek(&self, seconds: f64, phase: SeekPhase) {
        let Some(subject) = self.subject_now() else {
            return;
        };
        if subject.episode_id().is_none() {
            return;
        }
        let controller = self.controller();
        controller.stop_preview();
        controller.playhead_signal().set(seconds);
        if let Some(episode) = subject.recorded_episode() {
            self.preview
                .request_scrub(episode, seconds, phase == SeekPhase::Commit);
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
                    self.session()
                        .project()
                        .report_rollout_info("Press Shift+X to discard this take.");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(f64::NAN.render_slot(), 0);
        assert_eq!(0.0_f64.render_slot(), 0);
        assert_eq!(0.032_f64.render_slot(), 0);
        assert_eq!(0.034_f64.render_slot(), 1);
        assert_eq!(1.249_f64.render_slot(), 37);
    }
}
