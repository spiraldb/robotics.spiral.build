//! Making a workspace, and deleting the mounted one.

use dioxus::prelude::*;
use spiral_robotics_common::account::SpiralProject;
use spiral_robotics_common::workspace::NewWorkspace;
use spiral_robotics_ui::{
    ActionPurpose, BlockingConfirmation, Button, ButtonVariant, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, Field, FieldLayout, Notice, NoticeTone, Select,
    TextInput,
};

use crate::client::CommandError;
use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::routes::StudioRoute;

use super::panel::SettingsHeading;
use crate::shell::workspace::{WorkspaceClient, WorkspaceSwitch};

/// A name, and the Spiral project the workspace works on: a new one, or one
/// the user can already open. Making it moves this browser into it.
#[component]
pub(super) fn NewWorkspaceForm(cancellable: bool) -> Element {
    let mut name = use_signal(String::new);
    let mut project = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut failure = use_signal(|| None::<CommandError>);
    let projects = QueryPolicy::on_demand(QueryDomain::Settings)
        .use_resource(|| WorkspaceClient::current().spiral_projects());
    let navigator = use_navigator();
    let switch = WorkspaceSwitch::current();
    let named = !name.read().trim().is_empty();
    let mut create = move || {
        if *pending.peek() || name.peek().trim().is_empty() {
            return;
        }
        let existing = project.peek().clone();
        let request = NewWorkspace::new(
            name.peek().clone(),
            (!existing.is_empty()).then_some(existing),
        );
        pending.set(true);
        failure.set(None);
        ProjectClient::current().spawn(async move {
            let made = match WorkspaceClient::current().create(&request).await {
                Ok(workspace) => switch.to(workspace.project_id()).await,
                Err(error) => Err(error),
            };
            // Moving remounts Studio on the new workspace, and this form with
            // it; only a failure stays here to be read.
            match made {
                Ok(()) => {
                    navigator.push(StudioRoute::Data {});
                }
                Err(error) => {
                    failure.set(Some(error));
                    pending.set(false);
                }
            }
        });
    };
    rsx! {
        div { class: "settings-new-workspace",
            Field { label: "Name", layout: FieldLayout::Inline, class: "settings-field",
                TextInput {
                    value: name(),
                    placeholder: "Lab arms",
                    disabled: pending(),
                    oninput: move |value| name.set(value),
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Enter {
                            create();
                        }
                    },
                }
            }
            Field { label: "Spiral project", layout: FieldLayout::Inline, class: "settings-field",
                Select {
                    value: project(),
                    disabled: pending(),
                    onchange: move |chosen| project.set(chosen),
                    option { value: "", selected: project().is_empty(), "A new project" }
                    ProjectOptions { projects: projects.read().clone(), chosen: project() }
                }
            }
            p { class: "settings-help settings-control-column",
                "A new project goes in your current organization. An existing one keeps its datasets, and its members see this workspace."
            }
            div { class: "settings-transfer-actions settings-control-column",
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Solid,
                    disabled: pending() || !named,
                    aria_busy: pending(),
                    onclick: move |_| create(),
                    if pending() { "Creating…" } else { "Create workspace" }
                }
                if cancellable {
                    Button {
                        purpose: ActionPurpose::Cancel,
                        disabled: pending(),
                        onclick: move |_| { navigator.push(StudioRoute::Settings {}); },
                        "Cancel"
                    }
                }
            }
            if let Some(error) = failure() {
                Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
            }
        }
    }
}

/// The Spiral projects a new workspace may use, once listed.
#[component]
fn ProjectOptions(
    projects: Option<Result<Vec<SpiralProject>, CommandError>>,
    chosen: String,
) -> Element {
    match projects {
        None => rsx! { option { disabled: true, "Loading your projects…" } },
        Some(Err(error)) => rsx! { option { disabled: true, "{error}" } },
        Some(Ok(projects)) => rsx! {
            for project in projects {
                option {
                    key: "{project.id()}",
                    value: "{project.id()}",
                    selected: project.id() == chosen,
                    {project.description().map_or_else(|| project.id().to_owned(), |description| format!("{} — {description}", project.id()))}
                }
            }
        },
    }
}

/// Delete the mounted workspace for everyone, and move this browser on.
#[component]
pub(super) fn DeleteWorkspace(name: String, project_id: String) -> Element {
    const OPENER: &str = "workspace-delete";
    let mut confirming = use_signal(|| false);
    let mut pending = use_signal(|| false);
    let mut failure = use_signal(|| None::<CommandError>);
    let switch = WorkspaceSwitch::current();
    let delete = move |()| {
        pending.set(true);
        failure.set(None);
        let project_id = project_id.clone();
        ProjectClient::current().spawn(async move {
            match WorkspaceClient::current().delete(&project_id).await {
                // Leaving remounts Studio elsewhere, and this section with it.
                Ok(()) => switch.leave(),
                Err(error) => {
                    failure.set(Some(error));
                    pending.set(false);
                    confirming.set(false);
                }
            }
        });
    };
    rsx! {
        section { class: "settings-group", "aria-labelledby": "workspace-delete-title",
            SettingsHeading { title: "Delete workspace", id: "workspace-delete-title" }
            p { class: "settings-help",
                "Removes it for everyone, with its curations, preferences, and training history. The Spiral project and its datasets stay."
            }
            div { class: "settings-transfer-actions",
                Button {
                    id: OPENER,
                    purpose: ActionPurpose::Delete,
                    variant: ButtonVariant::Outline,
                    disabled: pending(),
                    onclick: move |_| confirming.set(true),
                    "Delete workspace"
                }
            }
            if let Some(error) = failure() {
                Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
            }
        }
        if confirming() {
            BlockingConfirmation {
                id: "workspace-delete-confirmation",
                title: format!("Delete workspace {name}?"),
                consequence: "Everyone loses its curations, preferences, and training history in Studio. The Spiral project and its datasets stay.".to_owned(),
                copy: ConfirmationCopy::new(
                    format!("Delete {name} for everyone who can open it."),
                    if pending() { "Deleting…" } else { "Delete workspace" },
                    "Keep workspace",
                ),
                response: ConfirmationResponse::new(
                    ActionPurpose::Delete,
                    delete,
                    move |()| confirming.set(false),
                ),
                dismissal: ConfirmationDismissal::new(OPENER).with_pending(pending()),
            }
        }
    }
}
