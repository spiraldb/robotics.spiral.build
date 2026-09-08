//! Settings screen for robots, SpiralDB, training, and video.

use crate::components::interaction::{
    Action, ActionLink, ActionPurpose, ActionVariant, BlockingConfirmation, BrowserFocus,
    ConfirmationCopy, ConfirmationDismissal, ConfirmationResponse, FileAction, FileInput, Toggle,
};
use crate::settings::SettingsClient;
use dioxus::prelude::*;
use spiral_robotics_common::settings::{DataSettings, SpiralConnectionUpdate};

use crate::settings::robots::RobotPanel;
use crate::shell::notifications::MessageSource;
use crate::shell::state::studio;
use crate::tasks::SingleFlight;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Robots,
    Spiral,
    Training,
    Video,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SettingsLoadState {
    initial: Option<DataSettings>,
    load_error: Option<String>,
}

impl SettingsLoadState {
    pub(crate) const fn new(initial: Option<DataSettings>, load_error: Option<String>) -> Self {
        Self {
            initial,
            load_error,
        }
    }
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
    client_secret: Signal<String>,
    section: Signal<SettingsSection>,
}

#[derive(Clone, Copy)]
struct SettingsOperationSignals {
    saving: Signal<bool>,
    testing: Signal<bool>,
    transferring_project: Signal<bool>,
    test_result: Signal<Option<spiral_robotics_common::settings::ConnectionTest>>,
    save_error: Signal<Option<crate::client::CommandError>>,
    project_transfer: ProjectTransferSignals,
}

#[derive(Clone, Copy)]
struct ProjectTransferSignals {
    pending_file: Signal<Option<dioxus::html::FileData>>,
    error: Signal<Option<String>>,
}

