//! The workspace this browser works in, and the others it can switch to. A
//! workspace is one Spiral project under a shared name, with the robot
//! connector and training runtime everyone in it uses.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::account::SpiralProject;
use spiral_robotics_common::settings::{DataSettings, SpiralSettings};
use spiral_robotics_common::workspace::{NewWorkspace, Workspace, WorkspaceChoice};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, CheckIcon, ChevronIcon, Menu};

use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::transport::{CommandError, HttpRequest};
use crate::shell::project::ProjectSelection;
use crate::shell::routes::StudioRoute;
use crate::shell::state::StudioProjectState;

/// Workspace requests. Workspaces belong to the signed-in user, not to the
/// mounted one; these name it only because every request from a mount does.
pub(crate) struct WorkspaceClient {
    project: ProjectClient,
}

impl WorkspaceClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }

    fn workspaces(&self) -> impl Future<Output = Result<Vec<Workspace>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/workspaces"))
    }

    /// The Spiral projects the user can open, which a new workspace may use.
    pub(crate) fn spiral_projects(
        &self,
    ) -> impl Future<Output = Result<Vec<SpiralProject>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/account/projects"))
    }

    pub(crate) fn create(
        &self,
        request: &NewWorkspace,
    ) -> impl Future<Output = Result<Workspace, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Settings],
            HttpRequest::new("POST", "/api/workspaces").json(request),
        )
    }

    fn choose(
        &self,
        project_id: &str,
    ) -> impl Future<Output = Result<SpiralSettings, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project],
            HttpRequest::new("PUT", "/api/settings/connection")
                .json(&WorkspaceChoice::new(project_id)),
        )
    }

    pub(crate) fn delete(
        &self,
        project_id: &str,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        let path = format!("/api/workspaces/{project_id}");
        self.project
            .mutate(&[QueryDomain::Project], HttpRequest::new("DELETE", &path))
    }
}

/// Moving this browser between workspaces. Studio remounts on the one it
/// moves to, dropping every draft and request of the one it leaves.
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceSwitch {
    selection: ProjectSelection,
}

impl WorkspaceSwitch {
    pub(crate) fn current() -> Self {
        Self {
            selection: ProjectSelection::current(),
        }
    }

    /// Work in the workspace on `project_id` from now on, in this browser.
    pub(crate) async fn to(self, project_id: &str) -> Result<(), CommandError> {
        let saved = WorkspaceClient::current().choose(project_id).await?;
        self.selection.select(saved.project());
        Ok(())
    }

    /// Leave the mounted workspace for wherever this browser's session
    /// works now.
    pub(crate) fn leave(self) {
        self.selection.forget();
    }
}

/// What the mounted workspace keeps: its name, robot connector, and training
/// runtime. Read once a mount, again after a change here, and every few
/// seconds for changes made in other browsers.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WorkspaceSettings {
    settings: Resource<Result<DataSettings, String>>,
    loaded: Signal<Option<DataSettings>>,
}

impl WorkspaceSettings {
    pub(crate) fn use_provider() -> Self {
        let settings = QueryPolicy::new(QueryDomain::Settings, 10_000).use_resource(|| async {
            ProjectClient::current()
                .request(HttpRequest::new("GET", "/api/settings"))
                .await
                .map_err(|error: CommandError| error.to_string())
        });
        let mut loaded = use_signal(|| None::<DataSettings>);
        use_effect(move || {
            let result = settings.read();
            if let Some(Ok(value)) = result.as_ref()
                && loaded.peek().as_ref() != Some(value)
            {
                loaded.set(Some(value.clone()));
            }
        });
        use_context_provider(|| Self { settings, loaded })
    }

    pub(crate) fn current() -> Self {
        consume_context()
    }

    /// The settings last read; `None` until the first read lands.
    pub(crate) fn loaded(self) -> Option<DataSettings> {
        self.loaded.read().clone()
    }

    pub(crate) fn error(self) -> Option<String> {
        self.settings
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .cloned()
    }

    pub(crate) fn refresh(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Settings]);
    }

    /// The workspace's name once read; empty before a workspace is chosen.
    pub(crate) fn name(self) -> Option<String> {
        self.loaded
            .read()
            .as_ref()
            .map(|settings| settings.name().to_owned())
    }
}

