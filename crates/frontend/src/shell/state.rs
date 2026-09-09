//! State shared across workspaces: project configuration, connection, and notifications.
use crate::activities::settings::SettingsClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::shell::notifications::{
    MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};
use crate::shell::{StudioClient, project::ProjectSessionSignals};
use dioxus::prelude::*;
use spiral_robotics_common::robots::{ModeSelection, RobotSummary};
use spiral_robotics_common::settings::DataSettings;
use spiral_robotics_common::status::StudioSnapshot;
use std::future::Future;
/// Project lifetime: connection, configuration, and notifications shared by routes.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct StudioProjectState {
    backend: ProjectBackendSignals,
    resources: StudioProjectResources,
    notifications: Signal<NotificationCenterState>,
}

#[derive(Clone, Copy, PartialEq)]
struct ProjectBackendSignals {
    snapshot: Signal<StudioSnapshot>,
    live: Memo<bool>,
    clock_ms: Signal<f64>,
}

#[derive(Clone, Copy, PartialEq)]
struct StudioProjectResources {
    settings: Resource<Result<DataSettings, String>>,
    loaded_settings: Signal<Option<DataSettings>>,
    configuration: Memo<StudioConfiguration>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StudioConfiguration {
    spiral_query_available: bool,
}

impl StudioConfiguration {
    pub(crate) const fn spiral_query_available(&self) -> bool {
        self.spiral_query_available
    }
}

impl StudioProjectResources {
    fn use_resources() -> Self {
        let settings = QueryPolicy::new(QueryDomain::Settings, 10_000).use_resource(|| async {
            SettingsClient::current()
                .data_settings()
                .await
                .map_err(|error| error.to_string())
        });
        let mut loaded_settings = use_signal(|| None::<DataSettings>);
        use_effect(move || {
            let result = settings.read();
            if let Some(Ok(value)) = result.as_ref()
                && loaded_settings.peek().as_ref() != Some(value)
            {
                loaded_settings.set(Some(value.clone()));
            }
        });
        let loaded_configuration = loaded_settings.read().to_owned();
        let configuration = use_memo(use_reactive((&loaded_configuration,), move |(settings,)| {
            StudioConfiguration {
                spiral_query_available: settings
                    .as_ref()
                    .is_some_and(|settings| !settings.spiral().project_id().trim().is_empty()),
            }
        }));
        Self {
            settings,
            loaded_settings,
            configuration,
        }
    }

    fn current_configuration(&self) -> StudioConfiguration {
        (self.configuration)()
    }
}

impl StudioProjectState {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn now_ms() -> f64 {
        js_sys::Date::now()
    }
    pub(crate) fn snapshot(self) -> MappedSignal<StudioSnapshot, Signal<StudioSnapshot>> {
        self.backend.snapshot.map(|value| value)
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
    pub(crate) fn configuration(&self) -> StudioConfiguration {
        self.resources.current_configuration()
    }
    pub(crate) fn loaded_settings(&self) -> Option<DataSettings> {
        self.resources.loaded_settings.read().to_owned()
    }
    pub(crate) fn settings_result(&self) -> Option<Result<DataSettings, String>> {
        self.resources.settings.read_unchecked().to_owned()
    }
    pub(crate) fn refresh_settings(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Settings]);
    }
    pub(crate) fn connected_robots(&self) -> Vec<RobotSummary> {
        self.backend
            .snapshot
            .read()
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .cloned()
            .collect()
    }
    pub(crate) fn mode(&self) -> ModeSelection {
        self.backend.snapshot.read().mode().selection().to_owned()
    }
    pub(crate) fn mode_active(&self) -> bool {
        self.backend.snapshot.read().mode().active()
    }
    pub(crate) fn is_manual_target(&self, id: &str) -> bool {
        self.mode_active() && matches!(self.mode(), ModeSelection::Manual { robot } if robot == id)
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
                snapshot: use_signal(StudioSnapshot::default),
                live: use_memo(move || sessions.connected()),
                clock_ms: use_signal(|| 0.0),
            },
            resources: StudioProjectResources::use_resources(),
            notifications: use_signal(NotificationCenterState::default),
        };
        use_context_provider(|| state)
    }

    pub(crate) fn use_polling(mut self) {
        let setup = QueryPolicy::new(QueryDomain::Setup, 2_000)
            .use_resource(|| StudioClient::current().setup_snapshot());
        use_effect(move || {
            let result = setup.read();
            if setup.finished()
                && let Some(Ok(value)) = result.as_ref()
                && *self.backend.snapshot.peek() != *value
            {
                self.backend.snapshot.set(value.clone());
            }
        });
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