impl SettingsPanelState {
    fn use_state(initial: DataSettings, section: SettingsSection) -> Self {
        crate::shell::view_state::ViewState::use_retained(|| Self {
            draft: SettingsDraftSignals {
                base: Signal::new(initial.clone()),
                saved: Signal::new(None),
                current: Signal::new(initial),
                client_secret: Signal::new(String::new()),
                section: Signal::new(section),
            },
            operations: SettingsOperationSignals {
                saving: Signal::new(false),
                testing: Signal::new(false),
                transferring_project: Signal::new(false),
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

#[component]
pub(crate) fn SettingsPanel(state: SettingsLoadState) -> Element {
    let SettingsLoadState {
        initial,
        load_error,
    } = state;
    let initial_value = initial.clone().unwrap_or_default();
    let settings_loaded = initial.is_some();
    let initial_section = if initial_value.spiral().project_id().trim().is_empty() {
        SettingsSection::Spiral
    } else {
        SettingsSection::Robots
    };
    let SettingsPanelState {
        draft:
            SettingsDraftSignals {
                mut current,
                mut base,
                mut saved,
                mut client_secret,
                mut section,
            },
        operations:
            SettingsOperationSignals {
                mut saving,
                mut testing,
                transferring_project,
                mut test_result,
                mut save_error,
                project_transfer:
                    ProjectTransferSignals {
                        pending_file: mut pending_project_file,
                        error: mut project_transfer_error,
                    },
            },
    } = SettingsPanelState::use_state(initial_value, initial_section);
    let studio = studio();
    let data = studio.data_controller();
    let on_saved = move || {
        studio.refresh_settings();
        studio.refresh_relation_catalog();
        data.invalidate_episode_catalog();
    };

    use_effect(|| BrowserFocus::after_render("data-settings-panel".to_owned()));

    use_effect(use_reactive((&initial,), move |(initial,)| {
        let acknowledged = saved();
        let busy = saving() || transferring_project();
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

    let mut save = move |connection_test: Option<SettingsSection>| {
        if saving() || transferring_project() || !settings_loaded {
            return;
        }
        save_error.set(None);
        saving.set(true);
        let submitted = current();
        let destination_changed =
            submitted.spiral() != base.peek().spiral() || !client_secret.peek().is_empty();
        let connection = SpiralConnectionUpdate::new(
            current().spiral().endpoint(),
            current().spiral().spfs_endpoint(),
            current().spiral().project_id(),
        )
        .with_client_credentials(current().spiral().client_id(), client_secret())
        .with_clear_client_secret(
            current().spiral().client_id().trim().is_empty()
                && current().spiral().client_secret_configured(),
        );
        let training = current().training().to_owned();
        let video = current().video().to_owned();
        crate::client::project::ProjectClient::current().spawn(async move {
            match SettingsClient::current()
                .update_data_settings(spiral_robotics_common::settings::SettingsUpdate::new(
                    connection, training, video,
                ))
                .await
            {
                Ok(_) => {
                    saved.set(Some(submitted));
                    client_secret.set(String::new());
                    on_saved();
                    if let Some(connection_test) = connection_test.filter(|_| !destination_changed)
                    {
                        testing.set(true);
                        test_result.set(None);
                        let result = match connection_test {
                            SettingsSection::Spiral => {
                                SettingsClient::current().test_spiral_connection().await
                            }
                            SettingsSection::Training => {
                                SettingsClient::current().test_training_runtime().await
                            }
                            SettingsSection::Robots | SettingsSection::Video => unreachable!(
                                "only connection-backed settings sections can request a test"
                            ),
                        };
                        match result {
                            Ok(response) => test_result.set(Some(response)),
                            Err(error) => test_result.set(Some(
                                spiral_robotics_common::settings::ConnectionTest::new(
                                    false,
                                    format!("{error} {}", error.recovery_guidance()),
                                ),
                            )),
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
        });
    };
    let configured_destination = current();
    let project_transfer_ready = initial.as_ref().is_some_and(|active| {
        !configured_destination
            .spiral()
            .project_id()
            .trim()
            .is_empty()
            && active.spiral().endpoint() == configured_destination.spiral().endpoint()
            && active.spiral().spfs_endpoint() == configured_destination.spiral().spfs_endpoint()
            && active.spiral().project_id() == configured_destination.spiral().project_id()
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
                                        value: current().spiral().endpoint(),
                                        behavior: SettingsInputBehavior::new(
                                            "https://api.spiraldb.dev",
                                            move |value| current.write().spiral_mut().set_endpoint(value),
                                        ),
                                    }
                                    SettingsField {
                                        label: "SpFS endpoint",
                                        value: current().spiral().spfs_endpoint(),
                                        behavior: SettingsInputBehavior::new(
                                            "https://spfs.spiraldb.dev",
                                            move |value| current.write().spiral_mut().set_spfs_endpoint(value),
                                        ),
                                    }
                                }
                                div { class: "settings-field-row",
                                    SettingsField {
                                        label: "Project ID",
                                        value: current().spiral().project_id(),
                                        behavior: SettingsInputBehavior::new(
                                            "your-project-id",
                                            move |value| current.write().spiral_mut().set_project_id(value),
                                        ),
                                    }
                                    SettingsField { label: "Namespace", value: current().spiral().namespace(), behavior: SettingsInputBehavior::new("spiral_robotics", move |_| {}).with_disabled() }
                                }
                                div { class: "settings-field-row",
                                    SettingsField {
                                        label: "Client ID",
                                        value: current().spiral().client_id(),
                                        behavior: SettingsInputBehavior::new(
                                            "your-client-id",
                                            move |value| current.write().spiral_mut().set_client_id(value),
                                        ),
                                    }
                                    SettingsField {
                                        label: "Client secret",
                                        value: client_secret(),
                                        behavior: SettingsInputBehavior::new(
                                            if current().spiral().client_secret_configured() { "Saved — leave blank to keep" } else { "your-client-secret" },
                                            move |value| client_secret.set(value),
                                        )
                                        .with_secret(),
                                    }
                                }
                                p { class: "settings-help",
                                    "The API, SpFS endpoint, project, and optional client credentials are applied to a new scoped SpiralDB connection when you save. The secret is stored only in the backend's private bootstrap file and is never returned to this page."
                                }
                                Action {
                                    purpose: ActionPurpose::Apply,
                                    variant: ActionVariant::Outline,
                                    disabled: saving() || testing() || transferring_project() || current().spiral().project_id().trim().is_empty(),
                                    aria_busy: testing(),
                                    onclick: move |_| save(Some(SettingsSection::Spiral)),
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
                                            purpose: ActionPurpose::Import,
                                            variant: ActionVariant::Outline,
                                            title: if project_transfer_ready { "Replace all Studio-managed relations in the active SpiralDB project" } else { "Save the Spiral destination before transferring a project" },
                                            disabled: saving() || transferring_project() || !project_transfer_ready,
                                            aria_busy: transferring_project(),
                                            input: FileInput::new(
                                                ".spiral-project,application/vnd.spiral-robotics.project",
                                                move |event: Event<FormData>| {
                                                    let Some(file) = event.files().first().cloned() else { return };
                                                    project_transfer_error.set(None);
                                                    pending_project_file.set(Some(file));
                                                },
                                            ).with_id("project-import-input"),
                                            "Import project"
                                        }
                                        if transferring_project() || !project_transfer_ready {
                                            Action { purpose: ActionPurpose::Export, variant: ActionVariant::Outline, title: if project_transfer_ready { "Project transfer is in progress" } else { "Save the Spiral destination before transferring a project" }, disabled: true, "Export project" }
                                        } else {
                                            ActionLink {
                                                purpose: ActionPurpose::Export,
                                                variant: ActionVariant::Outline,
                                                href: crate::client::project::ProjectClient::current().scoped_url("/api/project/export"),
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
                                    value: current().training().endpoint(),
                                    behavior: SettingsInputBehavior::new(
                                        "http://127.0.0.1:50051",
                                        move |value| current.write().training_mut().set_endpoint(value),
                                    ),
                                }
                                p { class: "settings-help",
                                    "Studio uses Arrow IPC over HTTP. The selected endpoint applies to new training and rollout sessions; an active session keeps its current connection."
                                }
                                Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline, disabled: saving() || testing() || transferring_project(), aria_busy: testing(), onclick: move |_| save(Some(SettingsSection::Training)), if testing() { "Connecting…" } else { "Save and connect" } }
                            },
                            SettingsSection::Video => rsx! {
                                ProviderHeading { title: "Video", description: "Include assigned cameras in new episodes." }
                                Toggle { label: "Record video", checked: current().video().enabled(), onchange: move |value| current.write().video_mut().set_enabled(value) }
                            },
                        }
                        if let Some(response) = test_result() {
                            p {
                                class: if response.connected() { "connection-test connection-test-ok" } else { "connection-test connection-test-failed" },
                                role: "status",
                                "{response.message()}"
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
                        span { "{error.recovery_guidance()}" }
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
                } else {
                    span {
                        "Applies to this Studio."
                    }
                    div {
                        Action { purpose: ActionPurpose::Cancel, disabled: saving() || transferring_project(), onclick: move |_| { current.set(base()); client_secret.set(String::new()); save_error.set(None); test_result.set(None); }, "Discard changes" }
                        Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Solid, disabled: saving() || transferring_project() || current().spiral().project_id().trim().is_empty(), aria_busy: saving(), onclick: move |_| save(None), if saving() { "Saving…" } else { "Save settings" } }
                    }
                }
            }
        }
        if let Some(file) = pending_project_file() {
            BlockingConfirmation {
                id: "project-import-confirmation",
                title: format!("Replace project {}?", current().spiral().project_id()),
                consequence: "Every Studio-managed relation in this project will be replaced, including robots, episodes, media, settings, training history, checkpoints, and rollouts. Connected robots will disconnect first; imported robots will then connect automatically.".to_owned(),
                copy: ConfirmationCopy::new(
                    format!("Import {} into the active SpiralDB project.", file.name()),
                    if transferring_project() { "Restoring…" } else { "Import and replace project" },
                    "Keep current project",
                ),
                response: ConfirmationResponse::new(ActionPurpose::Replace, move |_| {
                    if transferring_project() {
                        return;
                    }
                    let file = file.clone();
                    project_transfer_error.set(None);
                    crate::client::project::ProjectClient::current().spawn(async move {
                        let Some(outcome) = SingleFlight::run(transferring_project, async {
                                let result = SettingsClient::current().import_project(file)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                data.set_selected_episode(None);
                                data.invalidate_episode_catalog();
                                Ok::<_, String>(result)
                            })
                            .await
                        else {
                            return;
                        };
                        match outcome {
                            Ok(result) => {
                                on_saved();
                                pending_project_file.set(None);
                                project_transfer_error.set(None);
                                studio.report_success_from(
                                    MessageSource::Settings,
                                    format!(
                                        "Project restored from {} relations ({} rows).{}",
                                        result.relations(),
                                        result.rows(),
                                        result.warnings().import_warning_suffix(),
                                    ),
                                );
                            }
                            Err(error) => {
                                pending_project_file.set(None);
                                project_transfer_error.set(Some(error.clone()));
                                studio.report_from(MessageSource::Settings, error);
                                BrowserFocus::after_render("project-import-input".to_owned());
                            }
                        }
                    });
                }, move |_| pending_project_file.set(None)),
                dismissal: ConfirmationDismissal::new("project-import-input")
                    .with_pending(transferring_project()),
            }
        }
    }
}

trait ImportWarningsPresentation {
    fn import_warning_suffix(&self) -> String;
}

impl ImportWarningsPresentation for [String] {
    fn import_warning_suffix(&self) -> String {
        if self.is_empty() {
            String::new()
        } else {
            format!(" Warning: {}", self.join(" "))
        }
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

#[derive(Clone, PartialEq)]
struct SettingsInputBehavior {
    placeholder: String,
    disabled: bool,
    secret: bool,
    oninput: EventHandler<String>,
}

impl SettingsInputBehavior {
    fn new(placeholder: impl Into<String>, oninput: impl FnMut(String) + 'static) -> Self {
        Self {
            placeholder: placeholder.into(),
            disabled: false,
            secret: false,
            oninput: EventHandler::new(oninput),
        }
    }

    const fn with_disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    const fn with_secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

#[component]
fn SettingsField(label: &'static str, value: String, behavior: SettingsInputBehavior) -> Element {
    let SettingsInputBehavior {
        placeholder,
        disabled,
        secret,
        oninput,
    } = behavior;
    rsx! {
        label { class: "settings-field",
            span { class: "label", "{label}" }
            input { class: "field", r#type: if secret { "password" } else { "text" }, value, placeholder, disabled, oninput: move |event| oninput.call(event.value()) }
        }
    }
}

#[component]
pub(crate) fn SettingsWorkspace() -> Element {
    let studio =
        crate::shell::state::Studio::use_studio(crate::shell::activity::Activity::Settings);
    studio.use_hardware_polling();
    rsx! {
        crate::shell::frame::WorkspaceFrame {
            SettingsPanel { state: SettingsLoadState::new(
                studio.loaded_settings(),
                studio.settings_result().and_then(Result::err),
            ) }
        }
    }
}
