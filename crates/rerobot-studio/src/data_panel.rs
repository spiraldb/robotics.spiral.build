//! Spiral dataset explorer and backend connection settings.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    ConnectionTestResponse, DataSettings, PublicationStatus, SpiralQueryRequest,
    SpiralQueryResponse, StartSpiralReplayRequest, UpdateDataSettingsRequest,
};

use crate::api;
use crate::state::studio;

#[component]
pub(crate) fn DataWorkspace(
    on_close: EventHandler<()>,
    on_settings: EventHandler<()>,
    settings_revision: Signal<u64>,
) -> Element {
    let studio = studio();
    let settings = use_resource(move || {
        let _revision = settings_revision();
        async { api::get::<DataSettings>("/api/settings/data").await }
    });
    let mut publications =
        use_resource(|| async { api::get::<Vec<PublicationStatus>>("/api/publications").await });
    let mut sql = use_signal(|| {
        "SELECT * FROM rerobot.episodes ORDER BY started_ms DESC LIMIT 100".to_owned()
    });
    let mut result = use_signal(|| None::<SpiralQueryResponse>);
    let mut query_error = use_signal(|| None::<String>);
    let mut running = use_signal(|| false);
    let mut replaying = use_signal(|| false);
    let snapshot = studio.snapshot.read().clone();
    let default_robot = snapshot
        .robots
        .iter()
        .find(|robot| {
            robot
                .robot_type
                .as_deref()
                .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"))
        })
        .or_else(|| snapshot.robots.first())
        .map(|robot| robot.id.clone())
        .unwrap_or_default();
    let default_episode = snapshot
        .episodes
        .iter()
        .rev()
        .find(|episode| episode.usable())
        .map(|episode| episode.id.clone())
        .unwrap_or_default();
    let mut replay_robot = use_signal(|| default_robot);
    let mut replay_episode = use_signal(|| default_episode.clone());
    let mut replay_stream = use_signal(String::new);
    let mut video_episode = use_signal(|| default_episode.clone());
    let mut video_stream = use_signal(String::new);
    let mut video_url = use_signal(|| None::<String>);
    let mut video_error = use_signal(|| false);
    let mut publish_episode = use_signal(|| default_episode);
    let mut publishing_episode = use_signal(|| false);

    let dataset = settings
        .read_unchecked()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|settings| settings.spiral.dataset.clone())
        .filter(|dataset| !dataset.is_empty())
        .unwrap_or_else(|| "rerobot".to_owned());
    let spiral_is_memory = settings
        .read_unchecked()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|settings| settings.spiral.uri == "spiral:memory:");
    let video_index_query = if spiral_is_memory {
        format!(
            "SELECT v.episode_id, v.stream_id, tracks.* FROM {dataset}.video_sources v \
             CROSS JOIN LATERAL spql.mp4.tracks(v.source) AS tracks LIMIT 20"
        )
    } else {
        format!(
            "SELECT episode_id, stream_id, media, video_index FROM {dataset}.video.assets LIMIT 20"
        )
    };

    let mut run_query = move || {
        if running() || sql.read().trim().is_empty() {
            return;
        }
        running.set(true);
        query_error.set(None);
        result.set(None);
        let request = SpiralQueryRequest {
            sql: sql(),
            max_rows: 100,
        };
        spawn(async move {
            match api::send_json(Method::POST, "/api/spiral/query", Some(&request)).await {
                Ok(response) => result.set(Some(response)),
                Err(error) => query_error.set(Some(error)),
            }
            running.set(false);
        });
    };

    rsx! {
        section {
            class: "data-workspace",
            "aria-label": "Dataset workspace",
            tabindex: "-1",
            autofocus: true,
            header { class: "data-workspace-header",
                div {
                    h2 { "Dataset" }
                    p { "Query SpiralDB, inspect publications, and replay a stored trajectory." }
                }
                div { class: "data-header-actions",
                    button {
                        r#type: "button", class: "btn btn-outline",
                        onclick: move |_| on_settings.call(()),
                        SettingsIcon {}
                        "Connections"
                    }
                    button {
                        r#type: "button", class: "icon-button",
                        "aria-label": "Close Dataset workspace", title: "Close",
                        onclick: move |_| on_close.call(()),
                        CloseIcon {}
                    }
                }
            }
            div { class: "data-workspace-body",
                aside { class: "data-sidebar",
                    h3 { "Destinations" }
                    match &*settings.read_unchecked() {
                        Some(Ok(current)) => rsx! {
                            DestinationState {
                                name: "SpiralDB",
                                enabled: current.spiral.enabled,
                                detail: if current.spiral.enabled {
                                    if current.spiral.project_id.is_empty() {
                                        format!("{} · {}", current.spiral.uri, current.spiral.dataset)
                                    } else {
                                        format!("{} · {}", current.spiral.project_id, current.spiral.dataset)
                                    }
                                } else { "Not configured".to_owned() }
                            }
                            DestinationState {
                                name: "Hugging Face",
                                enabled: current.hugging_face.enabled,
                                detail: if current.hugging_face.enabled {
                                    current.hugging_face.repository.clone()
                                } else { "Not configured".to_owned() }
                            }
                        },
                        Some(Err(error)) => rsx! { p { class: "data-error", "{error}" } },
                        None => rsx! { p { class: "data-muted", "Loading connections…" } },
                    }
                    div { class: "data-sidebar-section",
                        div { class: "data-sidebar-heading",
                            h3 { "Recent publishes" }
                            button {
                                r#type: "button", class: "link",
                                onclick: move |_| publications.restart(),
                                "Refresh"
                            }
                        }
                        match &*publications.read_unchecked() {
                            Some(Ok(items)) if items.is_empty() => rsx! {
                                p { class: "data-muted", "New episodes will appear here after recording." }
                            },
                            Some(Ok(items)) => rsx! {
                                ul { class: "publication-list",
                                    for item in items.iter().rev().take(8) {
                                        li { key: "{item.episode_id}",
                                            div { class: "publication-summary",
                                                span { class: "publication-id", "{item.episode_id}" }
                                                span { class: publication_class(&item.state), "{item.state}" }
                                            }
                                            if item.state == "failed" {
                                                p { class: "publication-error", "{publication_detail(item)}" }
                                            }
                                        }
                                    }
                                }
                            },
                            Some(Err(error)) => rsx! { p { class: "data-error", "{error}" } },
                            None => rsx! { p { class: "data-muted", "Loading…" } },
                        }
                    }
                    div { class: "data-sidebar-section publish-existing",
                        h3 { "Publish local copy" }
                        p { class: "data-muted", "Retry a completed recovery copy after a provider failure." }
                        input { class: "field", value: "{publish_episode}", oninput: move |event| publish_episode.set(event.value()), placeholder: "episode_000001", "aria-label": "Local episode to publish" }
                        button {
                            r#type: "button", class: "btn btn-outline",
                            disabled: publishing_episode() || publish_episode.read().trim().is_empty(),
                            onclick: move |_| {
                                publishing_episode.set(true);
                                let path = api::publication_url(&publish_episode());
                                spawn(async move {
                                    match api::send::<()>(Method::POST, &path, None).await {
                                        Ok(()) => {
                                            studio.report_success("Episode queued for publication.");
                                            publications.restart();
                                        }
                                        Err(error) => studio.report(error),
                                    }
                                    publishing_episode.set(false);
                                });
                            },
                            if publishing_episode() { "Queuing…" } else { "Publish episode" }
                        }
                    }
                }
                main { class: "data-main",
                    section { class: "query-editor", "aria-labelledby": "spiral-query-heading",
                        div { class: "query-heading",
                            div {
                                h3 { id: "spiral-query-heading", "Spiral query" }
                                p { "Read-only preview · up to 100 rows" }
                            }
                            button {
                                r#type: "button", class: "btn btn-primary",
                                disabled: running() || sql.read().trim().is_empty(),
                                onclick: move |_| run_query(),
                                if running() { "Running…" } else { "Run query" }
                            }
                        }
                        div { class: "query-presets", "Examples"
                            button { r#type: "button", class: "link", onclick: { let dataset = dataset.clone(); move |_| sql.set(format!("SELECT * FROM {dataset}.episodes ORDER BY started_ms DESC LIMIT 100")) }, "Episodes" }
                            button { r#type: "button", class: "link", onclick: { let dataset = dataset.clone(); move |_| sql.set(format!("SELECT * FROM {dataset}.episode_streams LIMIT 100")) }, "Streams" }
                            button { r#type: "button", class: "link", onclick: { let dataset = dataset.clone(); move |_| sql.set(format!("SELECT episode_id, stream_id, frames, width, height, fps FROM {dataset}.video_assets LIMIT 100")) }, "Video assets" }
                            button { r#type: "button", class: "link", onclick: { let query = video_index_query.clone(); move |_| sql.set(query.clone()) }, if spiral_is_memory { "Video tracks" } else { "Video index" } }
                        }
                        textarea {
                            class: "query-input",
                            spellcheck: "false",
                            "aria-label": "Spiral SQL query",
                            value: "{sql}",
                            oninput: move |event| sql.set(event.value()),
                            onkeydown: move |event| {
                                if event.key() == Key::Enter && (event.modifiers().ctrl() || event.modifiers().meta()) {
                                    event.prevent_default();
                                    run_query();
                                }
                            }
                        }
                        if let Some(error) = query_error() {
                            p { class: "data-error", role: "alert", "{error}" }
                        }
                    }
                    QueryResult { result: result() }
                    section { class: "remote-video", "aria-labelledby": "remote-video-heading",
                        div { class: "query-heading",
                            div {
                                h3 { id: "remote-video-heading", "Preview Spiral video" }
                                p { if spiral_is_memory { "Stream the original MP4 through this backend; use Video tracks to inspect it with Spiral's MP4 plugin." } else { "Stream the original MP4 through this backend; use Video index to inspect Spiral's persisted frame index." } }
                            }
                            button {
                                r#type: "button", class: "btn btn-outline",
                                disabled: video_episode.read().trim().is_empty() || video_stream.read().trim().is_empty(),
                                onclick: move |_| {
                                    video_error.set(false);
                                    video_url.set(Some(api::spiral_video_url(&video_episode(), &video_stream())));
                                },
                                "Preview"
                            }
                        }
                        div { class: "video-preview-fields",
                            label { span { class: "label", "Episode" }
                                input { class: "field", value: "{video_episode}", oninput: move |event| { video_episode.set(event.value()); video_url.set(None); }, placeholder: "episode_000001" }
                            }
                            label { span { class: "label", "Video stream" }
                                input { class: "field", value: "{video_stream}", oninput: move |event| { video_stream.set(event.value()); video_url.set(None); }, placeholder: "follower.camera.wrist" }
                            }
                        }
                        if let Some(url) = video_url() {
                            video { class: "spiral-video-preview", controls: true, preload: "metadata", src: "{url}", onerror: move |_| video_error.set(true) }
                        }
                        if video_error() {
                            p { class: "data-error", role: "alert", "That MP4 could not be loaded. Check the episode and video stream, then try again." }
                        }
                    }
                    section { class: "remote-replay", "aria-labelledby": "remote-replay-heading",
                        div { class: "query-heading",
                            div {
                                h3 { id: "remote-replay-heading", "Replay from Spiral" }
                                p { "Fetch joint state on the backend, then drive a connected follower locally." }
                            }
                            button {
                                r#type: "button", class: "btn btn-primary",
                                disabled: replaying() || replay_robot.read().trim().is_empty() || replay_episode.read().trim().is_empty() || replay_stream.read().trim().is_empty(),
                                onclick: move |_| {
                                    replaying.set(true);
                                    let request = StartSpiralReplayRequest {
                                        episode: replay_episode(),
                                        robot: replay_robot(),
                                        stream: replay_stream(),
                                    };
                                    spawn(async move {
                                        let outcome = api::send(Method::POST, "/api/spiral/replay", Some(&request)).await;
                                        replaying.set(false);
                                        match outcome {
                                            Ok(()) => studio.report_success("Replay started from SpiralDB."),
                                            Err(error) => studio.report(error),
                                        }
                                    });
                                },
                                if replaying() { "Loading…" } else { "Replay" }
                            }
                        }
                        div { class: "replay-fields",
                            label { span { class: "label", "Episode" }
                                input { class: "field", value: "{replay_episode}", oninput: move |event| replay_episode.set(event.value()), placeholder: "episode_000001" }
                            }
                            label { span { class: "label", "Spiral stream" }
                                input { class: "field", value: "{replay_stream}", oninput: move |event| replay_stream.set(event.value()), placeholder: "follower" }
                            }
                            label { span { class: "label", "Local robot" }
                                select { class: "field", onchange: move |event| replay_robot.set(event.value()),
                                    option { value: "", selected: replay_robot.read().is_empty(), "Choose robot…" }
                                    for robot in &snapshot.robots {
                                        option { value: "{robot.id}", selected: replay_robot() == robot.id, "{robot.id}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DestinationState(name: &'static str, enabled: bool, detail: String) -> Element {
    rsx! {
        div { class: "destination-row",
            span { class: if enabled { "destination-dot destination-dot-live" } else { "destination-dot" } }
            div { strong { "{name}" } p { "{detail}" } }
        }
    }
}

#[component]
fn QueryResult(result: Option<SpiralQueryResponse>) -> Element {
    let Some(result) = result else {
        return rsx! {
            section { class: "query-empty",
                h3 { "No result yet" }
                p { "Run a SELECT query to preview episode, joint, or video rows." }
            }
        };
    };
    rsx! {
        section { class: "query-result", "aria-label": "Spiral query result",
            div { class: "query-result-meta",
                span { "{result.rows.len()} rows" }
                span { "{result.elapsed_ms} ms" }
                if result.truncated { span { class: "chip chip-caution", "Truncated" } }
            }
            if result.rows.is_empty() {
                p { class: "query-empty-inline", "The query completed and returned no rows." }
            } else {
                div { class: "query-table-wrap",
                    table { class: "query-table",
                        thead { tr {
                            for column in &result.columns {
                                th { key: "{column.name}", title: "{column.data_type}", "{column.name}" }
                            }
                        } }
                        tbody {
                            for (index, row) in result.rows.iter().enumerate() {
                                tr { key: "{index}",
                                    for column in &result.columns {
                                        td { key: "{column.name}", title: cell_value(row.get(&column.name)),
                                            "{cell_value(row.get(&column.name))}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    Spiral,
    HuggingFace,
    Video,
}

#[component]
pub(crate) fn DataSettingsPanel(on_close: EventHandler<()>, on_saved: EventHandler<()>) -> Element {
    let mut current = use_signal(DataSettings::default);
    let mut loaded = use_signal(|| false);
    let mut load_error = use_signal(|| None::<String>);
    let mut section = use_signal(|| SettingsSection::Spiral);
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

    let mut save = move |after: Option<&'static str>| {
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
                    if let Some(path) = after {
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
            role: "dialog", "aria-modal": "true", "aria-labelledby": "data-settings-title",
            tabindex: "-1",
            autofocus: true,
            onclick: move |event| event.stop_propagation(),
            header { class: "settings-header",
                div { h2 { id: "data-settings-title", "Settings" } p { "Data destinations are owned by this backend." } }
                button { r#type: "button", class: "icon-button", "aria-label": "Close settings", autofocus: true, onclick: move |_| on_close.call(()), CloseIcon {} }
            }
            if !loaded() {
                div { class: "settings-loading", "Loading settings…" }
            } else if let Some(error) = load_error() {
                div { class: "settings-loading data-error", "{error}" }
            } else {
                div { class: "settings-layout",
                    nav { class: "settings-nav", "aria-label": "Data settings sections",
                        SettingsNavButton { label: "SpiralDB", active: section() == SettingsSection::Spiral, onclick: move |_| { section.set(SettingsSection::Spiral); test_result.set(None); } }
                        SettingsNavButton { label: "Hugging Face", active: section() == SettingsSection::HuggingFace, onclick: move |_| { section.set(SettingsSection::HuggingFace); test_result.set(None); } }
                        SettingsNavButton { label: "Video", active: section() == SettingsSection::Video, onclick: move |_| { section.set(SettingsSection::Video); test_result.set(None); } }
                    }
                    div { class: "settings-content",
                        match section() {
                            SettingsSection::Spiral => rsx! {
                                ProviderHeading { title: "SpiralDB", description: "Primary relational episode store, connected through the standard ADBC interface." }
                                ToggleField { label: "Publish new episodes to SpiralDB", checked: current().spiral.enabled, onchange: move |value| current.write().spiral.enabled = value }
                                SettingsField { label: "Connection URI", value: current().spiral.uri, placeholder: "spiraldb://", oninput: move |value| current.write().spiral.uri = value }
                                div { class: "settings-field-row",
                                    SettingsField { label: "Project", value: current().spiral.project_id, placeholder: "Required hosted; unused in memory", disabled: current().spiral.uri.trim() == "spiral:memory:", oninput: move |value| current.write().spiral.project_id = value }
                                    SettingsField { label: "Dataset", value: current().spiral.dataset, placeholder: "rerobot", oninput: move |value| current.write().spiral.dataset = value }
                                }
                                p { class: "settings-help", "Use spiral:memory: for a local session. Hosted connections reuse credentials managed by the Spiral CLI/ADBC driver. The backend operator supplies the trusted driver with --spiral-adbc-driver or SPIRALDB_ADBC_DRIVER." }
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
                                p { class: "settings-help", "The backend operator chooses FFmpeg and its codec at server startup. MP4 payloads are ingested through ADBC. Hosted projects maintain a reusable managed video index; memory sessions expose the same MP4 and video functions ad hoc." }
                            },
                        }
                        if let Some(response) = test_result() {
                            p { class: if response.ok { "connection-test connection-test-ok" } else { "connection-test connection-test-failed" }, role: "status", "{response.message}" }
                        }
                    }
                }
                footer { class: "settings-footer",
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
    rsx! { button { r#type: "button", class: if active { "settings-nav-active" } else { "" }, "aria-current": active.then_some("page"), onclick: move |_| onclick.call(()), "{label}" } }
}

#[component]
fn ProviderHeading(title: &'static str, description: &'static str) -> Element {
    rsx! { div { class: "provider-heading", h3 { "{title}" } p { "{description}" } } }
}

#[component]
fn ToggleField(label: &'static str, checked: bool, onchange: EventHandler<bool>) -> Element {
    rsx! { label { class: "toggle-field", input { r#type: "checkbox", checked, onchange: move |event| onchange.call(event.checked()) } span { "{label}" } } }
}

#[component]
fn SettingsField(
    label: &'static str,
    value: String,
    placeholder: &'static str,
    #[props(default)] disabled: bool,
    oninput: EventHandler<String>,
) -> Element {
    rsx! { label { class: "settings-field", span { class: "label", "{label}" } input { class: "field", value, placeholder, disabled, oninput: move |event| oninput.call(event.value()) } } }
}

#[component]
fn SecretField(
    label: &'static str,
    has_secret: bool,
    value: String,
    oninput: EventHandler<String>,
    on_clear: EventHandler<()>,
) -> Element {
    rsx! { label { class: "settings-field", span { class: "label", "{label}" }
        div { class: "secret-field",
            input { class: "field", r#type: "password", autocomplete: "new-password", value, placeholder: if has_secret { "Stored — enter to replace" } else { "Paste token" }, oninput: move |event| oninput.call(event.value()) }
            if has_secret { button { r#type: "button", class: "btn", onclick: move |_| on_clear.call(()), "Clear" } }
        }
    } }
}

#[component]
pub(crate) fn DataIcon() -> Element {
    rsx! { svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
        ellipse { cx: "8", cy: "3.5", rx: "5", ry: "2" }
        path { d: "M3 3.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4M3 7.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4" }
    } }
}

#[component]
fn SettingsIcon() -> Element {
    rsx! { svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
        circle { cx: "8", cy: "8", r: "2.25" }
        path { d: "M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.58 3.58l1.06 1.06M11.36 11.36l1.06 1.06M12.42 3.58l-1.06 1.06M4.64 11.36l-1.06 1.06" }
    } }
}

#[component]
fn CloseIcon() -> Element {
    rsx! { svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true", path { d: "M4 4l8 8M12 4l-8 8" } } }
}

fn cell_value(value: Option<&serde_json::Value>) -> String {
    let value = match value {
        None | Some(serde_json::Value::Null) => "null".to_owned(),
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
    };
    const LIMIT: usize = 120;
    if value.chars().count() > LIMIT {
        format!("{}…", value.chars().take(LIMIT).collect::<String>())
    } else {
        value
    }
}

fn publication_class(state: &str) -> &'static str {
    match state {
        "published" => "publication-state publication-state-ok",
        "failed" => "publication-state publication-state-failed",
        "local" => "publication-state",
        _ => "publication-state",
    }
}

fn publication_detail(status: &PublicationStatus) -> String {
    status
        .spiral
        .iter()
        .chain(status.hugging_face.iter())
        .filter(|detail| detail.starts_with("failed:"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}
