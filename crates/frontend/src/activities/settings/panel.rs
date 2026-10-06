//! Settings screen for the workspace and the Spiral account. The robots are
//! set up where they are driven, in Rollout.

use crate::activities::settings::SettingsClient;
use dioxus::dioxus_core::CapturedError;
use dioxus::prelude::*;
use spiral_robotics_common::settings::{ConnectionTest, DataSettings};
use spiral_robotics_ui::{
    ActionPurpose, BlockingConfirmation, BrowserFocus, Button, ButtonVariant, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, Fact, FactList, Field, FieldLayout, FileButton,
    FileInput, LinkButton, TextInput,
};

use crate::shell::notifications::MessageSource;
use crate::shell::routes::StudioRoute;
use crate::shell::state::StudioProjectState;
use crate::shell::workspace::WorkspaceSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Workspace,
    Account,
}

#[derive(Clone, Copy)]
struct SettingsPanelState {
    draft: SettingsDraftSignals,
    operations: SettingsOperationSignals,
}

#[derive(Clone, Copy)]
struct SettingsDraftSignals {
    current: Signal<DataSettings>,
    base: Signal<DataSettings>,
    saved: Signal<Option<DataSettings>>,
}

#[derive(Clone, Copy)]
struct SettingsOperationSignals {
    saving: Signal<bool>,
    testing: Signal<bool>,
    test_result: Signal<Option<ConnectionTest>>,
    save_error: Signal<Option<crate::client::CommandError>>,
    project_transfer: ProjectTransferSignals,
}

#[derive(Clone, Copy)]
struct ProjectTransferSignals {
    pending_file: Signal<Option<dioxus::html::FileData>>,
    error: Signal<Option<String>>,
}

impl SettingsPanelState {
    fn use_state(initial: DataSettings) -> Self {
        use_hook(|| Self {
            draft: SettingsDraftSignals {
                base: Signal::new(initial.clone()),
                saved: Signal::new(None),
                current: Signal::new(initial),
            },
            operations: SettingsOperationSignals {
                saving: Signal::new(false),
                testing: Signal::new(false),
                test_result: Signal::new(None),
                save_error: Signal::new(None),
                project_transfer: ProjectTransferSignals {
                    pending_file: Signal::new(None),
                    error: Signal::new(None),
                },
            },
        })
    }
}

/// Where the panel opens: a section, and whether the Workspace section makes
/// a new workspace instead of showing this one.
#[derive(Clone, Copy, PartialEq)]
struct SettingsPlace {
    section: Signal<SettingsSection>,
    creating: bool,
}

