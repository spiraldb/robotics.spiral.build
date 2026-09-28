//! Project-scoped clock and notifications shared by workspaces.
use crate::shell::notifications::{
    MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};
use dioxus::dioxus_core::CapturedError;
use dioxus::prelude::*;
use std::future::Future;
/// Project lifetime for the clock and notifications shared by routes.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct StudioProjectState {
    clock_ms: Signal<f64>,
    notifications: Signal<NotificationCenterState>,
}

impl StudioProjectState {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    fn now_ms() -> f64 {
        js_sys::Date::now()
    }
    pub(crate) const fn clock_ms(self) -> Signal<f64> {
        self.clock_ms
    }
    pub(super) const fn notifications(self) -> Signal<NotificationCenterState> {
        self.notifications
    }
    /// Run a command on the project and report its outcome, `success` when
    /// it succeeds.
    pub(crate) fn run_reported<F>(self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let success = success.into();
        // Accepted commands belong to the project and report through its notifications.
        crate::client::project::ProjectClient::current().spawn(async move {
            let _ = self.execute_utility(success, future).await;
        });
    }
    /// Report an operation's outcome while its caller owns the task lifecycle.
    pub(crate) async fn execute_utility(
        self,
        success: impl Into<String>,
        future: impl Future<Output = Result<(), String>>,
    ) -> Result<(), CapturedError> {
        let success = success.into();
        match future.await {
            Ok(()) => {
                if !success.is_empty() {
                    self.report_success(success);
                }
                Ok(())
            }
            Err(error) => {
                self.report(&error);
                Err(CapturedError::from_display(error))
            }
        }
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
    pub(crate) fn report_live_info(&self, message: impl Into<String>) {
        self.notify(MessageNotice::new(
            message,
            MessageSeverity::Info,
            MessageSource::Live,
        ));
    }
    fn notify(&self, notice: MessageNotice) {
        let mut notifications = self.notifications;
        notifications.write().push(notice, Self::now_ms());
    }
}
impl StudioProjectState {
    pub(crate) fn use_provider() -> Self {
        let state = Self {
            clock_ms: use_signal(|| 0.0),
            notifications: use_signal(NotificationCenterState::default),
        };
        use_context_provider(|| state)
    }

    pub(super) fn use_polling(mut self) {
        use_future(move || async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(250).await;
                self.clock_ms.set(Self::now_ms());
            }
        });
    }

    pub(super) fn close_notifications(mut self) {
        self.notifications.write().close();
    }
}
