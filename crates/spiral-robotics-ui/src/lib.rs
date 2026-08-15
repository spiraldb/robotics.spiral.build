//! One typed interaction vocabulary for Spiral Robotics interfaces.
//!
//! Layout belongs to each product surface. Action meaning, interaction states,
//! confirmation behavior, focus movement, and target sizing belong here.

use dioxus::prelude::*;

mod icons;

pub use icons::{ChevronIcon, CloseIcon, DeleteIcon, RefreshIcon, UnassignIcon};

const UI_STYLES: Asset = asset!("/assets/ui.css");

/// Install the shared interaction styles. Dioxus deduplicates identical assets,
/// so reusable workspaces can safely install this beside an application shell.
#[component]
pub fn UiStyles() -> Element {
    rsx! { document::Stylesheet { href: UI_STYLES } }
}

/// The stable verb behind an action. Product code chooses a verb before it
/// chooses an icon or visual treatment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionPurpose {
    Navigate,
    Select,
    Toggle,
    Create,
    Import,
    Export,
    Assign,
    Connect,
    Disconnect,
    Remove,
    Close,
    Delete,
    Discard,
    Apply,
    Replace,
    Hold,
    Teleoperate,
    Release,
    Sync,
    Refresh,
    Retry,
    Start,
    Stop,
    Cancel,
}

impl ActionPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Select => "select",
            Self::Toggle => "toggle",
            Self::Create => "create",
            Self::Import => "import",
            Self::Export => "export",
            Self::Assign => "assign",
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
            Self::Remove => "remove",
            Self::Close => "close",
            Self::Delete => "delete",
            Self::Discard => "discard",
            Self::Apply => "apply",
            Self::Replace => "replace",
            Self::Hold => "hold",
            Self::Teleoperate => "teleoperate",
            Self::Release => "release",
            Self::Sync => "sync",
            Self::Refresh => "refresh",
            Self::Retry => "retry",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Cancel => "cancel",
        }
    }
}

/// Presentation is independent from meaning. A Delete action remains Delete
/// whether a dense ledger renders it as an icon or a settings footer spells it
/// out as text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionVariant {
    #[default]
    Quiet,
    Outline,
    Solid,
    Link,
    Icon,
    Row,
    Tab,
    Segment,
}

