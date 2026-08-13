//! URL routes and the low-frequency Studio shell shared by every screen.

use dioxus::prelude::*;
use spiral_robotics_contracts::api::DataSettings;

use crate::frontend::api::http as api;
use crate::frontend::app::activity::Activity;
use crate::frontend::app::navigation::{ShellAction, use_shell_controller};
use crate::frontend::app::shell::{ActivityRail, StatusBar};
use crate::frontend::panels::timeline::state::use_timeline_controller;
use crate::frontend::screens::data::state::use_data_controller;
use crate::frontend::screens::rollout::state::use_rollout_controller;
use crate::frontend::screens::settings::panel::SettingsPanel;
use crate::frontend::screens::training::state::use_training_controller;

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
            Self::Training {} => Activity::Training,
            Self::Rollout {} => Activity::Rollout,
        }
    }

    pub(crate) const fn for_activity(activity: Activity) -> Self {
        match activity {
            Activity::Live => Self::Live {},
            Activity::Data => Self::Data {},
            Activity::Training => Self::Training {},
            Activity::Rollout => Self::Rollout {},
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudioConfiguration {
    pub dataset: String,
    pub spiral_query_available: bool,
}

impl Default for StudioConfiguration {
    fn default() -> Self {
        Self {
            dataset: "spiral_robotics".to_owned(),
            spiral_query_available: false,
        }
    }
}

pub(crate) fn studio_configuration() -> StudioConfiguration {
    use_context::<Signal<StudioConfiguration>>()()
}

#[component]
pub(crate) fn StudioRouter() -> Element {
    rsx! { Router::<StudioRoute> {} }
}

/// Controllers and settings live above the route outlet so drafts, selected
/// runs, messages, and long-running backend commands survive navigation.
#[component]
fn StudioShell() -> Element {
    let mut studio = crate::frontend::app::state::studio();
    let route = use_route::<StudioRoute>();
    let activity = route.activity();
    let navigator = use_navigator();
    let shell = use_shell_controller();
    let mut settings_busy = use_signal(|| false);
    let mut settings =
        use_resource(|| async { api::get::<DataSettings>("/api/settings/data").await });
    let settings_result = settings.read_unchecked().clone();
    let loaded_settings = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    let settings_error = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let configuration = StudioConfiguration {
        dataset: loaded_settings
            .as_ref()
            .map(|settings| settings.spiral.dataset.clone())
            .filter(|dataset| !dataset.trim().is_empty())
            .unwrap_or_else(|| "spiral_robotics".to_owned()),
        spiral_query_available: loaded_settings.as_ref().is_some_and(|settings| {
            settings.spiral.enabled
                && (settings.spiral.uri == "spiral:memory:"
                    || !settings.spiral.project_id.trim().is_empty())
        }),
    };
    let mut configuration_signal = use_signal(StudioConfiguration::default);
    use_context_provider(|| configuration_signal);
    use_effect(use_reactive((&configuration,), move |(next,)| {
        if *configuration_signal.peek() != next {
            configuration_signal.set(next);
        }
    }));

    let selected_dataset = studio.selected_dataset.read().clone();
    use_data_controller(&configuration.dataset, &selected_dataset);
    use_training_controller(
        &configuration.dataset,
        &selected_dataset,
        crate::frontend::screens::training::workspace::default_training_sql,
    );
    use_rollout_controller();
    let timeline = use_timeline_controller();
    let settings_open = shell.settings_open();
    let mut settings_was_open = use_signal(|| false);
    use_effect(use_reactive((&settings_open,), move |(settings_open,)| {
        let was_open = settings_was_open();
        if !settings_open && was_open {
            spawn(async {
                gloo_timers::future::TimeoutFuture::new(0).await;
                focus_settings_trigger();
            });
        }
        if was_open != settings_open {
            settings_was_open.set(settings_open);
        }
    }));

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Script { src: ECHARTS_RUNTIME }
        document::Title { "Spiral Robotics Studio · {activity.label()}" }
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
            if settings_open {
                SettingsPanel {
                    initial: loaded_settings.clone(),
                    load_error: settings_error.clone(),
                    on_close: move |_| {
                        if !settings_busy() {
                            shell.dispatch(ShellAction::CloseSettings);
                        }
                    },
                    on_saved: move |_| settings.restart(),
                    on_busy_change: move |busy| settings_busy.set(busy),
                }
            }
            div {
                class: "studio-shell-background",
                inert: settings_open.then_some("true"),
                "aria-hidden": settings_open,
                main { class: "studio-v2-body",
                    ActivityRail {
                        activity,
                        on_activity: move |next| {
                            timeline.stop_preview();
                            studio.notifications.write().close();
                            navigator.push(StudioRoute::for_activity(next));
                        },
                        on_settings: move |_| shell.dispatch(ShellAction::OpenSettings),
                    }
                    Outlet::<StudioRoute> {}
                }
                StatusBar {
                    mode: activity,
                    on_open_episode: move |episode| {
                        studio.selected_episode.set(Some(episode));
                        navigator.push(StudioRoute::Data {});
                    },
                }
            }
        }
    }
}

#[cfg(feature = "web")]
fn focus_settings_trigger() {
    use wasm_bindgen::JsCast as _;

    let Some(trigger) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("settings-rail-trigger"))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = trigger.focus();
}

#[cfg(not(feature = "web"))]
fn focus_settings_trigger() {}

#[component]
fn Live() -> Element {
    rsx! { crate::frontend::screens::live::screen::LiveScreen {} }
}

#[component]
fn Data() -> Element {
    rsx! { crate::frontend::screens::data::screen::DataScreen {} }
}

#[component]
fn Training() -> Element {
    rsx! { crate::frontend::screens::training::screen::TrainingScreen {} }
}

#[component]
fn Rollout() -> Element {
    rsx! { crate::frontend::screens::rollout::screen::RolloutScreen {} }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_primary_screen_has_a_stable_route() {
        assert_eq!(StudioRoute::Live {}.to_string(), "/live");
        assert_eq!(StudioRoute::Data {}.to_string(), "/data");
        assert_eq!(StudioRoute::Training {}.to_string(), "/training");
        assert_eq!(StudioRoute::Rollout {}.to_string(), "/rollout");
    }
}
