//! Who this browser is signed in to Spiral as, and the project it works on.
//! The backend holds the session; this page only learns who is signed in.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::account::{
    OrganizationSwitch, SpiralIdentity, SpiralOrganization, SpiralProject,
};
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, Field, Notice, NoticeTone, Select};

use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::client::transport::{CommandError, HttpRequest};
use crate::shell::project::ProjectSelection;

/// Account requests. Leaving the account closes this browser's open projects,
/// so those commands name the mounted project like every project command.
struct AccountClient {
    project: ProjectClient,
}

impl AccountClient {
    fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }

    fn identity(&self) -> impl Future<Output = Result<SpiralIdentity, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/account"))
    }

    fn organizations(
        &self,
    ) -> impl Future<Output = Result<Vec<SpiralOrganization>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/account/organizations"))
    }

    fn projects(&self) -> impl Future<Output = Result<Vec<SpiralProject>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/account/projects"))
    }

    fn sign_out(&self) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project, QueryDomain::Settings],
            HttpRequest::new("POST", "/api/account/sign-out"),
        )
    }

    fn switch_organization(
        &self,
        organization: String,
    ) -> impl Future<Output = Result<SpiralIdentity, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project, QueryDomain::Settings],
            HttpRequest::new("PUT", "/api/account/organization")
                .json(&OrganizationSwitch::new(organization)),
        )
    }
}

/// Where this browser goes once an account command succeeds.
#[derive(Clone, Copy)]
enum AccountChange {
    /// Another organization's projects are others: start over without one.
    LeavesProject,
    /// Only signing in again continues.
    SignsOut,
}

/// One account command at a time, and what the last one reported.
#[derive(Clone, Copy)]
struct AccountActions {
    pending: Signal<bool>,
    failure: Signal<Option<CommandError>>,
    selection: ProjectSelection,
}

impl AccountActions {
    fn use_actions() -> Self {
        Self {
            pending: use_signal(|| false),
            failure: use_signal(|| None),
            selection: ProjectSelection::current(),
        }
    }

    /// Run `command`, which closes this browser's projects.
    fn run<T: 'static>(
        mut self,
        command: impl Future<Output = Result<T, CommandError>> + 'static,
        change: AccountChange,
    ) {
        if *self.pending.peek() {
            return;
        }
        self.pending.set(true);
        self.failure.set(None);
        let client = ProjectClient::current();
        let endpoint = client.project().endpoint().to_owned();
        client.spawn(async move {
            let result = command.await;
            self.pending.set(false);
            match (result, change) {
                (Ok(_), AccountChange::LeavesProject) => {
                    self.selection.select(ProjectKey::new(endpoint, ""));
                }
                (Ok(_), AccountChange::SignsOut) => self.selection.sign_out(),
                (Err(error), _) => self.failure.set(Some(error)),
            }
        });
    }
}

