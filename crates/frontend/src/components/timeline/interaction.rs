use super::tracks::TimelineInterval;
use super::{PlaybackClock, ViewWindow};
use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SeekPhase {
    Preview,
    Commit,
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimelineAction {
    Seek { seconds: f64, phase: SeekPhase },
    SetPlaying(bool),
    SetSpeed(f64),
    SetLooping(bool),
    SetWindow(ViewWindow),
    ActivateInterval(TimelineInterval),
}

/// An interaction stays attached to the source that rendered it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineEvent {
    target: String,
    action: TimelineAction,
}
impl TimelineEvent {
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
    pub(crate) fn action(self) -> TimelineAction {
        self.action
    }
}

#[derive(Clone, Default, PartialEq)]
struct TimelineControls {
    seek_blocked: Option<String>,
    play_blocked: Option<String>,
    external_playback: bool,
    buffering: bool,
}

/// Read-only projection of a caller-owned clock. Backend-driven playback uses
/// the same projection, with speed/loop controls omitted.
#[derive(Clone, PartialEq)]
pub(crate) struct TimelineState {
    target: String,
    clock: PlaybackClock,
    window: ViewWindow,
    controls: TimelineControls,
}
impl TimelineState {
    pub(crate) fn new(target: impl Into<String>, clock: PlaybackClock) -> Self {
        Self {
            target: target.into(),
            window: ViewWindow::full(clock.duration()),
            clock,
            controls: TimelineControls::default(),
        }
    }
    pub(crate) fn with_window(mut self, window: ViewWindow) -> Self {
        self.window = window;
        self
    }
    pub(crate) fn with_buffering(mut self, buffering: bool) -> Self {
        self.controls.buffering = buffering;
        self
    }
    pub(crate) fn with_external_playback(mut self) -> Self {
        self.controls.external_playback = true;
        self
    }
    pub(crate) fn with_blocked_seek(mut self, reason: Option<String>) -> Self {
        self.controls.seek_blocked = reason;
        self
    }
    pub(crate) fn with_blocked_play(mut self, reason: Option<String>) -> Self {
        self.controls.play_blocked = reason;
        self
    }
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
    pub(crate) fn clock(&self) -> PlaybackClock {
        self.clock
    }
    pub(crate) fn window(&self) -> ViewWindow {
        self.window
    }
    pub(super) fn seek_reason(&self) -> Option<&str> {
        self.controls.seek_blocked.as_deref()
    }
    pub(super) fn play_reason(&self) -> Option<&str> {
        self.controls.play_blocked.as_deref()
    }
    pub(super) fn playback_options(&self) -> bool {
        !self.controls.external_playback
    }
    pub(super) fn buffering(&self) -> bool {
        self.controls.buffering
    }
    pub(super) fn can_seek(&self) -> bool {
        self.clock.duration() > 0.0 && self.seek_reason().is_none()
    }
    pub(super) fn can_play(&self) -> bool {
        self.clock.duration() > 0.0 && self.play_reason().is_none()
    }
    pub(crate) fn key_action(&self, key: TransportKey) -> Option<TimelineAction> {
        if self.clock.duration() <= 0.0 || key.command {
            return None;
        }
        let step = if key.shift {
            1.0
        } else if key.alt {
            0.01
        } else {
            0.1
        };
        let seconds = self.clock.seconds();
        let seek = |seconds| {
            self.can_seek().then_some(TimelineAction::Seek {
                seconds,
                phase: SeekPhase::Commit,
            })
        };
        match key.name.as_str() {
            " " if self.can_play() && !key.repeating && !KeyboardFocus::on_button() => {
                Some(TimelineAction::SetPlaying(!self.clock.playing()))
            }
            "ArrowLeft" => seek(seconds - step),
            "ArrowRight" => seek(seconds + step),
            "Home" => seek(0.0),
            "End" => seek(self.clock.duration()),
            "+" | "=" => Some(self.zoom(0.5)),
            "-" => Some(self.zoom(2.0)),
            "0" => Some(TimelineAction::SetWindow(ViewWindow::full(
                self.clock.duration(),
            ))),
            _ => None,
        }
    }
    pub(super) fn zoom(&self, factor: f64) -> TimelineAction {
        TimelineAction::SetWindow(self.window.zoomed(
            factor,
            self.clock.seconds(),
            self.clock.duration(),
        ))
    }
}

