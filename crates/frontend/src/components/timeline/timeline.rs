use super::playback::{PLAYBACK_SPEEDS, ViewWindow};
use super::{KeyboardFocus, SeekPhase, TimelineAction, TimelineInteraction};
use crate::components::{
    icons::*,
    interaction::{Action, ActionPurpose, ActionVariant, Toggle},
};
use crate::format::PreciseClockPresentation;
use dioxus::{html::geometry::WheelDelta, prelude::*};

/// Slots supply activity-specific content without replacing timeline controls.
#[derive(Clone, PartialEq)]
pub(crate) struct TimelineContent {
    identity: Element,
    toolbar: Element,
    corner: Element,
    notice: Element,
    rows: Element,
}
impl TimelineContent {
    pub(crate) fn new(rows: Element) -> Self {
        Self {
            rows,
            identity: rsx! { "Timeline" },
            toolbar: rsx! {},
            corner: rsx! { "Tracks" },
            notice: rsx! {},
        }
    }
    pub(crate) fn with_identity(mut self, identity: Element) -> Self {
        self.identity = identity;
        self
    }
    pub(crate) fn with_toolbar(mut self, toolbar: Element) -> Self {
        self.toolbar = toolbar;
        self
    }
    pub(crate) fn with_corner(mut self, corner: Element) -> Self {
        self.corner = corner;
        self
    }
    pub(crate) fn with_notice(mut self, notice: Element) -> Self {
        self.notice = notice;
        self
    }
}

#[component]
pub(crate) fn Timeline(interaction: TimelineInteraction, content: TimelineContent) -> Element {
    let state = interaction.state();
    let view = state.window();
    let percent = view.percent(state.clock().seconds());
    let keyboard = interaction.clone();
    let wheel = interaction.clone();
    rsx! {
        section { class: "data-timeline", "aria-label": "Timeline", tabindex: "0",
            onkeydown: move |event: KeyboardEvent| {
                if !KeyboardFocus::editing() && let Some(action) = keyboard.state().key_action((&event).into()) {
                    keyboard.emit(action);
                    event.prevent_default(); event.stop_propagation();
                }
            },
            TransportBar { interaction: interaction.clone(), identity: content.identity, toolbar: content.toolbar }
            {content.notice}
            div { class: "data-timeline-body", style: "--playhead: {percent / 100.0:.6}",
                onwheel: move |event: WheelEvent| {
                    let (dx, dy) = match event.delta() {
                        WheelDelta::Pixels(delta) => (delta.x, delta.y),
                        WheelDelta::Lines(delta) => (delta.x * 16.0, delta.y * 16.0),
                        WheelDelta::Pages(delta) => (delta.x * 400.0, delta.y * 400.0),
                    };
                    let state = wheel.state();
                    if event.modifiers().ctrl() || event.modifiers().meta() {
                        event.prevent_default();
                        wheel.emit(state.zoom(if dy > 0.0 { 1.25 } else { 0.8 }));
                    } else if event.modifiers().shift() || dx.abs() > dy.abs() {
                        event.prevent_default();
                        let delta = if event.modifiers().shift() { dy } else { dx };
                        wheel.emit(TimelineAction::SetWindow(state.window().panned(delta / 800.0 * state.window().span(), state.clock().duration())));
                    }
                },
                div { class: "data-timeline-ruler",
                    div { class: "data-timeline-corner", {content.corner} }
                    div { class: "data-timeline-scale",
                        if state.clock().duration() > 0.0 {
                            for tick in view.ticks() {
                                div { key: "{tick.seconds()}", class: "data-time-tick", style: "left: {tick.position():.4}%",
                                    "data-edge": if tick.position() > 90.0 { "end" } else { "start" }, span { "{tick.label()}" } }
                            }
                        }
                        TimelineScrubber { interaction: interaction.clone(), labelled: true }
                        div { class: "data-timeline-handle", style: "left: {percent:.4}%", "aria-hidden": "true" }
                    }
                }
                div { class: "data-timeline-rows",
                    {content.rows}
                    div { class: "data-timeline-cursor", "aria-hidden": "true" }
                }
            }
        }
    }
}