/// The top bar's workspace: pressing it lists the workspaces to switch to.
#[component]
pub(crate) fn WorkspaceSwitcher(tone: &'static str, title: String, children: Element) -> Element {
    const OPENER: &str = "workspace-switcher";
    let mut anchor = use_signal(|| None::<MenuAnchor>);
    let opener = use_signal(|| OPENER.to_owned());
    rsx! {
        button {
            id: OPENER,
            r#type: "button",
            class: "studio-status-connection",
            "data-tone": tone,
            title,
            "aria-haspopup": "menu",
            "aria-expanded": anchor.read().is_some(),
            onclick: move |_| {
                let next = if anchor.peek().is_some() { None } else { MenuAnchor::below(OPENER) };
                anchor.set(next);
            },
            {children}
            ChevronIcon { open: anchor.read().is_some() }
        }
        if let Some(at) = anchor() {
            div { class: "studio-workspace-menu", style: "{at.style()}",
                WorkspaceMenu { opener, on_close: move |()| anchor.set(None) }
            }
        }
    }
}

/// Where an open menu hangs: right-aligned under the control that opened it,
/// placed on the page so no clipping parent cuts it off.
#[derive(Clone, Copy, PartialEq)]
struct MenuAnchor {
    right: f64,
    top: f64,
}

impl MenuAnchor {
    const GAP_PX: f64 = 4.0;

    fn below(opener: &str) -> Option<Self> {
        let window = web_sys::window()?;
        let rect = window
            .document()?
            .get_element_by_id(opener)?
            .get_bounding_client_rect();
        let width = window.inner_width().ok()?.as_f64()?;
        Some(Self {
            right: (width - rect.right()).max(0.0),
            top: rect.bottom() + Self::GAP_PX,
        })
    }

    fn style(self) -> String {
        format!("right:{}px;top:{}px", self.right, self.top)
    }
}

#[component]
fn WorkspaceMenu(opener: ReadSignal<String>, on_close: EventHandler<()>) -> Element {
    let workspaces = QueryPolicy::on_demand(QueryDomain::Settings)
        .use_resource(|| WorkspaceClient::current().workspaces());
    let current = ProjectClient::current().project().project_id().to_owned();
    let navigator = use_navigator();
    let switch = WorkspaceSwitch::current();
    let studio = StudioProjectState::current();
    let choose = move |project_id: String| {
        on_close.call(());
        ProjectClient::current().spawn(async move {
            if let Err(error) = switch.to(&project_id).await {
                studio.report(format!("{error} {}", error.recovery_guidance()));
            }
        });
    };
    let listed = match &*workspaces.read() {
        None => {
            rsx! { p { class: "studio-workspace-menu-note", role: "status", "Loading workspaces…" } }
        }
        Some(Err(error)) => {
            rsx! { p { class: "studio-workspace-menu-note", role: "alert", "{error}" } }
        }
        Some(Ok(workspaces)) => rsx! {
            for workspace in workspaces.iter().cloned() {
                Button {
                    key: "{workspace.project_id()}",
                    purpose: ActionPurpose::Select,
                    variant: ButtonVariant::Row,
                    role: "menuitemradio",
                    aria_checked: workspace.project_id() == current,
                    title: "Spiral project {workspace.project_id()}",
                    onclick: {
                        let project_id = workspace.project_id().to_owned();
                        move |_| choose(project_id.clone())
                    },
                    span { class: "studio-workspace-menu-name", "{workspace.name()}" }
                    if workspace.project_id() == current {
                        CheckIcon {}
                    }
                }
            }
        },
    };
    rsx! {
        Menu { label: "Workspaces", opener, on_close,
            {listed}
            div { class: "studio-workspace-menu-separator", role: "separator" }
            Button {
                purpose: ActionPurpose::Create,
                variant: ButtonVariant::Row,
                role: "menuitem",
                onclick: move |_| {
                    on_close.call(());
                    navigator.push(StudioRoute::NewWorkspace {});
                },
                "New workspace…"
            }
            if !current.is_empty() {
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Row,
                    role: "menuitem",
                    onclick: move |_| {
                        on_close.call(());
                        navigator.push(StudioRoute::Settings {});
                    },
                    "Workspace settings"
                }
            }
        }
    }
}
