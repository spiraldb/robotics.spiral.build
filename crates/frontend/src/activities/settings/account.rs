//! Who this browser is signed in to Spiral as, in which organization. The
//! backend holds the session; this page only learns who is signed in.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::account::SpiralIdentity;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Fact, FactList, Notice, NoticeTone,
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

    fn sign_out(&self) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[QueryDomain::Project, QueryDomain::Settings],
            HttpRequest::new("POST", "/api/account/sign-out"),
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

    /// Run `command`, which closes this browser's projects.
    fn run<T: 'static>(mut self, command: impl Future<Output = Result<T, CommandError>> + 'static) {
        if *self.pending.peek() {
            return;
        }
        self.pending.set(true);
        self.failure.set(None);
        let client = ProjectClient::current();
        client.spawn(async move {
            let result = command.await;
            self.pending.set(false);
            match result {
                Ok(_) => self.selection.sign_out(),
                Err(error) => self.failure.set(Some(error)),
            }
        });
    }
}

/// Who this browser reaches Spiral as, and in which organization.
#[component]
pub(super) fn SpiralAccountSection(disabled: bool) -> Element {
    let identity = QueryPolicy::new(QueryDomain::Settings, 1_000)
        .use_resource(|| AccountClient::current().identity());
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
                            onclick: move |_| actions.run(AccountClient::current().sign_out()),
                            "Sign out"
                        }
                    }
                    Fact { label: "Organization",
                        "{identity.organization().label()}"
                    }
                }
            },
        }
        if let Some(error) = (actions.failure)() {
            Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
        }
    }
}
