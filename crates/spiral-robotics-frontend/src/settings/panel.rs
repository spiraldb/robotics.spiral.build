//! Settings screen for robots, SpiralDB, training, and video.

use crate::components::interaction::{
    Action, ActionLink, ActionPurpose, ActionVariant, BlockingConfirmation, CloseIcon, FileAction,
    Toggle, focus_after_render,
};
use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::api::{
    ConnectionTestResponse, DataSettings, ProjectImportSummary, UpdateDataSettingsRequest,
};

use crate::client::http as api;
use crate::settings::robots::RobotPanel;
use crate::shell::notifications::MessageSource;
use crate::shell::state::studio;
use crate::tasks::single_flight;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Robots,
    Spiral,
    Training,
    Video,
}

#[component]
pub(crate) fn SettingsPanel(
    initial: Option<DataSettings>,
    load_error: Option<String>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
    on_busy_change: EventHandler<bool>,
) -> Element {
    let initial_value = initial.clone().unwrap_or_default();
    let settings_loaded = initial.is_some();
    let initial_section = if initial_value.spiral.project_id.trim().is_empty() {
        SettingsSection::Spiral
    } else {
        SettingsSection::Robots
    };
    let mut current = use_signal(move || initial_value);
    let mut client_secret = use_signal(String::new);
    let mut section = use_signal(move || initial_section);
    let mut saving = use_signal(|| false);
    let mut testing = use_signal(|| false);
    let transferring_project = use_signal(|| false);
    let mut test_result = use_signal(|| None::<ConnectionTestResponse>);
    let mut save_error = use_signal(|| None::<String>);
    let mut pending_project_file = use_signal(|| None::<dioxus::html::FileData>);
    let mut project_transfer_error = use_signal(|| None::<String>);
    let mut initial_focus_set = use_signal(|| false);
    let mut studio = studio();

    use_effect(move || {
        if !initial_focus_set() {
            focus_after_render("data-settings-panel".to_owned());
            initial_focus_set.set(true);
        }
    });

    use_effect(use_reactive((&initial,), move |(initial,)| {
        if let Some(settings) = initial
            && !saving.peek().to_owned()
            && !transferring_project.peek().to_owned()
        {
            current.set(settings);
        }
    }));

    let mut save = move |test_path: Option<&'static str>| {
        if saving() || transferring_project() || !settings_loaded {
            return;
        }
        save_error.set(None);
        saving.set(true);
        on_busy_change.call(true);
        let request = UpdateDataSettingsRequest {
            spiral_endpoint: current().spiral.endpoint,
            spiral_spfs_endpoint: current().spiral.spfs_endpoint,
            project_id: current().spiral.project_id,
            client_id: current().spiral.client_id,
            client_secret: client_secret(),
            clear_client_secret: current().spiral.client_id.trim().is_empty()
                && current().spiral.client_secret_configured,
            training: current().training,
            video: current().video,
        };
        spawn(async move {
            match api::send_json::<_, DataSettings>(
                Method::PATCH,
                "/api/settings/data",
                Some(&request),
            )
            .await
            {
                Ok(settings) => {
                    current.set(settings);
                    client_secret.set(String::new());
                    on_saved.call(());
                    if let Some(path) = test_path {
                        testing.set(true);
                        test_result.set(None);
                        match api::send_json::<(), ConnectionTestResponse>(Method::POST, path, None)
                            .await
                        {
                            Ok(response) => test_result.set(Some(response)),
                            Err(error) => test_result.set(Some(ConnectionTestResponse {
                                ok: false,
                                message: error,
                            })),
                        }
                        testing.set(false);
                    } else {
                        studio.report_success_from(
                            MessageSource::Settings,
                            "Settings saved and applied.",
                        );
                    }
                }
                Err(error) => save_error.set(Some(error)),
            }
            saving.set(false);
            on_busy_change.call(false);
        });
    };
    let configured_destination = current();
    let project_transfer_ready = initial.as_ref().is_some_and(|active| {
        !configured_destination.spiral.project_id.trim().is_empty()
            && active.spiral.endpoint == configured_destination.spiral.endpoint
            && active.spiral.spfs_endpoint == configured_destination.spiral.spfs_endpoint
            && active.spiral.project_id == configured_destination.spiral.project_id
    });

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
                    p { "Robots, data, training, and video." }
                }
                Action {
                    id: "settings-close-action",
                    purpose: ActionPurpose::Close,
                    variant: ActionVariant::Icon,
                    aria_label: "Close settings",
                    disabled: saving() || transferring_project(),
                    onclick: move |_| if !saving() && !transferring_project() { on_close.call(()) },
                    CloseIcon {}
                }
            }
            div { class: "settings-layout",
                nav { class: "settings-nav", "aria-label": "Settings sections",
                    SettingsSectionAction { label: "Robots", active: section() == SettingsSection::Robots, onclick: move |_| { section.set(SettingsSection::Robots); test_result.set(None); save_error.set(None); } }
                    SettingsSectionAction { label: "SpiralDB", active: section() == SettingsSection::Spiral, onclick: move |_| { section.set(SettingsSection::Spiral); test_result.set(None); save_error.set(None); } }
                    SettingsSectionAction { label: "Training", active: section() == SettingsSection::Training, onclick: move |_| { section.set(SettingsSection::Training); test_result.set(None); save_error.set(None); } }
                    SettingsSectionAction { label: "Video", active: section() == SettingsSection::Video, onclick: move |_| { section.set(SettingsSection::Video); test_result.set(None); save_error.set(None); } }
                }
                div { class: "settings-content",
                    if section() == SettingsSection::Robots {
                        ProviderHeading { title: "Robot setup", description: "Configure ports, camera assignments, and calibration for each arm." }
                        div { class: "robot-settings-panel", RobotPanel {} }
                    } else if let Some(error) = load_error.as_ref() {
                        div { class: "settings-loading data-error", "{error}" }
                    } else if !settings_loaded {
                        div { class: "settings-loading", "Loading data settings…" }
                    } else {
                        match section() {
                            SettingsSection::Robots => rsx! {},
                            SettingsSection::Spiral => rsx! {
                                ProviderHeading { title: "SpiralDB", description: "The one persistent store for robots, episodes, video, runs, and Studio preferences." }
                                div { class: "settings-field-row",
                                    SettingsField {
                                        label: "API endpoint",
                                        value: current().spiral.endpoint,
                                        placeholder: "https://api.spiraldb.dev",
                                        oninput: move |value| current.write().spiral.endpoint = value,
                                    }
                                    SettingsField {
                                        label: "SpFS endpoint",
                                        value: current().spiral.spfs_endpoint,
                                        placeholder: "https://spfs.spiraldb.dev",
                                        oninput: move |value| current.write().spiral.spfs_endpoint = value,
                                    }
                                }
                                div { class: "settings-field-row",
                                    SettingsField {
                                        label: "Project ID",
                                        value: current().spiral.project_id,
                                        placeholder: "your-project-id",
                                        oninput: move |value| current.write().spiral.project_id = value,
                                    }
                                    SettingsField { label: "Namespace", value: current().spiral.namespace, placeholder: "spiral_robotics", disabled: true, oninput: move |_| {} }
                                }
                                div { class: "settings-field-row",
                                    SettingsField {
                                        label: "Client ID",
                                        value: current().spiral.client_id,
                                        placeholder: "your-client-id",
                                        oninput: move |value| current.write().spiral.client_id = value,
                                    }
                                    SettingsField {
                                        label: "Client secret",
                                        value: client_secret(),
                                        placeholder: if current().spiral.client_secret_configured { "Saved — leave blank to keep" } else { "your-client-secret" },
                                        secret: true,
                                        oninput: move |value| client_secret.set(value),
                                    }
                                }
                                p { class: "settings-help",
                                    "The API, SpFS endpoint, project, and optional client credentials are applied to a new scoped SpiralDB connection when you save. The secret is stored only in the backend's private bootstrap file and is never returned to this page."
                                }
                                Action {
                                    purpose: ActionPurpose::Apply,
                                    variant: ActionVariant::Outline,
                                    disabled: saving() || testing() || transferring_project() || current().spiral.project_id.trim().is_empty(),
                                    aria_busy: testing(),
                                    onclick: move |_| save(Some("/api/settings/data/test/spiral")),
                                    if testing() { "Testing…" } else { "Save and test connection" }
                                }
                                section { class: "settings-project-transfer", "aria-labelledby": "project-transfer-title",
                                    div { class: "provider-heading",
                                        h3 { id: "project-transfer-title", "Project backup" }
                                        p { "Transfer the complete active SpiralDB project as one portable file." }
                                    }
                                    p { class: "settings-help",
                                        "The file contains all Studio-managed data: robots, datasets, episodes, streams, video, preferences, training runs and metrics, checkpoints, and rollouts. Import replaces every existing Studio relation in the active project."
                                    }
                                    div { class: "settings-transfer-actions",
                                        FileAction {
                                            input_id: "project-import-input",
                                            purpose: ActionPurpose::Import,
                                            variant: ActionVariant::Outline,
                                            title: if project_transfer_ready { "Replace all Studio-managed relations in the active SpiralDB project" } else { "Save the Spiral destination before transferring a project" },
                                            accept: ".spiral-project,application/vnd.spiral-robotics.project",
                                            disabled: saving() || transferring_project() || !project_transfer_ready,
                                            aria_busy: transferring_project(),
                                            onchange: move |event: Event<FormData>| {
                                                let Some(file) = event.files().first().cloned() else { return };
                                                project_transfer_error.set(None);
                                                pending_project_file.set(Some(file));
                                            },
                                            "Import project"
                                        }
                                        if transferring_project() || !project_transfer_ready {
                                            Action { purpose: ActionPurpose::Export, variant: ActionVariant::Outline, title: if project_transfer_ready { "Project transfer is in progress" } else { "Save the Spiral destination before transferring a project" }, disabled: true, "Export project" }
                                        } else {
                                            ActionLink {
                                                purpose: ActionPurpose::Export,
                                                variant: ActionVariant::Outline,
                                                href: "/api/project/export",
                                                download: "spiral-robotics-project.spiral-project",
                                                title: "Export every Studio-managed relation from the active SpiralDB project",
                                                "Export project"
                                            }
                                        }
                                    }
                                    if let Some(error) = project_transfer_error() {
                                        p {
                                            class: "connection-test connection-test-failed",
                                            role: "status",
                                            "Project import failed. {error}"
                                        }
                                    }
                                }
                            },
                            SettingsSection::Training => rsx! {
                                ProviderHeading { title: "Policy compute runtime", description: "Run training and rollout inference in a remote PyTorch process." }
                                SettingsField {
                                    label: "Runtime endpoint",
                                    value: current().training.endpoint,
                                    placeholder: "http://127.0.0.1:50051",
                                    oninput: move |value| current.write().training.endpoint = value,
                                }
                                p { class: "settings-help",
                                    "Studio uses Arrow IPC over HTTP. The selected endpoint applies to new training and rollout sessions; an active session keeps its current connection."
                                }
                                Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline, disabled: saving() || testing() || transferring_project(), aria_busy: testing(), onclick: move |_| save(Some("/api/settings/data/test/training")), if testing() { "Connecting…" } else { "Save and connect" } }
                            },
                            SettingsSection::Video => rsx! {
                                ProviderHeading { title: "Video", description: "Include assigned cameras in new episodes." }
                                Toggle { label: "Record video", checked: current().video.enabled, onchange: move |value| current.write().video.enabled = value }
                            },
                        }
                        if let Some(response) = test_result() {
                            p {
                                class: if response.ok { "connection-test connection-test-ok" } else { "connection-test connection-test-failed" },
                                role: "status",
                                "{response.message}"
                            }
                        }
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
                    }
                    Action {
                        purpose: ActionPurpose::Close,
                        aria_label: "Dismiss settings error",
                        onclick: move |_| save_error.set(None),
                        "Dismiss"
                    }
                }
            }
            footer { class: "settings-footer",
                if section() == SettingsSection::Robots {
                    span { "Robot changes are saved immediately." }
                    div { Action { purpose: ActionPurpose::Close, variant: ActionVariant::Solid, disabled: saving() || transferring_project(), onclick: move |_| if !saving() && !transferring_project() { on_close.call(()) }, "Done" } }
                } else {
                    span {
                        "Applies to this Studio."
                    }
                    div {
                        Action { purpose: ActionPurpose::Cancel, disabled: saving() || transferring_project(), onclick: move |_| if !saving() && !transferring_project() { on_close.call(()) }, "Cancel" }
                        Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Solid, disabled: saving() || transferring_project() || current().spiral.project_id.trim().is_empty(), aria_busy: saving(), onclick: move |_| save(None), if saving() { "Saving…" } else { "Save settings" } }
                    }
                }
            }
        }
        if let Some(file) = pending_project_file() {
            BlockingConfirmation {
                id: "project-import-confirmation",
                title: format!("Replace project {}?", current().spiral.project_id),
                message: format!(
                    "Import {} into the active SpiralDB project.",
                    file.name(),
                ),
                consequence: "Every Studio-managed relation in this project will be replaced, including robots, episodes, media, settings, training history, checkpoints, and rollouts. Connected robots will disconnect first; imported robots will then connect automatically.".to_owned(),
                confirm_label: if transferring_project() { "Restoring…" } else { "Import and replace project" },
                cancel_label: "Keep current project",
                purpose: ActionPurpose::Replace,
                pending: transferring_project(),
                return_focus_id: "project-import-input",
                on_confirm: move |_| {
                    if transferring_project() {
                        return;
                    }
                    let file = file.clone();
                    project_transfer_error.set(None);
                    on_busy_change.call(true);
                    spawn(async move {
                        let Some(outcome) = single_flight(transferring_project, async {
                                let result = api::upload_file_json::<ProjectImportSummary>(
                                    "/api/project/import",
                                    &file,
                                    "application/vnd.spiral-robotics.project",
                                )
                                .await?;
                                studio.selected_episode.set(None);
                                studio.refresh_episode_catalog().await?;
                                let settings = api::send_json::<(), DataSettings>(
                                    Method::GET,
                                    "/api/settings/data",
                                    None,
                                )
                                .await?;
                                Ok::<_, String>((result, settings))
                            })
                            .await
                        else {
                            on_busy_change.call(false);
                            return;
                        };
                        match outcome {
                            Ok((result, settings)) => {
                                current.set(settings);
                                on_saved.call(());
                                pending_project_file.set(None);
                                project_transfer_error.set(None);
                                studio.report_success_from(
                                    MessageSource::Settings,
                                    format!(
                                        "Project restored from {} relations ({} rows).{}",
                                        result.relations,
                                        result.rows,
                                        import_warning_suffix(&result.warnings),
                                    ),
                                );
                            }
                            Err(error) => {
                                pending_project_file.set(None);
                                project_transfer_error.set(Some(error.clone()));
                                studio.report_from(MessageSource::Settings, error);
                                focus_after_render("project-import-input".to_owned());
                            }
                        }
                        on_busy_change.call(false);
                    });
                },
                on_cancel: move |_| pending_project_file.set(None),
            }
        }
    }
}

fn import_warning_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!(" Warning: {}", warnings.join(" "))
    }
}

#[component]
fn SettingsSectionAction(label: &'static str, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        Action {
            purpose: ActionPurpose::Navigate,
            variant: ActionVariant::Row,
            aria_current: active.then(|| "page".to_owned()),
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
fn ProviderHeading(title: &'static str, description: &'static str) -> Element {
    rsx! { div { class: "provider-heading", h3 { "{title}" } p { "{description}" } } }
}

#[component]
fn SettingsField(
    label: &'static str,
    value: String,
    placeholder: String,
    #[props(default)] disabled: bool,
    #[props(default)] secret: bool,
    oninput: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "settings-field",
            span { class: "label", "{label}" }
            input { class: "field", r#type: if secret { "password" } else { "text" }, value, placeholder, disabled, oninput: move |event| oninput.call(event.value()) }
        }
    }
}
