//! URL routes and the low-frequency Studio shell shared by every screen.

use dioxus::prelude::*;

use crate::activities::curation::workspace::CurationWorkspace;
use crate::activities::data::screen::DataWorkspace;
use crate::activities::live::screen::LiveWorkspace;
use crate::activities::rollout::screen::RolloutWorkspace;
use crate::activities::settings::panel::SettingsWorkspace;
use crate::activities::training::workspace::TrainingWorkspace;
use crate::shell::activity::Activity;
use crate::shell::frame::StudioTopBar;
use crate::shell::project::ProjectSelection;
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::project::ProjectKey;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const ECHARTS_RUNTIME: &str = "https://cdn.jsdelivr.net/npm/echarts@6.1.0/dist/echarts.min.js";
const FAVICON: Asset = asset!("/assets/favicon.svg");

#[derive(Clone, Routable, Debug, PartialEq)]
pub(crate) enum StudioRoute {
    #[layout(StudioShell)]
    #[redirect("/", || StudioRoute::Data {})]
    #[route("/live", SelectedActivityRoute)]
    Live {},
    #[route("/data", SelectedActivityRoute)]
    Data {},
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
    pub(super) const fn activity(&self) -> Activity {
        match self {
            Self::Live {} => Activity::Live,
            Self::Data {} => Activity::Data,
            Self::Curation {} => Activity::Curation,
            Self::Training {} => Activity::Training,
            Self::Rollout {} => Activity::Rollout,
            Self::Settings {} => Activity::Settings,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        self.activity().label()
    }

    pub(super) const fn for_activity(activity: Activity) -> Self {
        match activity {
            Activity::Live => Self::Live {},
            Activity::Data => Self::Data {},
            Activity::Curation => Self::Curation {},
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
            Self::Curation {},
            Self::Training {},
            Self::Rollout {},
            Self::Settings {},
        ]
    }
}

#[component]
pub(super) fn StudioRouter() -> Element {
    rsx! { Router::<StudioRoute> {} }
}

/// Only the saved connection is read above the project mount. A browser
/// without a Spiral session mounts no project.
#[component]
fn StudioShell() -> Element {
    let selection = ProjectSelection::use_provider();
    selection.use_polling();
    let project = selection.selected().filter(|_| !selection.signed_out());
    rsx! {
        document::Stylesheet { href: TAILWIND }
        if selection.signed_out() {
            super::sign_in::SignInGate {}
        } else if project.is_none() {
            div { class: "studio-shell studio-shell-v2",
                div { class: "editor-empty", role: "status", "Connecting to Studio…" }
            }
        }
        for project in project {
            ProjectStudioShell { key: "{project.endpoint()} {project.project_id()}", project }
        }
    }
}

/// Every project resource, controller, selection, and draft is owned below this
/// key. Dioxus drops this scope and its tasks before mounting another project.
#[component]
fn ProjectStudioShell(project: ProjectKey) -> Element {
    let chosen = project.is_chosen();
    use_context_provider(|| ProjectSelection::current().client(project));
    crate::activities::episode::EpisodeStills::use_provider();
    crate::activities::data::DatasetOverviewMemory::use_provider();
    crate::activities::data::DatasetChecks::use_provider();
    let project = StudioProjectState::use_provider();
    project.use_polling();
    let route = use_route::<StudioRoute>();
    let selected_view = use_memo(use_reactive((&route,), |(route,)| route.activity()));
    let navigator = use_navigator();
    // The mount is per project, so this asks once for a project to be chosen.
    use_effect(move || {
        if !chosen {
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
                    StudioTopBar {
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
