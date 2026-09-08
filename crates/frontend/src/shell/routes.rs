//! URL routes and the low-frequency Studio shell shared by every screen.

use dioxus::prelude::*;

use crate::activities::data::screen::DataWorkspace;
use crate::activities::live::screen::LiveWorkspace;
use crate::activities::rollout::screen::RolloutWorkspace;
use crate::activities::settings::panel::SettingsWorkspace;
use crate::activities::training::workspace::TrainingWorkspace;
use crate::activities::workflow::workspace::WorkflowWorkspace;
use crate::shell::activity::Activity;
use crate::shell::frame::ActivityRail;
use crate::shell::project::ProjectSessionSignals;
use crate::shell::state::StudioProjectState;
use crate::shell::view_state::ProjectViewStates;
use spiral_robotics_common::project::ProjectSessionInfo;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const ECHARTS_RUNTIME: &str = "https://cdn.jsdelivr.net/npm/echarts@6.1.0/dist/echarts.min.js";
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

#[derive(Clone, Routable, Debug, PartialEq)]
pub(crate) enum StudioRoute {
    #[layout(StudioShell)]
    #[redirect("/", || StudioRoute::Live {})]
    #[route("/live", LiveWorkspace)]
    Live {},
    #[route("/data", DataWorkspace)]
    Data {},
    #[route("/workflow", WorkflowWorkspace)]
    Workflow {},
    #[route("/training", TrainingWorkspace)]
    Training {},
    #[route("/rollout", RolloutWorkspace)]
    Rollout {},
    #[route("/settings", SettingsWorkspace)]
    Settings {},
}

impl StudioRoute {
    pub(crate) const fn activity(&self) -> Activity {
        match self {
            Self::Live {} => Activity::Live,
            Self::Data {} => Activity::Data,
            Self::Workflow {} => Activity::Workflow,
            Self::Training {} => Activity::Training,
            Self::Rollout {} => Activity::Rollout,
            Self::Settings {} => Activity::Settings,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.activity().label()
    }

    pub(crate) const fn for_activity(activity: Activity) -> Self {
        match activity {
            Activity::Live => Self::Live {},
            Activity::Data => Self::Data {},
            Activity::Workflow => Self::Workflow {},
            Activity::Training => Self::Training {},
            Activity::Rollout => Self::Rollout {},
            Activity::Settings => Self::Settings {},
        }
    }

    #[cfg(test)]
    fn all() -> [Self; 6] {
        [
            Self::Live {},
            Self::Data {},
            Self::Workflow {},
            Self::Training {},
            Self::Rollout {},
            Self::Settings {},
        ]
    }
}

#[component]
pub(crate) fn StudioRouter() -> Element {
    rsx! { Router::<StudioRoute> {} }
}

/// Only project connection and configuration reads run above the view sessions.
#[component]
fn StudioShell() -> Element {
    let sessions = ProjectSessionSignals::use_provider();
    sessions.use_polling();
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
    ProjectViewStates::use_provider();
    let project = StudioProjectState::use_provider();
    project.use_polling();
    let route = use_route::<StudioRoute>();
    let navigator = use_navigator();
    let mut project_setup_prompted = use_signal(|| false);
    use_effect(move || {
        if project
            .loaded_settings()
            .as_ref()
            .is_some_and(|settings| settings.spiral().project_id().trim().is_empty())
            && !project_setup_prompted()
        {
            project_setup_prompted.set(true);
            navigator.push(StudioRoute::Settings {});
        }
    });
    let title = format!("Spiral Robotics Studio · {}", route.label());
    rsx! {
        document::Script { src: ECHARTS_RUNTIME }
        document::Title { "{title}" }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light" }
        div { class: "studio-shell studio-shell-v2",
            div { class: "studio-shell-background",
                main { class: "studio-v2-body studio-route-layout",
                    ActivityRail {
                        activity: route.activity(),
                        on_activity: move |next| {
                            project.close_notifications();
                            navigator.push(StudioRoute::for_activity(next));
                        },
                    }
                    Outlet::<StudioRoute> {}
                }
            }
        }
    }
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
        assert_eq!(StudioRoute::Settings {}.to_string(), "/settings");
        for route in StudioRoute::all() {
            assert_eq!(StudioRoute::for_activity(route.activity()), route);
        }
        assert_eq!(StudioRoute::Settings {}.activity(), Activity::Settings);
    }
}
