//! URL routes and the low-frequency Studio shell shared by every screen.

use dioxus::prelude::*;

use crate::activities::curation::workspace::CurationWorkspace;
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
use spiral_robotics_common::project::ProjectSessionInfo;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const ECHARTS_RUNTIME: &str = "https://cdn.jsdelivr.net/npm/echarts@6.1.0/dist/echarts.min.js";
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

#[derive(Clone, Routable, Debug, PartialEq)]
pub(crate) enum StudioRoute {
    #[layout(StudioShell)]
    #[redirect("/", || StudioRoute::Live {})]
    #[route("/live", SelectedActivityRoute)]
    Live {},
    #[route("/data", SelectedActivityRoute)]
    Data {},
    #[route("/workflow", SelectedActivityRoute)]
    Workflow {},
    #[route("/curation", SelectedActivityRoute)]
    Curation {},
    #[route("/training", SelectedActivityRoute)]
    Training {},
    #[route("/rollout", SelectedActivityRoute)]
    Rollout {},
    #[route("/settings", SelectedActivityRoute)]
    Settings {},
}

impl StudioRoute {
    pub(crate) const fn activity(&self) -> Activity {
        match self {
            Self::Live {} => Activity::Live,
            Self::Data {} => Activity::Data,
            Self::Workflow {} => Activity::Workflow,
            Self::Curation {} => Activity::Curation,
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
            Activity::Curation => Self::Curation {},
            Activity::Training => Self::Training {},
            Activity::Rollout => Self::Rollout {},
            Activity::Settings => Self::Settings {},
        }
    }

    #[cfg(test)]
    fn all() -> [Self; 7] {
        [
            Self::Live {},
            Self::Data {},
            Self::Workflow {},
            Self::Curation {},
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
        if session.is_none() {
            div { class: "studio-shell studio-shell-v2",
                div { class: "editor-empty", role: "status", "Connecting to Studio…" }
            }
        }
        for session in session {
            ProjectStudioShell { key: "{session.id()}", session }
        }
    }
}

/// Every project resource, controller, selection, and draft is owned below this
/// key. Dioxus drops this scope and its tasks before mounting another project.
#[component]
fn ProjectStudioShell(session: ProjectSessionInfo) -> Element {
    let sessions = ProjectSessionSignals::current();
    use_context_provider(|| sessions.client(session));
    let project = StudioProjectState::use_provider();
    project.use_polling();
    let route = use_route::<StudioRoute>();
    let selected_view = use_memo(use_reactive((&route,), |(route,)| route.activity()));
    let navigator = use_navigator();
    let mut project_setup_prompted = use_signal(|| false);
    use_effect(move || {
        if sessions
            .mounted_session()
            .is_some_and(|session| session.project_id().trim().is_empty())
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
                    for activity in Activity::all() {
                        ActivityWorkspace { key: "{activity.label()}", activity, selected_view }
                    }
                    Outlet::<StudioRoute> {}
                }
            }
        }
    }
}

/// URL leaves select an activity. The shared layout owns its persistent instance.
#[component]
fn SelectedActivityRoute() -> Element {
    rsx! {}
}

#[component]
fn ActivityWorkspace(activity: Activity, selected_view: ReadSignal<Activity>) -> Element {
    let is_active = use_memo(move || selected_view() == activity);
    rsx! {
        PersistentWorkspace { is_active,
            match activity {
                Activity::Live => rsx! { LiveWorkspace { is_active } },
                Activity::Data => rsx! { DataWorkspace { is_active } },
                Activity::Workflow => rsx! { WorkflowWorkspace { is_active } },
                Activity::Curation => rsx! { CurationWorkspace { is_active } },
                Activity::Training => rsx! { TrainingWorkspace { is_active } },
                Activity::Rollout => rsx! { RolloutWorkspace { is_active } },
                Activity::Settings => rsx! { SettingsWorkspace { is_active } },
            }
        }
    }
}

/// Visit once, then preserve the component tree until its project is disposed.
#[component]
fn PersistentWorkspace(is_active: ReadSignal<bool>, children: Element) -> Element {
    let active = is_active();
    let mut visited = use_signal(move || active);
    use_effect(move || {
        if is_active() && !*visited.peek() {
            visited.set(true);
        }
    });
    rsx! {
        div { class: "studio-workspace", hidden: !is_active(),
            if visited() || is_active() {
                {children}
            }
        }
    }
}

#[cfg(test)]
mod tests;
