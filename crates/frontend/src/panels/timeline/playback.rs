//! Preview clock and replay transport rules, independent of activity and backend access.

use super::panel::TimelinePlayback;
use super::state::TimelineController;
use dioxus::prelude::*;
use spiral_robotics_common::recording::{ReplayPhase, ReplayProgress, ReplayPurpose};

impl TimelineController {
    /// Keep the preview clock mounted, but run it only while its workspace is active.
    pub(crate) fn use_clock(
        self,
        is_active: ReadSignal<bool>,
        on_sample: EventHandler<(String, f64)>,
    ) {
        let mut playhead = self.playhead_signal();
        let mut last_tick = self.preview_tick_signal();
        // Subscribe the owning component so visibility changes run the lifecycle hook.
        let _ = is_active();
        let mut clock = use_future(move || async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(33).await;
                if !self.preview_playing() {
                    last_tick.set(None);
                    continue;
                }
                let Some((episode, duration)) = self.preview_target() else {
                    self.stop_preview();
                    continue;
                };
                let now = js_sys::Date::now() / 1_000.0;
                let clock = PreviewClock::new(duration, last_tick.replace(Some(now)));
                let next = clock.advance(playhead(), now);
                playhead.set(next);
                on_sample.call((episode, next));
                if next >= duration {
                    self.stop_preview();
                }
            }
        });
        // use_future resumes its task after a render; apply workspace visibility afterward.
        use_hook_did_run(move |did_run| {
            if did_run && is_active() {
                clock.resume();
            } else {
                clock.pause();
                last_tick.set(None);
            }
        });
    }

    pub(crate) fn playback(self, replay: Option<&ReplayProgress>) -> TimelinePlayback {
        replay
            .filter(|replay| replay.owns_timeline_transport())
            .map(|replay| {
                if replay.paused() {
                    TimelinePlayback::RobotSyncPaused
                } else if replay.phase() == ReplayPhase::Seeking {
                    TimelinePlayback::RobotSyncSeeking
                } else {
                    TimelinePlayback::RobotSyncPlaying
                }
            })
            .unwrap_or_else(|| {
                if self.preview_playing() {
                    TimelinePlayback::PreviewPlaying
                } else {
                    TimelinePlayback::Stopped
                }
            })
    }
}

/// Bound wall-clock gaps and clamp the final frame to the episode duration.
struct PreviewClock {
    duration: f64,
    previous_tick: Option<f64>,
}

impl PreviewClock {
    fn new(duration: f64, previous_tick: Option<f64>) -> Self {
        Self {
            duration,
            previous_tick,
        }
    }
    fn advance(&self, playhead: f64, now: f64) -> f64 {
        let delta = self
            .previous_tick
            .map(|previous| (now - previous).clamp(0.0, 0.25))
            .unwrap_or_default();
        (playhead + delta).clamp(0.0, self.duration.max(0.0))
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

impl TimelineReplay for spiral_robotics_common::recording::ReplayProgress {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::data::episode_cache::EpisodePlaybackTime;
    use spiral_robotics_common::recording::{ReplayIdentity, ReplayTimeline, ReplayTransport};
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
            crate::shell::view_state::ProjectViewStates::use_provider();
            crate::shell::view_state::ViewState::use_provider(
                crate::shell::activity::Activity::Live,
            );
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
    fn preview_clock_preserves_the_first_frame_and_bounds_delayed_ticks() {
        assert_eq!(PreviewClock::new(10.0, None).advance(2.0, 100.0), 2.0);
        assert_eq!(PreviewClock::new(10.0, Some(1.0)).advance(2.0, 100.0), 2.25);
        assert_eq!(PreviewClock::new(10.0, Some(5.0)).advance(2.0, 4.0), 2.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn preview_clock_samples_the_exact_final_frame() {
        assert_eq!(PreviewClock::new(2.0, Some(1.0)).advance(1.99, 1.2), 2.0);
        assert_eq!(PreviewClock::new(0.0, Some(1.0)).advance(1.0, 1.2), 0.0);
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
        let mut replay = spiral_robotics_common::recording::ReplayProgress::new(
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
