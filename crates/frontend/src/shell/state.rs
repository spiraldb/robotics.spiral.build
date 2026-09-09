//! Project-scoped connection, clock, and notifications shared by workspaces.
use crate::shell::notifications::{
    MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};
use crate::shell::project::ProjectSessionSignals;
use dioxus::prelude::*;
use std::future::Future;
/// Project lifetime for connection status and notifications shared by routes.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct StudioProjectState {
    backend: ProjectBackendSignals,
    notifications: Signal<NotificationCenterState>,
}

#[derive(Clone, Copy, PartialEq)]
struct ProjectBackendSignals {
    live: Memo<bool>,
    clock_ms: Signal<f64>,
}

impl StudioProjectState {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn now_ms() -> f64 {
        js_sys::Date::now()
    }
    pub(crate) fn live(self) -> Memo<bool> {
        self.backend.live
    }
    pub(crate) const fn clock_ms(self) -> Signal<f64> {
        self.backend.clock_ms
    }
    pub(crate) const fn notifications(self) -> Signal<NotificationCenterState> {
        self.notifications
    }
    pub(crate) fn run_safety<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }
    pub(crate) fn run_utility<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }
    fn run_inner<F>(self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let success = success.into();
        // Accepted commands belong to the project and report through its notifications.
        crate::client::project::ProjectClient::current().spawn(async move {
            match future.await {
                Ok(()) if success.is_empty() => {}
                Ok(()) => self.report_success(success),
                Err(error) => self.report(error),
            }
        });
    }
    pub(crate) fn report(&self, message: impl Into<String>) {
        self.report_from(MessageSource::Studio, message);
    }
    pub(crate) fn report_success(&self, message: impl Into<String>) {
        self.report_success_from(MessageSource::Studio, message);
    }
    pub(crate) fn report_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(message, MessageSeverity::Error, source));
    }
    pub(crate) fn report_success_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(
            message,
            MessageSeverity::Success,
            source,
        ));
    }
    pub(crate) fn report_info_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(message, MessageSeverity::Info, source));
    }
    pub(crate) fn report_warning_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(
            message,
            MessageSeverity::Warning,
            source,
        ));
    }
    fn notify(&self, notice: MessageNotice) {
        let mut notifications = self.notifications;
        notifications.write().push(notice, Self::now_ms());
    }
}
impl StudioProjectState {
    pub(crate) fn use_provider() -> Self {
        let sessions = ProjectSessionSignals::current();
        let state = Self {
            backend: ProjectBackendSignals {
                live: use_memo(move || sessions.connected()),
                clock_ms: use_signal(|| 0.0),
            },
            notifications: use_signal(NotificationCenterState::default),
        };
        use_context_provider(|| state)
    }

    pub(crate) fn use_polling(mut self) {
        use_future(move || async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(250).await;
                self.backend.clock_ms.set(Self::now_ms());
            }
        });
    }

    pub(crate) fn close_notifications(mut self) {
        self.notifications.write().close();
    }
}