/// Explicit binding passed to custom rows; no ambient timeline context.
#[derive(Clone, PartialEq)]
pub(crate) struct TimelineInteraction {
    state: TimelineState,
    on_action: EventHandler<TimelineEvent>,
}
impl TimelineInteraction {
    pub(crate) fn new(state: TimelineState, on_action: EventHandler<TimelineEvent>) -> Self {
        Self { state, on_action }
    }
    pub(crate) fn state(&self) -> &TimelineState {
        &self.state
    }
    pub(crate) fn emit(&self, action: TimelineAction) {
        let action = match action {
            TimelineAction::Seek { seconds, phase }
                if self.state.can_seek() && seconds.is_finite() =>
            {
                TimelineAction::Seek {
                    seconds: seconds.clamp(0.0, self.state.clock.duration()),
                    phase,
                }
            }
            TimelineAction::Seek { .. } => return,
            TimelineAction::SetPlaying(_) if !self.state.can_play() => return,
            action => action,
        };
        self.on_action.call(TimelineEvent {
            target: self.state.target.clone(),
            action,
        });
    }
}

/// Transport shortcuts must not steal keys from text entry or menus.
pub(crate) struct KeyboardFocus;
impl KeyboardFocus {
    fn active_tag() -> Option<(String, Option<String>)> {
        let element = web_sys::window()?.document()?.active_element()?;
        if element.get_attribute("contenteditable").is_some() {
            return Some(("CONTENTEDITABLE".into(), None));
        }
        Some((element.tag_name(), element.get_attribute("type")))
    }
    pub(crate) fn editing() -> bool {
        Self::active_tag().is_some_and(|(tag, kind)| {
            matches!(
                tag.as_str(),
                "INPUT" | "SELECT" | "TEXTAREA" | "SUMMARY" | "CONTENTEDITABLE"
            ) && kind.as_deref() != Some("range")
        })
    }
    /// A focused button already answers Space with a click.
    fn on_button() -> bool {
        Self::active_tag().is_some_and(|(tag, _)| tag == "BUTTON")
    }
}

/// One key press as the transport reads it: the DOM key name plus the
/// modifiers that scale a step. Built from a workspace event or from a
/// listener on the document, so both paths share one vocabulary.
pub(crate) struct TransportKey {
    name: String,
    shift: bool,
    alt: bool,
    repeating: bool,
    command: bool,
}

impl From<&KeyboardEvent> for TransportKey {
    fn from(event: &KeyboardEvent) -> Self {
        let modifiers = event.modifiers();
        Self {
            name: event.key().to_string(),
            shift: modifiers.shift(),
            alt: modifiers.alt(),
            repeating: event.is_auto_repeating(),
            command: modifiers.ctrl() || modifiers.meta(),
        }
    }
}

impl From<&web_sys::KeyboardEvent> for TransportKey {
    fn from(event: &web_sys::KeyboardEvent) -> Self {
        Self {
            name: event.key(),
            shift: event.shift_key(),
            alt: event.alt_key(),
            repeating: event.repeat(),
            command: event.ctrl_key() || event.meta_key(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn keyboard_respects_gates_modifiers_and_shared_time_window() {
        let state = TimelineState::new("episode", PlaybackClock::new(10.0));
        let mut key = TransportKey {
            name: "ArrowRight".into(),
            shift: true,
            alt: false,
            repeating: false,
            command: false,
        };
        assert_eq!(
            state.key_action(key),
            Some(TimelineAction::Seek {
                seconds: 1.0,
                phase: SeekPhase::Commit
            })
        );
        key = TransportKey {
            name: "ArrowRight".into(),
            shift: false,
            alt: true,
            repeating: false,
            command: false,
        };
        assert_eq!(
            state.key_action(key),
            Some(TimelineAction::Seek {
                seconds: 0.01,
                phase: SeekPhase::Commit
            })
        );
        let blocked = state.with_blocked_seek(Some("Recording".into()));
        key = TransportKey {
            name: "Home".into(),
            shift: false,
            alt: false,
            repeating: false,
            command: false,
        };
        assert!(blocked.key_action(key).is_none());
        key = TransportKey {
            name: "+".into(),
            shift: false,
            alt: false,
            repeating: false,
            command: true,
        };
        assert!(blocked.key_action(key).is_none());
    }
}