#[component]
fn TransportBar(interaction: TimelineInteraction, identity: Element, toolbar: Element) -> Element {
    let state = interaction.state();
    let clock = state.clock();
    let duration = clock.duration();
    let seconds = clock.seconds();
    let playing = clock.playing();
    let view = state.window();
    let fitted = view.start() <= 0.0 && view.end() >= duration;
    let step = interaction.clone();
    let play = interaction.clone();
    let speed = interaction.clone();
    let looping = interaction.clone();
    let zoom = interaction.clone();
    rsx! {
        header { class: "data-transport", role: "toolbar", "aria-label": "Playback",
            div { class: "data-transport-identity", {identity} }
            {toolbar}
            div { class: "data-transport-group", role: "group", "aria-label": "Transport",
                for (label, title, time, icon) in [
                    ("Jump to start", "Jump to start · Home", 0.0, rsx! { SkipToStartIcon {} }),
                    ("Step back", "Back 0.1 s · ← (Shift 1 s, Alt 10 ms)", seconds - 0.1, rsx! { StepBackIcon {} }),
                ] {
                    Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !state.can_seek(), title: state.seek_reason().unwrap_or(title), aria_label: label,
                        onclick: { let step = step.clone(); move |_| step.emit(TimelineAction::Seek { seconds: time, phase: SeekPhase::Commit }) }, {icon} }
                }
                Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Icon, class: "data-transport-play", disabled: !state.can_play(),
                    title: state.play_reason().unwrap_or(if playing { "Pause · Space" } else { "Play · Space" }),
                    aria_label: if playing { "Pause" } else { "Play" }, aria_pressed: playing,
                    onclick: move |_| play.emit(TimelineAction::SetPlaying(!playing)),
                    if playing { PauseIcon {} } else { PlayIcon {} } }
                for (label, title, time, icon) in [
                    ("Step forward", "Forward 0.1 s · → (Shift 1 s, Alt 10 ms)", seconds + 0.1, rsx! { StepForwardIcon {} }),
                    ("Jump to end", "Jump to end · End", duration, rsx! { SkipToEndIcon {} }),
                ] {
                    Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: !state.can_seek(), title: state.seek_reason().unwrap_or(title), aria_label: label,
                        onclick: { let step = step.clone(); move |_| step.emit(TimelineAction::Seek { seconds: time, phase: SeekPhase::Commit }) }, {icon} }
                }
            }
            output { class: "data-transport-clock", "aria-live": "off",
                span { "{seconds.precise_clock_label()}" }
                span { class: "data-transport-total", "/ {duration.precise_clock_label()}" }
            }
            if state.playback_options() {
                label { class: "data-transport-speed", title: "Playback speed",
                    span { class: "sr-only", "Speed" }
                    select { value: "{clock.speed()}", disabled: duration <= 0.0, "aria-label": "Playback speed",
                        onchange: move |event| if let Ok(value) = event.value().parse::<f64>() { speed.emit(TimelineAction::SetSpeed(value)); },
                        for option in PLAYBACK_SPEEDS { option { value: "{option}", selected: option == clock.speed(), "{option}×" } }
                    }
                }
                Toggle { label: "Loop".to_owned(), checked: clock.looping(), disabled: duration <= 0.0, class: "data-transport-loop".to_owned(),
                    title: "Start over when the episode ends".to_owned(), onchange: move |_| looping.emit(TimelineAction::SetLooping(!clock.looping())) }
            }
            span { class: "data-transport-note", role: "status", if state.buffering() { "Buffering…" } }
            div { class: "data-transport-group data-transport-zoom", role: "group", "aria-label": "Time scale",
                for (label, title, action, icon, disabled) in [
                    ("Zoom out", "Zoom out · −", state.zoom(2.0), rsx! { ZoomOutIcon {} }, fitted),
                    ("Zoom in", "Zoom in · +", state.zoom(0.5), rsx! { ZoomInIcon {} }, false),
                    ("Fit episode", "Fit the whole episode · 0", TimelineAction::SetWindow(ViewWindow::full(duration)), rsx! { FitTimeIcon {} }, fitted),
                ] {
                    Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, disabled: duration <= 0.0 || disabled, title, aria_label: label,
                        onclick: { let zoom = zoom.clone(); move |_| zoom.emit(action.clone()) }, {icon} }
                }
            }
        }
    }
}