/// Who this browser reaches Spiral as, and the project it works on. Choosing a
/// project edits the settings draft; saving applies it.
#[component]
pub(super) fn SpiralAccountSection(
    project_id: String,
    on_project: EventHandler<String>,
    disabled: bool,
) -> Element {
    let identity = QueryPolicy::new(QueryDomain::Settings, 1_000)
        .use_resource(|| AccountClient::current().identity());
    let signed_in = use_memo(move || matches!(&*identity.read(), Some(Ok(_))));
    let organizations = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
        move || *signed_in.read(),
        || AccountClient::current().organizations(),
    );
    let projects = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
        move || *signed_in.read(),
        || AccountClient::current().projects(),
    );
    let actions = AccountActions::use_actions();
    let disabled = disabled || (actions.pending)();
    let current = identity.read().clone();
    rsx! {
        div { class: "settings-account",
        match current {
            None => rsx! { p { class: "settings-help", role: "status", "Checking your Spiral sign-in…" } },
            Some(Err(error)) => rsx! { Notice { tone: NoticeTone::Caution, "{error}" } },
            Some(Ok(identity)) => rsx! {
                div { class: "settings-account-identity",
                    p { class: "settings-help",
                        "Signed in as "
                        strong { "{identity.email()}" }
                        " in "
                        strong { "{identity.organization().label()}" }
                        "."
                    }
                    Button {
                        purpose: ActionPurpose::Disconnect,
                        variant: ButtonVariant::Link,
                        disabled,
                        title: "Signs this browser out of Studio",
                        onclick: move |_| actions.run(AccountClient::current().sign_out(), AccountChange::SignsOut),
                        "Sign out"
                    }
                }
                OrganizationChoice {
                    current: identity.organization().clone(),
                    organizations: organizations.read().as_ref().and_then(|result| result.as_ref().ok()).cloned().unwrap_or_default(),
                    choice: AccountChoice::new(
                        EventHandler::new(move |organization| actions.run(AccountClient::current().switch_organization(organization), AccountChange::LeavesProject)),
                        disabled,
                    ),
                }
                ProjectChoice {
                    project_id,
                    projects: projects.read().clone(),
                    choice: AccountChoice::new(on_project, disabled),
                }
            },
        }
        if let Some(error) = (actions.failure)() {
            Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
        }
        }
    }
}

/// Where a choice goes, and whether one can be made now.
#[derive(Clone, PartialEq)]
struct AccountChoice {
    on_choose: EventHandler<String>,
    disabled: bool,
}

impl AccountChoice {
    fn new(on_choose: EventHandler<String>, disabled: bool) -> Self {
        Self {
            on_choose,
            disabled,
        }
    }
}

/// Switching organization only appears for a user in more than one.
#[component]
fn OrganizationChoice(
    current: SpiralOrganization,
    organizations: Vec<SpiralOrganization>,
    choice: AccountChoice,
) -> Element {
    if organizations.len() < 2 {
        return rsx! {};
    }
    let on_organization = choice.on_choose;
    rsx! {
        Field { label: "Organization", class: "settings-field",
            Select {
                value: current.id().to_owned(),
                disabled: choice.disabled,
                onchange: move |chosen| on_organization.call(chosen),
                for organization in organizations.iter() {
                    option {
                        key: "{organization.id()}",
                        value: "{organization.id()}",
                        selected: organization.id() == current.id(),
                        "{organization.label()}"
                    }
                }
            }
        }
    }
}

#[component]
fn ProjectChoice(
    project_id: String,
    projects: Option<Result<Vec<SpiralProject>, CommandError>>,
    choice: AccountChoice,
) -> Element {
    let projects = match projects {
        None => {
            return rsx! { p { class: "settings-help", role: "status", "Loading your projects…" } };
        }
        Some(Err(error)) => return rsx! { Notice { tone: NoticeTone::Caution, "{error}" } },
        Some(Ok(projects)) => projects,
    };
    if projects.is_empty() {
        return rsx! {
            p { class: "settings-help", "This organization has no projects you can open." }
        };
    }
    // A saved project outside this organization stays visible rather than
    // silently becoming another one.
    let unlisted = (!project_id.is_empty()
        && projects.iter().all(|project| project.id() != project_id))
    .then(|| project_id.clone());
    let on_project = choice.on_choose;
    rsx! {
        Field { label: "Project", class: "settings-field",
            Select {
                value: project_id.clone(),
                disabled: choice.disabled,
                onchange: move |chosen| on_project.call(chosen),
                option { value: "", disabled: true, selected: project_id.is_empty(), "Choose a project" }
                for project in projects.iter() {
                    option {
                        key: "{project.id()}",
                        value: "{project.id()}",
                        selected: project.id() == project_id,
                        {project.description().map_or_else(|| project.id().to_owned(), |description| format!("{} — {description}", project.id()))}
                    }
                }
                if let Some(unlisted) = unlisted {
                    option { value: "{unlisted}", selected: true, "{unlisted} (not in this organization)" }
                }
            }
        }
    }
}
