//! URL routes and the low-frequency Studio shell shared by every screen.

use crate::components::interaction::BrowserFocus;
use dioxus::prelude::*;

use crate::activities::training::workspace::TrainingWorkspace;
use crate::activities::workflow::workspace::WorkflowWorkspace;
use crate::settings::panel::SettingsPanel;
use crate::shell::activity::Activity;
use crate::shell::frame::{ActivityRail, StatusBar};
use crate::shell::project::ProjectSessionSignals;
use crate::shell::state::Studio;
use crate::shell::workspace::EpisodeWorkspace;
use spiral_robotics_common::project::ProjectSessionInfo;

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

#[derive(Clone, Copy)]
struct StudioShellSignals {
    settings_open: Signal<bool>,
    settings_busy: Signal<bool>,
    settings_was_open: Signal<bool>,
    project_setup_prompted: Signal<bool>,
}

impl StudioShellSignals {
    fn use_signals() -> Self {
        Self {
            settings_open: use_signal(|| false),
            settings_busy: use_signal(|| false),
            settings_was_open: use_signal(|| false),
            project_setup_prompted: use_signal(|| false),
        }
    }
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

#[component]
pub(crate) fn StudioRouter() -> Element {
    rsx! { Router::<StudioRoute> {} }
}

/// Controllers and settings live above the route outlet so drafts, selected
/// runs, messages, and long-running backend commands survive navigation.
#[component]
fn StudioShell() -> Element {
    let sessions = ProjectSessionSignals::use_provider();
    sessions.use_event_stream();
    let session = sessions.mounted_session();
    rsx! {
        document::Stylesheet { href: TAILWIND }
        if let Some(session) = session {
            ProjectStudioShell { key: "{session.id()}", session }
        } else {
            div { class: "studio-shell studio-shell-v2",
                div { class: "editor-empty", role: "status", "Connecting to Studio…" }
            }
        }
    }
}

/// Every project resource, controller, selection, and draft is owned below this
/// key. Dioxus drops this scope and its tasks before mounting another project.
#[component]
fn ProjectStudioShell(session: ProjectSessionInfo) -> Element {
    let sessions = ProjectSessionSignals::current();
    use_context_provider(|| sessions.client(session));
    let studio = Studio::use_studio();
    let route = use_route::<StudioRoute>();
    let activity = route.activity();
    let navigator = use_navigator();
    let StudioShellSignals {
        mut settings_open,
        mut settings_busy,
        mut settings_was_open,
        mut project_setup_prompted,
    } = StudioShellSignals::use_signals();
    let settings_result = studio.settings_result();
    let loaded_settings = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    let settings_error = settings_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let data = studio.data_controller();
    studio.use_event_stream();
    let timeline = studio.timeline_controller();
    let settings_are_open = settings_open();
    use_effect(use_reactive((&loaded_settings,), move |(settings,)| {
        if settings
            .as_ref()
            .is_some_and(|settings| settings.spiral().project_id().trim().is_empty())
            && !project_setup_prompted()
        {
            project_setup_prompted.set(true);
            settings_open.set(true);
        }
    }));
    use_effect(use_reactive(
        (&settings_are_open,),
        move |(settings_are_open,)| {
            let was_open = settings_was_open();
            if !settings_are_open && was_open {
                BrowserFocus::after_render("settings-rail-trigger".to_owned());
            }
            if was_open != settings_are_open {
                settings_was_open.set(settings_are_open);
            }
        },
    ));

    rsx! {
        document::Script { src: ECHARTS_RUNTIME }
        document::Title { if settings_are_open { "Spiral Robotics Studio · Settings" } else { "Spiral Robotics Studio · {activity.label()}" } }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light" }

        div {
            class: "studio-shell studio-shell-v2",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape && settings_open() && !settings_busy() {
                    settings_open.set(false);
                }
            },
            div {
                class: "studio-shell-background",
                main { class: "studio-v2-body",
                    ActivityRail {
                        state: crate::shell::frame::ActivityRailState::new(
                            activity,
                            settings_are_open,
                        ),
                        on_activity: move |next| {
                            timeline.stop_preview();
                            studio.notifications().write().close();
                            settings_open.set(false);
                            navigator.push(StudioRoute::for_activity(next));
                        },
                        on_settings: move |_| settings_open.set(true),
                    }
                    if settings_are_open {
                        SettingsPanel {
                            state: crate::settings::panel::SettingsLoadState::new(
                                loaded_settings.clone(),
                                settings_error.clone(),
                            ),
                            handlers: crate::settings::panel::SettingsPanelHandlers::new(
                                move |_| {
                                    if !settings_busy() {
                                        settings_open.set(false);
                                    }
                                },
                                move |_| {
                                    studio.refresh_settings();
                                    studio.refresh_relation_catalog();
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