#[component]
fn SettingsPanel(project: StudioProjectState, place: SettingsPlace) -> Element {
    let SettingsPlace {
        mut section,
        creating,
    } = place;
    let settings = WorkspaceSettings::current();
    let initial = settings.loaded();
    let load_error = settings.error();
    let initial_value = initial.clone().unwrap_or_default();
    let settings_loaded = initial.is_some();
    let chosen = crate::client::project::ProjectClient::current()
        .project()
        .is_chosen();
    let SettingsPanelState {
        draft:
            SettingsDraftSignals {
                mut current,
                mut base,
                mut saved,
            },
        operations:
            SettingsOperationSignals {
                mut saving,
                mut testing,
                mut test_result,
                mut save_error,
                project_transfer:
                    ProjectTransferSignals {
                        pending_file: mut pending_project_file,
                        error: mut project_transfer_error,
                    },
            },
    } = SettingsPanelState::use_state(initial_value);
    let on_saved = move || {
        settings.refresh();
    };

    let mut import_project = use_action(move |file: dioxus::html::FileData| async move {
        project_transfer_error.set(None);
        match SettingsClient::current().import_project(file).await {
            Ok(result) => {
                on_saved();
                pending_project_file.set(None);
                project_transfer_error.set(None);
                project.report_success_from(
                    MessageSource::Settings,
                    format!(
                        "Studio records restored; {} data relations ({} rows) imported.",
                        result.relations(),
                        result.rows(),
                    ),
                );
                Ok(())
            }
            Err(error) => {
                pending_project_file.set(None);
                project_transfer_error.set(Some(error.to_string()));
                project.report_from(MessageSource::Settings, error.to_string());
                BrowserFocus::after_render("project-import-input".to_owned());
                Err(CapturedError::from_display(error))
            }
        }
    });

    let activity = crate::shell::activity::WorkspaceActivity::current();
    use_effect(move || {
        if activity.is_active() {
            BrowserFocus::after_render("data-settings-panel".to_owned());
        }
    });

    use_effect(use_reactive((&initial,), move |(initial,)| {
        let acknowledged = saved();
        let busy = saving() || import_project.pending();
        if let Some(settings) = initial
            && !busy
            && *base.peek() != settings
        {
            let unchanged = *current.peek() == *base.peek();
            let saved_draft = acknowledged
                .as_ref()
                .is_some_and(|submitted| submitted == &*current.peek());
            if (unchanged || saved_draft) && *current.peek() != settings {
                current.set(settings.clone());
            }
            if *base.peek() != settings {
                base.set(settings);
            }
            if acknowledged.is_some() {
                saved.set(None);
            }
        }
    }));

    let mut save = move || {
        if saving() || import_project.pending() || !settings_loaded {
            return;
        }
        save_error.set(None);
        saving.set(true);
        let submitted = current();
        let update = spiral_robotics_common::settings::SettingsUpdate::new(submitted.name());
        crate::client::project::ProjectClient::current().spawn(async move {
            match SettingsClient::current().update_data_settings(update).await {
                Ok(_) => {
                    saved.set(Some(submitted));
                    on_saved();
                    project.report_success_from(
                        MessageSource::Settings,
                        "Settings saved and applied.",
                    );
                }
                Err(error) => save_error.set(Some(error)),
            }
            saving.set(false);
        });
    };
    // Testing saves nothing; the footer's Save is the one way a draft is kept.
    let mut test_connection = move || {
        if testing() {
            return;
        }
        testing.set(true);
        test_result.set(None);
        crate::client::project::ProjectClient::current().spawn(async move {
            let tested = SettingsClient::current()
                .test_spiral_connection()
                .await
                .unwrap_or_else(|error| {
                    ConnectionTest::new(false, format!("{error} {}", error.recovery_guidance()))
                });
            test_result.set(Some(tested));
            testing.set(false);
        });
    };
    let test_disabled = saving() || testing() || import_project.pending();
    let connection_result = test_result().map(|response| rsx! {
        p {
            class: if response.connected() { "connection-test connection-test-ok settings-control-column" } else { "connection-test connection-test-failed settings-control-column" },
            role: "status",
            "{response.message()}"
        }
    });
    let project_transfer_ready = settings_loaded && chosen;
    let workspace_name = initial
        .as_ref()
        .map(|settings| settings.name().to_owned())
        .unwrap_or_default();
    let project_id = initial
        .as_ref()
        .map(|settings| settings.spiral().project_id().to_owned())
        .unwrap_or_default();
    let shows_draft = !creating && section() == SettingsSection::Workspace;

    rsx! {
        section {
            id: "data-settings-panel",
            class: "data-settings-panel",
            role: "region",
            "aria-labelledby": "data-settings-title",
            "aria-hidden": pending_project_file().is_some(),
            tabindex: "-1",
            header { class: "settings-header",
                div {
                    h2 { id: "data-settings-title", "Settings" }
                }
            }
            div { class: "settings-layout",
                nav { class: "settings-nav", "aria-label": "Settings sections",
                    for (label, target) in [
                        ("Workspace", SettingsSection::Workspace),
                        ("Account", SettingsSection::Account),
                    ] {
                        SettingsSectionAction { key: "{label}", label, active: section() == target, onclick: move |_| { section.set(target); test_result.set(None); save_error.set(None); } }
                    }
                }
                div { class: "settings-content",
                    match section() {
                        SettingsSection::Account => rsx! {
                            SettingsHeading { title: "Spiral account" }
                            super::account::SpiralAccountSection { disabled: saving() || import_project.pending() }
                        },
                        SettingsSection::Workspace if creating => rsx! {
                            SettingsHeading { title: "New workspace" }
                            super::workspace::NewWorkspaceForm { cancellable: chosen }
                        },
                        _ if !settings_loaded => rsx! {
                            div { class: "settings-loading", role: "status",
                                if let Some(error) = load_error.as_ref() { span { "{error}" } } else { span { "Loading settings…" } }
                                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Link, onclick: move |_| settings.refresh(), "Retry" }
                            }
                        },
                        SettingsSection::Workspace => rsx! {
                            SettingsHeading { title: "Workspace" }
                            SettingsField {
                                label: "Name",
                                value: current().name(),
                                placeholder: "Lab arms",
                                oninput: move |value| current.write().set_name(value),
                            }
                            FactList { class: "settings-facts",
                                Fact { label: "Spiral project",
                                    code { "{project_id}" }
                                    Button {
                                        purpose: ActionPurpose::Refresh,
                                        variant: ButtonVariant::Link,
                                        disabled: test_disabled,
                                        aria_busy: testing(),
                                        title: "Check that Studio can reach this Spiral project",
                                        onclick: move |_| test_connection(),
                                        if testing() { "Testing…" } else { "Test connection" }
                                    }
                                }
                            }
                            {connection_result}
                            section { class: "settings-group", "aria-labelledby": "project-transfer-title",
                                SettingsHeading { title: "Workspace backup", id: "project-transfer-title" }
                                p { class: "settings-help", "Import replaces this workspace’s data and Studio records." }
                                div { class: "settings-transfer-actions",
                                    FileButton {
                                        purpose: ActionPurpose::Import,
                                        variant: ButtonVariant::Outline,
                                        title: "Replace this workspace’s data and Studio records",
                                        disabled: saving() || import_project.pending() || !project_transfer_ready,
                                        aria_busy: import_project.pending(),
                                        input: FileInput::new(
                                            ".spiral-project,application/vnd.spiral-robotics.project",
                                            move |event: Event<FormData>| {
                                                let Some(file) = event.files().first().cloned() else { return };
                                                project_transfer_error.set(None);
                                                pending_project_file.set(Some(file));
                                            },
                                        ).with_id("project-import-input"),
                                        "Import backup"
                                    }
                                    if import_project.pending() || !project_transfer_ready {
                                        Button { purpose: ActionPurpose::Export, variant: ButtonVariant::Outline, title: "A backup is being restored", disabled: true, "Export backup" }
                                    } else {
                                        LinkButton {
                                            purpose: ActionPurpose::Export,
                                            variant: ButtonVariant::Outline,
                                            href: crate::client::project::ProjectClient::current().scoped_url("/api/project/export"),
                                            download: "{project_id}.spiral-project",
                                            title: "Export the workspace’s data and Studio records",
                                            "Export backup"
                                        }
                                    }
                                }
                                if let Some(error) = project_transfer_error() {
                                    p {
                                        class: "connection-test connection-test-failed",
                                        role: "status",
                                        "Import failed. {error}"
                                    }
                                }
                            }
                            super::workspace::DeleteWorkspace { name: workspace_name.clone(), project_id: project_id.clone() }
                        },
                    }
                }
            }
            if let Some(error) = save_error() {
                div {
                    class: "settings-save-error",
                    role: "alert",
                    "aria-live": "assertive",
                    div {
                        strong { "Settings weren’t saved." }
                        span { "{error}" }
                        span { "{error.recovery_guidance()}" }
                    }
                    Button {
                        purpose: ActionPurpose::Close,
                        aria_label: "Dismiss settings error",
                        onclick: move |_| save_error.set(None),
                        "Dismiss"
                    }
                }
            }
            if shows_draft {
                footer { class: "settings-footer",
                    Button { purpose: ActionPurpose::Cancel, disabled: saving() || import_project.pending(), onclick: move |_| { current.set(base()); save_error.set(None); test_result.set(None); }, "Discard changes" }
                    Button { purpose: ActionPurpose::Apply, variant: ButtonVariant::Solid, disabled: saving() || import_project.pending() || !settings_loaded || !chosen, aria_busy: saving(), onclick: move |_| save(), if saving() { "Saving…" } else { "Save settings" } }
                }
            }
        }
        if let Some(file) = pending_project_file() {
            BlockingConfirmation {
                id: "project-import-confirmation",
                title: format!("Replace workspace {workspace_name}?"),
                consequence: "This workspace’s Studio records and data will be replaced for everyone in it, including recordings, preferences, curations, training history, and checkpoints. Robots stay on the robot connector.".to_owned(),
                copy: ConfirmationCopy::new(
                    format!("Import {} into this workspace.", file.name()),
                    if import_project.pending() { "Restoring…" } else { "Import and replace" },
                    "Keep current data",
                ),
                response: ConfirmationResponse::new(ActionPurpose::Replace, move |_| {
                    import_project.call(file.clone());
                }, move |_| pending_project_file.set(None)),
                dismissal: ConfirmationDismissal::new("project-import-input")
                    .with_pending(import_project.pending()),
            }
        }
    }
}

