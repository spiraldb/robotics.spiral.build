//! URL routes and the low-frequency Studio shell shared by every screen.

use crate::components::interaction::BrowserFocus;
use dioxus::prelude::*;
use spiral_robotics_api::settings::DataSettings;

use crate::activities::data::state::use_data_controller;
use crate::activities::rollout::state::use_rollout_controller;
use crate::activities::training::state::use_training_controller;
use crate::activities::training::workspace::TrainingWorkspace;
use crate::activities::workflow::state::use_workflow_controller;
use crate::activities::workflow::workspace::WorkflowWorkspace;
use crate::client::http as api;
use crate::panels::timeline::state::use_timeline_controller;
use crate::settings::panel::SettingsPanel;
use crate::shell::activity::Activity;
use crate::shell::frame::{ActivityRail, StatusBar};
use crate::shell::navigation::{ShellAction, use_shell_controller};
use crate::shell::workspace::EpisodeWorkspace;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const ECHARTS_RUNTIME: &str = "https://cdn.jsdelivr.net/npm/echarts@6.1.0/dist/echarts.min.js";
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

#[derive(Clone, Routable, Debug, PartialEq)]
pub(crate) enum StudioRoute {
    #[layout(StudioShell)]
    #[redirect("/", || StudioRoute::Live {})]
    #[route("/live")]
    Live {},
    #[route("/data")]
    Data {},
    #[route("/workflow")]
    Workflow {},
    #[route("/training")]
    Training {},
    #[route("/rollout")]
    Rollout {},
}

impl StudioRoute {
    pub(crate) const fn activity(&self) -> Activity {
        match self {
            Self::Live {} => Activity::Live,
            Self::Data {} => Activity::Data,
            Self::Workflow {} => Activity::Workflow,
            Self::Training {} => Activity::Training,
            Self::Rollout {} => Activity::Rollout,
        }
    }

