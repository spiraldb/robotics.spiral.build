//! Signing this Studio in to Spiral, and choosing the project it works on.
//! The backend holds the session; this page only learns who is signed in.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::account::{
    OrganizationSwitch, SpiralAccountStatus, SpiralOrganization, SpiralProject, SpiralSignIn,
};
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Field, LinkButton, Notice, NoticeTone, Select,
};

use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::client::transport::{CommandError, HttpRequest};
use crate::shell::project::ProjectSelection;

/// Account requests. Leaving the account closes the backend's open projects,
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

    fn status(&self) -> impl Future<Output = Result<SpiralAccountStatus, CommandError>> + use<> {
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

    fn sign_in(&self) -> impl Future<Output = Result<SpiralAccountStatus, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Settings],
            HttpRequest::new("POST", "/api/account/sign-in"),
        )
    }

    fn sign_out(&self) -> impl Future<Output = Result<SpiralAccountStatus, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project, QueryDomain::Settings],
            HttpRequest::new("POST", "/api/account/sign-out"),
        )
    }

    fn switch_organization(
        &self,
        organization: String,
    ) -> impl Future<Output = Result<SpiralAccountStatus, CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project, QueryDomain::Settings],
            HttpRequest::new("PUT", "/api/account/organization")
                .json(&OrganizationSwitch::new(organization)),
        )
    }
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

    /// Run `command`. Leaving the account closes its projects, so this browser
    /// then starts over without one.
    fn run(
        mut self,
        command: impl Future<Output = Result<SpiralAccountStatus, CommandError>> + 'static,
        leaves_project: bool,
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
            match result {
                Ok(_) if leaves_project => self.selection.select(ProjectKey::new(endpoint, "")),
                Ok(_) => {}
                Err(error) => self.failure.set(Some(error)),
            }
        });
    }
}

/// Who this Studio reaches Spiral as, and the project it works on. Choosing a
/// project edits the settings draft; saving applies it.
#[component]
pub(super) fn SpiralAccountSection(
    project_id: String,
    on_project: EventHandler<String>,
    disabled: bool,
) -> Element {
    let status = QueryPolicy::new(QueryDomain::Settings, 1_000)
        .use_resource(|| AccountClient::current().status());
    let identity = use_memo(move || match &*status.read() {
        Some(Ok(SpiralAccountStatus::SignedIn(identity))) => Some(identity.clone()),
        _ => None,
    });
    let organizations = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
        move || identity().is_some(),
        || AccountClient::current().organizations(),
    );
    let projects = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
        move || identity().is_some(),
        || AccountClient::current().projects(),
    );
    let actions = AccountActions::use_actions();
    let disabled = disabled || (actions.pending)();
    let current = status.read().clone();
    rsx! {
        div { class: "settings-account",
        match current {
            None => rsx! { p { class: "settings-help", role: "status", "Checking your Spiral sign-in…" } },
            Some(Err(error)) => rsx! { Notice { tone: NoticeTone::Caution, "{error}" } },
            Some(Ok(SpiralAccountStatus::SignedOut { problem })) => rsx! {
                if let Some(problem) = problem {
                    Notice { tone: NoticeTone::Caution, "{problem}" }
                }
                p { class: "settings-help", "Sign in with your Spiral account to choose a project." }
                Button {
                    purpose: ActionPurpose::Connect,
                    variant: ButtonVariant::Solid,
                    disabled,
                    aria_busy: (actions.pending)(),
                    onclick: move |_| actions.run(AccountClient::current().sign_in(), false),
                    "Sign in with Spiral"
                }
            },
            Some(Ok(SpiralAccountStatus::SigningIn(sign_in))) => rsx! {
                PendingSignIn { sign_in, on_restart: move |_| actions.run(AccountClient::current().sign_in(), false), disabled }
            },
            Some(Ok(SpiralAccountStatus::SignedIn(identity))) => rsx! {
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
                        title: "Signs out of Spiral on this computer, including the spiraldb CLI",
                        onclick: move |_| actions.run(AccountClient::current().sign_out(), true),
                        "Sign out"
                    }
                }
                OrganizationChoice {
                    current: identity.organization().clone(),
                    organizations: organizations.read().as_ref().and_then(|result| result.as_ref().ok()).cloned().unwrap_or_default(),
                    choice: AccountChoice::new(
                        EventHandler::new(move |organization| actions.run(AccountClient::current().switch_organization(organization), true)),
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

#[component]
fn PendingSignIn(sign_in: SpiralSignIn, on_restart: EventHandler<()>, disabled: bool) -> Element {
    rsx! {
        p { class: "settings-help",
            "Confirm the code "
            code { "{sign_in.user_code()}" }
            " on the Spiral sign-in page. Studio continues once you do."
        }
        div { class: "settings-transfer-actions",
            LinkButton {
                purpose: ActionPurpose::Navigate,
                variant: ButtonVariant::Solid,
                href: sign_in.verification_url(),
                new_tab: true,
                "Open Spiral sign-in"
            }
            Button {
                purpose: ActionPurpose::Retry,
                variant: ButtonVariant::Link,
                disabled,
                onclick: move |_| on_restart.call(()),
                "Start over"
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
