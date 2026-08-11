//! Backend-owned robot, storage, publication, and video settings.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{ConnectionTestResponse, DataSettings, UpdateDataSettingsRequest};

use crate::api;
use crate::setup::RobotPanel;
use crate::state::studio;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Robots,
    Spiral,
    HuggingFace,
    Video,
}

#[component]
pub(crate) fn SettingsPanel(on_close: EventHandler<()>, on_saved: EventHandler<()>) -> Element {
    let mut current = use_signal(DataSettings::default);
    let mut loaded = use_signal(|| false);
    let mut load_error = use_signal(|| None::<String>);
    let mut section = use_signal(|| SettingsSection::Robots);
    let mut hugging_face_token = use_signal(String::new);
    let mut clear_hugging_face = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut testing = use_signal(|| false);
    let mut test_result = use_signal(|| None::<ConnectionTestResponse>);
    let studio = studio();

    use_future(move || async move {
        match api::get::<DataSettings>("/api/settings/data").await {
            Ok(settings) => current.set(settings),
            Err(error) => load_error.set(Some(error)),
        }
        loaded.set(true);
    });

    let mut save = move |test_path: Option<&'static str>| {
        if saving() || !loaded() {
            return;
        }
        saving.set(true);
        let request = UpdateDataSettingsRequest {
            spiral: current().spiral,
            hugging_face: current().hugging_face,
            video: current().video,
            hugging_face_token: (!hugging_face_token.read().trim().is_empty())
                .then_some(hugging_face_token()),
            clear_hugging_face_token: clear_hugging_face(),
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
                    on_saved.call(());
                    hugging_face_token.set(String::new());
                    clear_hugging_face.set(false);
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
                        studio.report_success("Data settings saved on the backend.");
                    }
                }
                Err(error) => studio.report(error),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "settings-backdrop", onclick: move |_| on_close.call(()) }
        section {
            class: "data-settings-dialog",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "data-settings-title",
            tabindex: "-1",
            autofocus: true,
            onclick: move |event| event.stop_propagation(),
            header { class: "settings-header",
                div {
                    h2 { id: "data-settings-title", "Settings" }
                    p { "Robot hardware and backend-owned data destinations." }
                }
                button {
                    r#type: "button",
                    class: "icon-button",
                    "aria-label": "Close settings",
                    autofocus: true,
                    onclick: move |_| on_close.call(()),
                    CloseIcon {}
                }
            }
            div { class: "settings-layout",
                nav { class: "settings-nav", "aria-label": "Settings sections",
                    SettingsNavButton { label: "Robots", active: section() == SettingsSection::Robots, onclick: move |_| { section.set(SettingsSection::Robots); test_result.set(None); } }
                    SettingsNavButton { label: "SpiralDB", active: section() == SettingsSection::Spiral, onclick: move |_| { section.set(SettingsSection::Spiral); test_result.set(None); } }
                    SettingsNavButton { label: "Hugging Face", active: section() == SettingsSection::HuggingFace, onclick: move |_| { section.set(SettingsSection::HuggingFace); test_result.set(None); } }
                    SettingsNavButton { label: "Video", active: section() == SettingsSection::Video, onclick: move |_| { section.set(SettingsSection::Video); test_result.set(None); } }
                }
                div { class: "settings-content",
                    if section() == SettingsSection::Robots {
                        ProviderHeading { title: "Robot setup", description: "Configure ports, camera assignments, and calibration for each arm." }
                        div { class: "robot-settings-panel", RobotPanel { heading: false } }
                    } else if !loaded() {
                        div { class: "settings-loading", "Loading data settings…" }
                    } else if let Some(error) = load_error() {
                        div { class: "settings-loading data-error", "{error}" }
                    } else {
                        match section() {
                            SettingsSection::Robots => rsx! {},
                            SettingsSection::Spiral => rsx! {
                                ProviderHeading { title: "SpiralDB", description: "Primary relational episode store, connected through the standard ADBC interface." }
                                label { class: "settings-field",
                                    span { "Catalog" }
                                    select {
                                        class: "field",
                                        value: "{current().spiral.uri}",
                                        onchange: move |event| current.write().spiral.uri = event.value(),
                                        option { value: "spiraldb://", "SpiralDB service" }
                                        option { value: "spiral:memory:", "Temporary in-memory session" }
                                    }
                                }
                                div { class: "settings-field-row",
                                    SettingsField { label: "Project", value: current().spiral.project_id, placeholder: "Required for a SpiralDB service", disabled: current().spiral.uri.trim() == "spiral:memory:", oninput: move |value| current.write().spiral.project_id = value }
                                    SettingsField { label: "Dataset", value: current().spiral.dataset, placeholder: "rerobot", oninput: move |value| current.write().spiral.dataset = value }
                                }
                                p { class: "settings-help", "For a local SpiralDB stack, keep the SpiralDB service catalog selected and start this backend with SPIRAL__SERVER__URL and SPIRAL__SPFS__URL pointing at that stack. Reserve the dataset name for rerobot so it can create the root resource and child tables. The in-memory option is disposable and does not connect to the local service. The backend also requires --spiral-adbc-driver or SPIRALDB_ADBC_DRIVER." }
                                button { r#type: "button", class: "btn btn-outline", disabled: saving() || testing(), onclick: move |_| save(Some("/api/settings/data/test/spiral")), if testing() { "Testing…" } else { "Save and test connection" } }
                            },
                            SettingsSection::HuggingFace => rsx! {
                                ProviderHeading { title: "Hugging Face", description: "Optional Arrow/Parquet dataset mirror, committed once per completed episode." }
                                ToggleField { label: "Mirror new episodes to Hugging Face", checked: current().hugging_face.enabled, onchange: move |value| current.write().hugging_face.enabled = value }
                                SettingsField { label: "Dataset repository", value: current().hugging_face.repository, placeholder: "owner/dataset", oninput: move |value| current.write().hugging_face.repository = value }
                                SettingsField { label: "Revision", value: current().hugging_face.revision, placeholder: "main", oninput: move |value| current.write().hugging_face.revision = value }
                                SecretField { label: "Write token", has_secret: current().hugging_face.has_token, value: hugging_face_token(), oninput: move |value| { hugging_face_token.set(value); clear_hugging_face.set(false); }, on_clear: move |_| { hugging_face_token.set(String::new()); clear_hugging_face.set(true); current.write().hugging_face.has_token = false; } }
                                button { r#type: "button", class: "btn btn-outline", disabled: saving() || testing(), onclick: move |_| save(Some("/api/settings/data/test/hugging-face")), if testing() { "Testing…" } else { "Save and test connection" } }
                            },
                            SettingsSection::Video => rsx! {
                                ProviderHeading { title: "Video", description: "Cameras are encoded on the backend, then indexed with Spiral's MP4 and video plugins." }
                                ToggleField { label: "Record assigned cameras with each episode", checked: current().video.enabled, onchange: move |value| current.write().video.enabled = value }
                                p { class: "settings-help", "The backend operator chooses FFmpeg and its codec at server startup. MP4 payloads are ingested through ADBC. Service projects maintain one immutable managed MP4 index per completed episode; memory sessions expose the same MP4 and video functions ad hoc." }
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
            footer { class: "settings-footer",
                if section() == SettingsSection::Robots {
                    span { "Robot changes are saved immediately." }
                    div { button { r#type: "button", class: "btn btn-primary", onclick: move |_| on_close.call(()), "Done" } }
                } else {
                    span { "Secrets stay on this backend." }
                    div {
                        button { r#type: "button", class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                        button { r#type: "button", class: "btn btn-primary", disabled: saving(), onclick: move |_| save(None), if saving() { "Saving…" } else { "Save settings" } }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingsNavButton(label: &'static str, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if active { "settings-nav-active" } else { "" },
            "aria-current": active.then_some("page"),
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
fn ToggleField(label: &'static str, checked: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        label { class: "toggle-field",
            input { r#type: "checkbox", checked, onchange: move |event| onchange.call(event.checked()) }
            span { "{label}" }
        }
    }
}

#[component]
fn SettingsField(
    label: &'static str,
    value: String,
    placeholder: &'static str,
    #[props(default)] disabled: bool,
    oninput: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "settings-field",
            span { class: "label", "{label}" }
            input { class: "field", value, placeholder, disabled, oninput: move |event| oninput.call(event.value()) }
        }
    }
}

#[component]
fn SecretField(
    label: &'static str,
    has_secret: bool,
    value: String,
    oninput: EventHandler<String>,
    on_clear: EventHandler<()>,
) -> Element {
    rsx! {
        label { class: "settings-field",
            span { class: "label", "{label}" }
            div { class: "secret-field",
                input { class: "field", r#type: "password", autocomplete: "new-password", value, placeholder: if has_secret { "Stored — enter to replace" } else { "Paste token" }, oninput: move |event| oninput.call(event.value()) }
                if has_secret {
                    button { r#type: "button", class: "btn", onclick: move |_| on_clear.call(()), "Clear" }
                }
            }
        }
    }
}

#[component]
fn CloseIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4 4l8 8M12 4l-8 8" }
        }
    }
}
