//! Settings screen for robots, storage, publication, and video.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    ConnectionTestResponse, DataSettings, UpdateDataSettingsRequest,
};

use crate::frontend::api::http as api;
use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::studio;
use crate::frontend::screens::settings::robots::RobotPanel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Robots,
    Spiral,
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
    let mut current = use_signal(move || initial_value);
    let mut section = use_signal(|| SettingsSection::Robots);
    let mut saving = use_signal(|| false);
    let mut testing = use_signal(|| false);
    let mut test_result = use_signal(|| None::<ConnectionTestResponse>);
    let mut initial_focus_set = use_signal(|| false);
    let studio = studio();

    use_effect(move || {
        if !initial_focus_set() {
            focus_settings_close();
            initial_focus_set.set(true);
        }
    });

    use_effect(use_reactive((&initial,), move |(initial,)| {
        if let Some(settings) = initial
            && !saving.peek().to_owned()
        {
            current.set(settings);
        }
    }));

    let mut save = move |test_path: Option<&'static str>| {
        if saving() || !settings_loaded {
            return;
        }
        saving.set(true);
        on_busy_change.call(true);
        let request = UpdateDataSettingsRequest {
            spiral: current().spiral,
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
                        studio.report_success_from(MessageSource::Settings, "Settings saved.");
                    }
                }
                Err(error) => studio.report_from(MessageSource::Settings, error),
            }
            saving.set(false);
            on_busy_change.call(false);
        });
    };

    rsx! {
        div { class: "settings-backdrop", onclick: move |_| if !saving() { on_close.call(()) } }
        section {
            id: "data-settings-dialog",
            class: "data-settings-dialog",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "data-settings-title",
            tabindex: "-1",
            onclick: move |event| event.stop_propagation(),
            onkeydown: move |event| {
                if event.key() == Key::Tab && trap_settings_focus(event.modifiers().shift()) {
                    event.prevent_default();
                }
            },
            header { class: "settings-header",
                div {
                    h2 { id: "data-settings-title", "Settings" }
                    p { "Robots, storage, and video." }
                }
                button {
                    id: "settings-close-button",
                    r#type: "button",
                    class: "icon-button",
                    "aria-label": "Close settings",
                    disabled: saving(),
                    onclick: move |_| if !saving() { on_close.call(()) },
                    CloseIcon {}
                }
            }
            div { class: "settings-layout",
                nav { class: "settings-nav", "aria-label": "Settings sections",
                    SettingsNavButton { label: "Robots", active: section() == SettingsSection::Robots, onclick: move |_| { section.set(SettingsSection::Robots); test_result.set(None); } }
                    SettingsNavButton { label: "SpiralDB", active: section() == SettingsSection::Spiral, onclick: move |_| { section.set(SettingsSection::Spiral); test_result.set(None); } }
                    SettingsNavButton { label: "Video", active: section() == SettingsSection::Video, onclick: move |_| { section.set(SettingsSection::Video); test_result.set(None); } }
                }
                div { class: "settings-content",
                    if section() == SettingsSection::Robots {
                        ProviderHeading { title: "Robot setup", description: "Configure ports, camera assignments, and calibration for each arm." }
                        div { class: "robot-settings-panel", RobotPanel { heading: false } }
                    } else if let Some(error) = load_error.as_ref() {
                        div { class: "settings-loading data-error", "{error}" }
                    } else if !settings_loaded {
                        div { class: "settings-loading", "Loading data settings…" }
                    } else {
                        match section() {
                            SettingsSection::Robots => rsx! {},
                            SettingsSection::Spiral => rsx! {
                                ProviderHeading { title: "SpiralDB", description: "Store and query episodes, video, and checkpoints." }
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
                                    SettingsField { label: "Namespace", value: current().spiral.dataset, placeholder: "spiral_robotics", oninput: move |value| current.write().spiral.dataset = value }
                                }
                                p { class: "settings-help",
                                    if current().spiral.uri.trim() == "spiral:memory:" {
                                        "Temporary data is cleared when Studio stops."
                                    } else {
                                        "Choose the project and namespace this Studio should use."
                                    }
                                }
                                button { r#type: "button", class: "btn btn-outline", disabled: saving() || testing(), onclick: move |_| save(Some("/api/settings/data/test/spiral")), if testing() { "Testing…" } else { "Save and test connection" } }
                            },
                            SettingsSection::Video => rsx! {
                                ProviderHeading { title: "Video", description: "Include assigned cameras in new episodes." }
                                ToggleField { label: "Record video", checked: current().video.enabled, onchange: move |value| current.write().video.enabled = value }
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
                    span { "Applies to this Studio." }
                    div {
                        button { r#type: "button", class: "btn", onclick: move |_| on_close.call(()), "Cancel" }
                        button { r#type: "button", class: "btn btn-primary", disabled: saving(), onclick: move |_| save(None), if saving() { "Saving…" } else { "Save settings" } }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "web")]
fn focus_settings_close() {
    use wasm_bindgen::JsCast as _;

    let Some(button) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("settings-close-button"))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = button.focus();
}

#[cfg(not(feature = "web"))]
fn focus_settings_close() {}

#[cfg(feature = "web")]
fn trap_settings_focus(backwards: bool) -> bool {
    use wasm_bindgen::JsCast as _;

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    let Some(dialog) = document.get_element_by_id("data-settings-dialog") else {
        return false;
    };
    let Ok(nodes) = dialog.query_selector_all(
        "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) else {
        return false;
    };
    let focusable = (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect::<Vec<_>>();
    let Some(active) = document.active_element() else {
        return focusable
            .first()
            .is_some_and(|element| element.focus().is_ok());
    };
    let current = focusable
        .iter()
        .position(|element| active.is_same_node(Some(element.as_ref())));
    let next = if backwards {
        current
            .and_then(|index| index.checked_sub(1))
            .unwrap_or_else(|| focusable.len().saturating_sub(1))
    } else {
        current.map_or(0, |index| (index + 1) % focusable.len().max(1))
    };
    focusable
        .get(next)
        .is_some_and(|element| element.focus().is_ok())
}

#[cfg(not(feature = "web"))]
fn trap_settings_focus(_backwards: bool) -> bool {
    false
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
fn CloseIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4 4l8 8M12 4l-8 8" }
        }
    }
}