    pub(crate) const fn for_activity(activity: Activity) -> Self {
        match activity {
            Activity::Live => Self::Live {},
            Activity::Data => Self::Data {},
            Activity::Workflow => Self::Workflow {},
            Activity::Training => Self::Training {},
            Activity::Rollout => Self::Rollout {},
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudioConfiguration {
    dataset: String,
    spiral_query_available: bool,
}

impl Default for StudioConfiguration {
    fn default() -> Self {
        Self {
            dataset: "spiral_robotics".to_owned(),
            spiral_query_available: false,
        }
    }
}

impl StudioConfiguration {
    pub(crate) fn current() -> Self {
        use_context::<Memo<Self>>()()
    }

    pub(crate) fn dataset(&self) -> &str {
        &self.dataset
    }

    pub(crate) const fn spiral_query_available(&self) -> bool {
        self.spiral_query_available
    }
}

#[component]
pub(crate) fn StudioRouter() -> Element {
    rsx! { Router::<StudioRoute> {} }
}

/// Controllers and settings live above the route outlet so drafts, selected
/// runs, messages, and long-running backend commands survive navigation.
#[component]
fn StudioShell() -> Element {
    let studio = crate::shell::state::studio();
    let route = use_route::<StudioRoute>();
    let activity = route.activity();
    let navigator = use_navigator();
    let shell = use_shell_controller();
    let mut settings_busy = use_signal(|| false);
    let mut settings =
        use_resource(|| async { api::Http::get::<DataSettings>("/api/settings/data").await });
    let settings_result = settings.read_unchecked().to_owned();
    let loaded_settings = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    let settings_error = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let configuration = use_memo(use_reactive((&loaded_settings,), move |(settings,)| {
        StudioConfiguration {
            dataset: settings
                .as_ref()
                .map(|settings| settings.spiral().namespace().to_owned())
                .filter(|dataset| !dataset.trim().is_empty())
                .unwrap_or_else(|| "spiral_robotics".to_owned()),
            spiral_query_available: settings
                .as_ref()
                .is_some_and(|settings| !settings.spiral().project_id().trim().is_empty()),
        }
    }));
    use_context_provider(|| configuration);
    let configuration_value = configuration();

    let selected_dataset = studio.selected_dataset().read().to_owned();
    let data = use_data_controller(&configuration_value.dataset, &selected_dataset);
    let workflow = use_workflow_controller();
    studio.use_event_stream(workflow);
    use_training_controller(&configuration_value.dataset, &selected_dataset);
    use_rollout_controller();
    let timeline = use_timeline_controller();
    let settings_open = shell.settings_open();
    let mut settings_was_open = use_signal(|| false);
    let mut project_setup_prompted = use_signal(|| false);
    use_effect(use_reactive((&loaded_settings,), move |(settings,)| {
        if settings
            .as_ref()
            .is_some_and(|settings| settings.spiral().project_id().trim().is_empty())
            && !project_setup_prompted()
        {
            project_setup_prompted.set(true);
            shell.dispatch(ShellAction::OpenSettings);
        }
    }));
    use_effect(use_reactive((&settings_open,), move |(settings_open,)| {
        let was_open = settings_was_open();
        if !settings_open && was_open {
            BrowserFocus::after_render("settings-rail-trigger".to_owned());
        }
        if was_open != settings_open {
            settings_was_open.set(settings_open);
        }
    }));

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Script { src: ECHARTS_RUNTIME }
        document::Title { if settings_open { "Spiral Robotics Studio · Settings" } else { "Spiral Robotics Studio · {activity.label()}" } }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light" }

        div {
            class: "studio-shell studio-shell-v2",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape && shell.settings_open() && !settings_busy() {
                    shell.dispatch(ShellAction::CloseSettings);
                }
            },
            div {
                class: "studio-shell-background",
                main { class: "studio-v2-body",
                    ActivityRail {
                        state: crate::shell::frame::ActivityRailState::new(
                            activity,
                            settings_open,
                        ),
                        on_activity: move |next| {
                            timeline.stop_preview();
                            studio.notifications().write().close();
                            shell.dispatch(ShellAction::CloseSettings);
                            navigator.push(StudioRoute::for_activity(next));
                        },
                        on_settings: move |_| shell.dispatch(ShellAction::OpenSettings),
                    }
                    if settings_open {
                        SettingsPanel {
                            state: crate::settings::panel::SettingsLoadState::new(
                                loaded_settings.clone(),
                                settings_error.clone(),
                            ),
                            handlers: crate::settings::panel::SettingsPanelHandlers::new(
                                move |_| {
                                    if !settings_busy() {
                                        shell.dispatch(ShellAction::CloseSettings);
                                    }
                                },
                                move |_| {
                                    settings.restart();
                                    data.invalidate_episode_catalog();
                                },
                                move |busy| settings_busy.set(busy),
                            ),
                        }
                    } else {
                        Outlet::<StudioRoute> {}
                    }
                }
                StatusBar {
                    mode: activity,
                    on_open_episode: move |episode| {
                        data.set_selected_episode(Some(episode));
                        navigator.push(StudioRoute::Data {});
                    },
                }
            }
        }
    }
}

#[component]
fn Live() -> Element {
    rsx! { EpisodeWorkspace { activity: Activity::Live } }
}

#[component]
fn Data() -> Element {
    rsx! { EpisodeWorkspace { activity: Activity::Data } }
}

#[component]
fn Workflow() -> Element {
    rsx! { WorkflowWorkspace {} }
}

#[component]
fn Training() -> Element {
    rsx! { TrainingWorkspace {} }
}

#[component]
fn Rollout() -> Element {
    rsx! { EpisodeWorkspace { activity: Activity::Rollout } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_primary_screen_has_a_stable_route() {
        assert_eq!(StudioRoute::Live {}.to_string(), "/live");
        assert_eq!(StudioRoute::Data {}.to_string(), "/data");
        assert_eq!(StudioRoute::Workflow {}.to_string(), "/workflow");
        assert_eq!(StudioRoute::Training {}.to_string(), "/training");
        assert_eq!(StudioRoute::Rollout {}.to_string(), "/rollout");
    }
}
