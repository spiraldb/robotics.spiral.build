//! Application-wide messages and notification-center behavior.
//!
//! Feature panels own durable domain state. Short-lived command outcomes and
//! recoverable failures belong here instead, so every screen reports through
//! the same status-bar surface and messages survive route changes.

use std::collections::VecDeque;

/// Keep enough history to diagnose a session without allowing a noisy backend
/// to grow browser memory forever.
const MAX_MESSAGES: usize = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MessageSeverity {
    Info,
    Success,
    Warning,
    Error,
}

impl MessageSeverity {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Success => "Success",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Info => "message-info",
            Self::Success => "message-success",
            Self::Warning => "message-warning",
            Self::Error => "message-error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MessageSource {
    Studio,
    Live,
    Data,
    Training,
    Rollout,
    Settings,
}

impl MessageSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Live => "Live",
            Self::Data => "Data",
            Self::Training => "Training",
            Self::Rollout => "Rollout",
            Self::Settings => "Settings",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MessageAction {
    OpenEpisode(String),
}

impl MessageAction {
    pub(crate) const fn label(&self) -> &'static str {
        match self {
            Self::OpenEpisode(_) => "View episode",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudioMessage {
    pub id: u64,
    pub text: String,
    pub severity: MessageSeverity,
    pub source: MessageSource,
    pub raised_ms: f64,
    /// How long this message receives space in the status bar. It remains in
    /// history after this deadline until the operator dismisses it.
    pub prominent_until_ms: f64,
    pub read: bool,
    pub action: Option<MessageAction>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NotificationCenterState {
    messages: VecDeque<StudioMessage>,
    next_id: u64,
    open: bool,
}

impl NotificationCenterState {
    pub(crate) fn push(
        &mut self,
        text: impl Into<String>,
        severity: MessageSeverity,
        source: MessageSource,
        action: Option<MessageAction>,
        now_ms: f64,
    ) -> u64 {
        let text = text.into();
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let prominent_until_ms = now_ms + f64::from(linger_ms(&text, severity));
        self.messages.push_back(StudioMessage {
            id,
            text,
            severity,
            source,
            raised_ms: now_ms,
            prominent_until_ms,
            read: self.open,
            action,
        });
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        id
    }

    pub(crate) fn messages(&self) -> impl DoubleEndedIterator<Item = &StudioMessage> {
        self.messages.iter()
    }

    pub(crate) fn latest_prominent(&self, now_ms: f64) -> Option<&StudioMessage> {
        self.messages
            .iter()
            .rev()
            .find(|message| now_ms < message.prominent_until_ms)
    }

    pub(crate) fn unread_count(&self) -> usize {
        self.messages.iter().filter(|message| !message.read).count()
    }

    pub(crate) const fn open(&self) -> bool {
        self.open
    }

    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            for message in &mut self.messages {
                message.read = true;
            }
        }
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    pub(crate) fn dismiss(&mut self, id: u64) {
        self.messages.retain(|message| message.id != id);
    }

    pub(crate) fn clear(&mut self) {
        self.messages.clear();
    }
}

fn linger_ms(text: &str, severity: MessageSeverity) -> u32 {
    let reading = 3_500 + (text.chars().count() as u32) * 55;
    let floor = match severity {
        MessageSeverity::Error | MessageSeverity::Warning => 8_000,
        MessageSeverity::Info | MessageSeverity::Success => 4_000,
    };
    reading.clamp(floor, 14_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_expire_from_the_status_bar_but_remain_in_history() {
        let mut center = NotificationCenterState::default();
        center.push(
            "Episode saved.",
            MessageSeverity::Success,
            MessageSource::Live,
            None,
            100.0,
        );
        let deadline = center.messages().next().unwrap().prominent_until_ms;

        assert!(center.latest_prominent(deadline - 1.0).is_some());
        assert!(center.latest_prominent(deadline).is_none());
        assert_eq!(center.messages().count(), 1);
    }

    #[test]
    fn opening_the_center_marks_every_visible_message_read() {
        let mut center = NotificationCenterState::default();
        center.push(
            "Failed.",
            MessageSeverity::Error,
            MessageSource::Training,
            None,
            0.0,
        );
        assert_eq!(center.unread_count(), 1);

        center.toggle();

        assert!(center.open());
        assert_eq!(center.unread_count(), 0);
    }

    #[test]
    fn history_is_bounded() {
        let mut center = NotificationCenterState::default();
        for index in 0..MAX_MESSAGES + 5 {
            center.push(
                format!("Message {index}"),
                MessageSeverity::Info,
                MessageSource::Studio,
                None,
                index as f64,
            );
        }
        assert_eq!(center.messages().count(), MAX_MESSAGES);
    }
}