/// Native range input shared by the ruler and every track.
#[component]
pub(crate) fn TimelineScrubber(
    interaction: TimelineInteraction,
    #[props(default)] labelled: bool,
) -> Element {
    let mut gesture = use_signal(ScrubGesture::default);
    let state = interaction.state();
    let view = state.window();
    let seconds = gesture.read().seconds(state);
    let input = interaction.clone();
    let commit = interaction.clone();
    rsx! {
        input { r#type: "range", class: if labelled { "data-time-scrub" } else { "data-track-scrub" },
            tabindex: if labelled { "0" } else { "-1" }, "aria-hidden": (!labelled).then_some("true"),
            "aria-label": "Playhead", "aria-valuetext": "{seconds.precise_clock_label()} of {state.clock().duration().precise_clock_label()}",
            title: state.seek_reason().unwrap_or("Drag or use the keyboard to seek"), disabled: !state.can_seek(),
            min: "{view.start()}", max: "{view.end()}", step: "0.001", value: "{seconds}",
            oninput: move |event| if let Ok(seconds) = event.value().parse::<f64>() {
                let action = gesture.write().preview(input.state(), seconds);
                input.emit(action);
            },
            onchange: move |_| {
                let action = gesture.write().finish(commit.state(), SeekPhase::Commit);
                if let Some(action) = action { commit.emit(action); }
            },
            onpointercancel: move |_| {
                let action = gesture.write().finish(interaction.state(), SeekPhase::Cancel);
                if let Some(action) = action { interaction.emit(action); }
            },
        }
    }
}

#[derive(Default)]
struct ScrubGesture {
    origin: Option<(String, f64)>,
    draft: f64,
}
impl ScrubGesture {
    fn seconds(&self, state: &super::TimelineState) -> f64 {
        if self
            .origin
            .as_ref()
            .is_some_and(|(target, _)| target == state.target())
        {
            self.draft
        } else {
            state.clock().seconds()
        }
    }
    fn preview(&mut self, state: &super::TimelineState, seconds: f64) -> TimelineAction {
        if self
            .origin
            .as_ref()
            .is_none_or(|(target, _)| target != state.target())
        {
            self.origin = Some((state.target().to_owned(), state.clock().seconds()));
        }
        self.draft = seconds;
        TimelineAction::Seek {
            seconds,
            phase: SeekPhase::Preview,
        }
    }
    fn finish(&mut self, state: &super::TimelineState, phase: SeekPhase) -> Option<TimelineAction> {
        let (target, original) = self.origin.take()?;
        (target == state.target()).then_some(TimelineAction::Seek {
            seconds: if phase == SeekPhase::Cancel {
                original
            } else {
                self.draft
            },
            phase,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::timeline::{PlaybackClock, TimelineState};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn scrub_commits_once_cancels_to_origin_and_does_not_cross_sources() {
        let mut clock = PlaybackClock::new(10.0);
        clock.seek(2.0);
        let state = TimelineState::new("first", clock);
        let mut gesture = ScrubGesture::default();
        assert_eq!(
            gesture.preview(&state, 4.0),
            TimelineAction::Seek {
                seconds: 4.0,
                phase: SeekPhase::Preview
            }
        );
        gesture.preview(&state, 6.0);
        assert_eq!(
            gesture.finish(&state, SeekPhase::Cancel),
            Some(TimelineAction::Seek {
                seconds: 2.0,
                phase: SeekPhase::Cancel
            })
        );
        gesture.preview(&state, 5.0);
        assert_eq!(
            gesture.finish(&state, SeekPhase::Commit),
            Some(TimelineAction::Seek {
                seconds: 5.0,
                phase: SeekPhase::Commit
            })
        );
        assert_eq!(gesture.finish(&state, SeekPhase::Commit), None);
        gesture.preview(&state, 7.0);
        let next = TimelineState::new("second", PlaybackClock::new(10.0));
        assert_eq!(gesture.seconds(&next), 0.0);
        assert_eq!(gesture.finish(&next, SeekPhase::Commit), None);
    }
}
