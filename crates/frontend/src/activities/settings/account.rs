//! Who this browser is signed in to Spiral as, in which organization. The
//! backend holds the session; this page only learns who is signed in.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::account::{OrganizationSwitch, SpiralIdentity, SpiralOrganization};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Fact, FactList, Notice, NoticeTone, Select,
};

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
    /// Another organization's workspaces are others: start over in the
    /// first one it can open.
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
        client.spawn(async move {
            let result = command.await;
            self.pending.set(false);
            match (result, change) {
                (Ok(_), AccountChange::LeavesProject) => self.selection.forget(),
                (Ok(_), AccountChange::SignsOut) => self.selection.sign_out(),
                (Err(error), _) => self.failure.set(Some(error)),
            }
        });
    }
}

/// Who this browser reaches Spiral as, and in which organization.
#[component]
pub(super) fn SpiralAccountSection(disabled: bool) -> Element {
    let identity = QueryPolicy::new(QueryDomain::Settings, 1_000)
        .use_resource(|| AccountClient::current().identity());
    let signed_in = use_memo(move || matches!(&*identity.read(), Some(Ok(_))));
    let organizations = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
        move || *signed_in.read(),
        || AccountClient::current().organizations(),
    );
    let actions = AccountActions::use_actions();
    let disabled = disabled || (actions.pending)();
    let current = identity.read().clone();
    rsx! {
        match current {
            None => rsx! { p { class: "settings-help", role: "status", "Checking your Spiral sign-in…" } },
            Some(Err(error)) => rsx! { Notice { tone: NoticeTone::Caution, "{error}" } },
            Some(Ok(identity)) => rsx! {
                FactList { class: "settings-facts",
                    Fact { label: "Signed in as",
                        span { "{identity.email()}" }
                        Button {
                            purpose: ActionPurpose::Disconnect,
                            variant: ButtonVariant::Link,
                            disabled,
                            title: "Signs this browser out of Studio",
                            onclick: move |_| actions.run(AccountClient::current().sign_out(), AccountChange::SignsOut),
                            "Sign out"
                        }
                    }
                    Fact { label: "Organization",
                        OrganizationChoice {
                            current: identity.organization().clone(),
                            organizations: organizations.read().as_ref().and_then(|result| result.as_ref().ok()).cloned().unwrap_or_default(),
                            choice: AccountChoice::new(
                                EventHandler::new(move |organization| actions.run(AccountClient::current().switch_organization(organization), AccountChange::LeavesProject)),
                                disabled,
                            ),
                        }
                    }
                }
            },
        }
        if let Some(error) = (actions.failure)() {
            Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
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

/// The organization this browser works in, which a user in more than one
/// can switch here.
#[component]
fn OrganizationChoice(
    current: SpiralOrganization,
    organizations: Vec<SpiralOrganization>,
    choice: AccountChoice,
) -> Element {
    if organizations.len() < 2 {
        return rsx! { "{current.label()}" };
    }
    let on_organization = choice.on_choose;
    rsx! {
        Select {
            value: current.id().to_owned(),
            disabled: choice.disabled,
            aria_label: "Organization",
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