impl ActionVariant {
    fn class_name(self) -> &'static str {
        match self {
            Self::Quiet => "ui-action--quiet",
            Self::Outline => "ui-action--outline",
            Self::Solid => "ui-action--solid",
            Self::Link => "ui-action--link",
            Self::Icon => "ui-action--icon",
            Self::Row => "ui-action--row",
            Self::Tab => "ui-action--tab",
            Self::Segment => "ui-action--segment",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionSize {
    #[default]
    Compact,
    Standard,
    Prominent,
}

impl ActionSize {
    fn class_name(self) -> &'static str {
        match self {
            Self::Compact => "ui-action--compact",
            Self::Standard => "ui-action--standard",
            Self::Prominent => "ui-action--prominent",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ActionProps {
    pub children: Element,
    pub purpose: ActionPurpose,
    #[props(default)]
    pub variant: ActionVariant,
    #[props(default)]
    pub size: ActionSize,
    #[props(default)]
    pub class: Option<String>,
    #[props(default)]
    pub id: Option<String>,
    #[props(default)]
    pub title: Option<String>,
    #[props(default)]
    pub role: Option<String>,
    #[props(default)]
    pub tabindex: Option<String>,
    #[props(default)]
    pub draggable: Option<String>,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub aria_label: Option<String>,
    #[props(default)]
    pub aria_pressed: Option<bool>,
    #[props(default)]
    pub aria_selected: Option<bool>,
    #[props(default)]
    pub aria_expanded: Option<bool>,
    #[props(default)]
    pub aria_controls: Option<String>,
    #[props(default)]
    pub aria_current: Option<String>,
    #[props(default)]
    pub aria_busy: Option<bool>,
    #[props(default)]
    pub onclick: Option<EventHandler<MouseEvent>>,
    #[props(default)]
    pub onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(default)]
    pub ondragstart: Option<EventHandler<DragEvent>>,
    #[props(default)]
    pub onfocus: Option<EventHandler<FocusEvent>>,
    #[props(default)]
    pub onblur: Option<EventHandler<FocusEvent>>,
}

/// The only ordinary button primitive. Native button attributes and events are
/// forwarded, while the purpose remains visible in the DOM and in code review.
#[component]
pub fn Action(props: ActionProps) -> Element {
    let class = action_class(
        props.variant,
        props.size,
        props.class.as_deref().unwrap_or_default(),
    );
    let onclick = props.onclick;
    let onkeydown = props.onkeydown;
    let ondragstart = props.ondragstart;
    let onfocus = props.onfocus;
    let onblur = props.onblur;
    rsx! {
        button {
            id: props.id,
            r#type: "button",
            class,
            "data-action": props.purpose.as_str(),
            title: props.title,
            role: props.role,
            tabindex: props.tabindex,
            draggable: props.draggable,
            disabled: props.disabled,
            "aria-label": props.aria_label,
            "aria-pressed": props.aria_pressed,
            "aria-selected": props.aria_selected,
            "aria-expanded": props.aria_expanded,
            "aria-controls": props.aria_controls,
            "aria-current": props.aria_current,
            "aria-busy": props.aria_busy,
            onclick: move |event| if let Some(handler) = onclick { handler.call(event) },
            onkeydown: move |event| if let Some(handler) = onkeydown { handler.call(event) },
            ondragstart: move |event| if let Some(handler) = ondragstart { handler.call(event) },
            onfocus: move |event| if let Some(handler) = onfocus { handler.call(event) },
            onblur: move |event| if let Some(handler) = onblur { handler.call(event) },
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ActionLinkProps {
    pub children: Element,
    pub href: String,
    pub purpose: ActionPurpose,
    #[props(default)]
    pub variant: ActionVariant,
    #[props(default)]
    pub title: Option<String>,
    #[props(default)]
    pub download: Option<String>,
}

/// Anchor counterpart to [`Action`], for real downloads and navigation.
#[component]
pub fn ActionLink(props: ActionLinkProps) -> Element {
    let class = action_class(props.variant, ActionSize::Compact, "");
    rsx! {
        a {
            class,
            "data-action": props.purpose.as_str(),
            href: props.href,
            title: props.title,
            download: props.download,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FileActionProps {
    pub children: Element,
    pub accept: String,
    pub onchange: EventHandler<FormEvent>,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub aria_busy: Option<bool>,
    pub purpose: ActionPurpose,
    #[props(default = ActionVariant::Outline)]
    pub variant: ActionVariant,
    #[props(default)]
    pub class: Option<String>,
    #[props(default)]
    pub title: Option<String>,
    #[props(default)]
    pub input_id: Option<String>,
}

/// Keyboard-visible file picker styled by the same contract as every action.
#[component]
pub fn FileAction(props: FileActionProps) -> Element {
    let class = action_class(
        props.variant,
        ActionSize::Compact,
        props.class.as_deref().unwrap_or_default(),
    );
    rsx! {
        label {
            class,
            title: props.title,
            "data-action": props.purpose.as_str(),
            "aria-disabled": props.disabled,
            "aria-busy": props.aria_busy,
            input {
                id: props.input_id,
                class: "ui-file-input",
                r#type: "file",
                accept: props.accept,
                disabled: props.disabled,
                onchange: move |event| props.onchange.call(event),
            }
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ModeGroupProps {
    pub label: String,
    pub children: Element,
    #[props(default)]
    pub class: Option<String>,
}

#[component]
pub fn ModeGroup(props: ModeGroupProps) -> Element {
    let class = class_with("ui-mode-group", props.class.as_deref());
    rsx! {
        div {
            class,
            role: "radiogroup",
            "aria-label": props.label,
            onkeydown: move |event| {
                let direction = match event.key() {
                    Key::ArrowLeft | Key::ArrowUp => Some(-1),
                    Key::ArrowRight | Key::ArrowDown => Some(1),
                    _ => None,
                };
                if let Some(direction) = direction {
                    event.prevent_default();
                    event.stop_propagation();
                    select_adjacent_mode(direction);
                }
            },
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ModeActionProps {
    pub children: Element,
    pub checked: bool,
    pub purpose: ActionPurpose,
    #[props(default)]
    pub class: Option<String>,
    #[props(default)]
    pub id: Option<String>,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub aria_label: Option<String>,
    #[props(default)]
    pub onclick: Option<EventHandler<MouseEvent>>,
}

/// A mutually exclusive mode control with native radio semantics.
#[component]
pub fn ModeAction(props: ModeActionProps) -> Element {
    let class = action_class(
        ActionVariant::Segment,
        ActionSize::Compact,
        props.class.as_deref().unwrap_or_default(),
    );
    let onclick = props.onclick;
    rsx! {
        button {
            id: props.id,
            r#type: "button",
            class,
            "data-action": props.purpose.as_str(),
            role: "radio",
            "aria-checked": props.checked,
            tabindex: if props.checked { "0" } else { "-1" },
            "aria-label": props.aria_label,
            disabled: props.disabled,
            onclick: move |event| if let Some(handler) = onclick { handler.call(event) },
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleProps {
    pub label: String,
    pub checked: bool,
    pub onchange: EventHandler<bool>,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub title: Option<String>,
    #[props(default)]
    pub class: Option<String>,
    #[props(default = ActionPurpose::Toggle)]
    pub purpose: ActionPurpose,
}

/// A binary option whose label and hit target are always part of the control.
#[component]
pub fn Toggle(props: ToggleProps) -> Element {
    let class = class_with("ui-toggle", props.class.as_deref());
    rsx! {
        label {
            class,
            title: props.title,
            "data-action": props.purpose.as_str(),
            "aria-disabled": props.disabled,
            input {
                r#type: "checkbox",
                checked: props.checked,
                disabled: props.disabled,
                onchange: move |event| props.onchange.call(event.checked()),
            }
            span { "{props.label}" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct InlineConfirmationProps {
    pub id: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub purpose: ActionPurpose,
    pub on_confirm: EventHandler<()>,
    pub on_cancel: EventHandler<()>,
    pub return_focus_id: String,
    #[props(default)]
    pub pending: bool,
    #[props(default)]
    pub class: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
struct ConfirmationActionsProps {
    cancel_id: String,
    confirm_label: String,
    cancel_label: String,
    purpose: ActionPurpose,
    pending: bool,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
    return_focus_id: String,
}

#[component]
fn ConfirmationActions(props: ConfirmationActionsProps) -> Element {
    rsx! {
        div { class: "ui-confirmation-actions",
            Action {
                id: props.cancel_id,
                purpose: ActionPurpose::Cancel,
                variant: ActionVariant::Outline,
                disabled: props.pending,
                onclick: move |_| dismiss_confirmation(props.on_cancel, &props.return_focus_id),
                "{props.cancel_label}"
            }
            Action {
                purpose: props.purpose,
                variant: ActionVariant::Solid,
                disabled: props.pending,
                aria_busy: props.pending,
                onclick: move |_| props.on_confirm.call(()),
                "{props.confirm_label}"
            }
        }
    }
}

/// Tier-one confirmation for a local destructive action. The safe choice takes
/// focus, Escape cancels, and cancellation returns to the originating action.
#[component]
pub fn InlineConfirmation(props: InlineConfirmationProps) -> Element {
    let cancel_id = format!("{}-cancel", props.id);
    let class = class_with("ui-inline-confirmation", props.class.as_deref());
    let initial_focus_id = cancel_id.clone();
    use_effect(move || focus_after_render(initial_focus_id.clone()));

    let cancel_from_key = props.on_cancel;
    let return_from_key = props.return_focus_id.clone();
    rsx! {
        div {
            id: props.id,
            class,
            role: "group",
            "aria-label": "Confirmation",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape && !props.pending {
                    event.prevent_default();
                    dismiss_confirmation(cancel_from_key, &return_from_key);
                }
            },
            p { "{props.message}" }
            ConfirmationActions {
                cancel_id,
                confirm_label: props.confirm_label,
                cancel_label: props.cancel_label,
                purpose: props.purpose,
                pending: props.pending,
                on_confirm: props.on_confirm,
                on_cancel: props.on_cancel,
                return_focus_id: props.return_focus_id,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct BlockingConfirmationProps {
    pub id: String,
    pub title: String,
    pub message: String,
    pub consequence: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub purpose: ActionPurpose,
    pub on_confirm: EventHandler<()>,
    pub on_cancel: EventHandler<()>,
    pub return_focus_id: String,
    #[props(default)]
    pub pending: bool,
}

/// Tier-two confirmation for scope-wide or physical consequences.
#[component]
pub fn BlockingConfirmation(props: BlockingConfirmationProps) -> Element {
    let dialog_id = props.id.clone();
    let title_id = format!("{}-title", props.id);
    let description_id = format!("{}-description", props.id);
    let cancel_id = format!("{}-cancel", props.id);
    let initial_focus_id = cancel_id.clone();
    use_effect(move || focus_after_render(initial_focus_id.clone()));

    let cancel_from_key = props.on_cancel;
    let cancel_from_backdrop = props.on_cancel;
    let return_from_key = props.return_focus_id.clone();
    let return_from_backdrop = props.return_focus_id.clone();
    rsx! {
        div {
            class: "ui-dialog-backdrop",
            onclick: move |_| {
                if !props.pending {
                    dismiss_confirmation(cancel_from_backdrop, &return_from_backdrop);
                }
            },
        }
        section {
            id: props.id,
            class: "ui-dialog-surface ui-blocking-confirmation",
            role: "alertdialog",
            "aria-modal": "true",
            "aria-labelledby": title_id.clone(),
            "aria-describedby": description_id.clone(),
            tabindex: "-1",
            onclick: move |event| event.stop_propagation(),
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape && !props.pending {
                    event.prevent_default();
                    dismiss_confirmation(cancel_from_key, &return_from_key);
                } else if event.key() == Key::Tab
                    && trap_focus_within(&dialog_id, event.modifiers().shift())
                {
                    event.prevent_default();
                }
            },
            header {
                h2 { id: title_id, "{props.title}" }
            }
            div { id: description_id, class: "ui-confirmation-copy",
                p { "{props.message}" }
                p { class: "ui-confirmation-consequence", "{props.consequence}" }
            }
            footer {
                ConfirmationActions {
                    cancel_id,
                    confirm_label: props.confirm_label,
                    cancel_label: props.cancel_label,
                    purpose: props.purpose,
                    pending: props.pending,
                    on_confirm: props.on_confirm,
                    on_cancel: props.on_cancel,
                    return_focus_id: props.return_focus_id,
                }
            }
        }
    }
}

fn action_class(variant: ActionVariant, size: ActionSize, extra: &str) -> String {
    let base = format!("ui-action {} {}", variant.class_name(), size.class_name());
    class_with(&base, Some(extra))
}

fn class_with(base: &str, extra: Option<&str>) -> String {
    let extra = extra.map(str::trim).filter(|extra| !extra.is_empty());
    extra.map_or_else(|| base.to_owned(), |extra| format!("{base} {extra}"))
}

fn dismiss_confirmation(on_cancel: EventHandler<()>, return_focus_id: &str) {
    on_cancel.call(());
    focus_after_render(return_focus_id.to_owned());
}

/// Move focus after Dioxus has committed a replacement subtree.
pub fn focus_after_render(id: String) {
    dioxus::dioxus_core::spawn_forever(async move {
        for attempt in 0..16 {
            let delay_ms = if attempt == 0 { 0 } else { 16 };
            gloo_timers::future::TimeoutFuture::new(delay_ms).await;
            if focus_element(&id) {
                break;
            }
        }
    });
}

#[cfg(feature = "web")]
pub fn focus_element(id: &str) -> bool {
    use wasm_bindgen::JsCast as _;

    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return false;
    };
    element.focus().is_ok()
}

#[cfg(not(feature = "web"))]
pub fn focus_element(_id: &str) -> bool {
    false
}

#[cfg(feature = "web")]
fn select_adjacent_mode(direction: isize) {
    use wasm_bindgen::JsCast as _;

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(active) = document.active_element() else {
        return;
    };
    let Ok(Some(group)) = active.closest("[role='radiogroup']") else {
        return;
    };
    let Ok(nodes) = group.query_selector_all("[role='radio']:not([disabled])") else {
        return;
    };
    let radios = (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect::<Vec<_>>();
    if radios.is_empty() {
        return;
    }
    let current = radios
        .iter()
        .position(|radio| active.is_same_node(Some(radio.as_ref())))
        .unwrap_or_default();
    let next = (current as isize + direction).rem_euclid(radios.len() as isize) as usize;
    if let Some(radio) = radios.get(next) {
        let _ = radio.focus();
        radio.click();
    }
}

#[cfg(not(feature = "web"))]
fn select_adjacent_mode(_direction: isize) {}

#[cfg(feature = "web")]
pub fn trap_focus_within(container_id: &str, backwards: bool) -> bool {
    use wasm_bindgen::JsCast as _;

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    let Some(container) = document.get_element_by_id(container_id) else {
        return false;
    };
    let Ok(nodes) = container.query_selector_all(
        "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) else {
        return false;
    };
    let focusable = (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect::<Vec<_>>();
    if focusable.is_empty() {
        return false;
    }
    let current = document.active_element().and_then(|active| {
        focusable
            .iter()
            .position(|item| active.is_same_node(Some(item.as_ref())))
    });
    let next = if backwards {
        current
            .and_then(|index| index.checked_sub(1))
            .unwrap_or_else(|| focusable.len() - 1)
    } else {
        current.map_or(0, |index| (index + 1) % focusable.len())
    };
    focusable
        .get(next)
        .is_some_and(|element| element.focus().is_ok())
}

#[cfg(not(feature = "web"))]
pub fn trap_focus_within(_container_id: &str, _backwards: bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::{ActionPurpose, ActionSize, ActionVariant, action_class};

    #[test]
    fn action_vocabulary_is_stable() {
        assert_eq!(ActionPurpose::Delete.as_str(), "delete");
        assert_eq!(ActionPurpose::Remove.as_str(), "remove");
        assert_eq!(ActionPurpose::Close.as_str(), "close");
        assert_eq!(ActionPurpose::Discard.as_str(), "discard");
        assert_eq!(ActionPurpose::Sync.as_str(), "sync");
    }

    #[test]
    fn action_classes_separate_presentation_from_layout() {
        assert_eq!(
            action_class(ActionVariant::Outline, ActionSize::Standard, "local-layout"),
            "ui-action ui-action--outline ui-action--standard local-layout"
        );
        assert_eq!(
            action_class(ActionVariant::Quiet, ActionSize::Compact, ""),
            "ui-action ui-action--quiet ui-action--compact"
        );
    }
}