#[component]
fn SettingsSectionAction(label: &'static str, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        Button {
            purpose: ActionPurpose::Navigate,
            variant: ButtonVariant::Row,
            aria_current: active.then(|| "page".to_owned()),
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

/// The name of a section or of a group inside one. What a section holds is
/// said by its fields and facts, not by a sentence under its name.
#[component]
pub(super) fn SettingsHeading(
    title: &'static str,
    #[props(default)] id: Option<&'static str>,
) -> Element {
    rsx! { div { class: "provider-heading", h3 { id, "{title}" } } }
}

/// A text setting on the section's label column.
#[component]
fn SettingsField(
    label: &'static str,
    value: String,
    placeholder: &'static str,
    oninput: EventHandler<String>,
) -> Element {
    rsx! {
        Field { label, layout: FieldLayout::Inline, class: "settings-field",
            TextInput { value, placeholder, oninput }
        }
    }
}

#[component]
pub(crate) fn SettingsWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Settings,
        is_active,
    );
    let project = crate::shell::state::StudioProjectState::current();
    let route = use_route::<StudioRoute>();
    let mut section = use_signal(|| SettingsSection::Workspace);
    // Opening a new workspace shows the Workspace section, whichever section
    // was open.
    use_effect(use_reactive((&route,), move |(route,)| {
        if route == (StudioRoute::NewWorkspace {}) {
            section.set(SettingsSection::Workspace);
        }
    }));
    let chosen = crate::client::project::ProjectClient::current()
        .project()
        .is_chosen();
    let creating = !chosen || route == (StudioRoute::NewWorkspace {});
    rsx! {
        crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::General,
            SettingsPanel {
                project,
                place: SettingsPlace { section, creating },
            }
        }
    }
}
