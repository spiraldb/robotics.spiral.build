//! One typed interaction vocabulary for Spiral Robotics interfaces.
//!
//! Layout belongs to each product surface. Action meaning, interaction states,
//! confirmation behavior, focus movement, and target sizing belong here.

use dioxus::dioxus_core::AttributeValue;
use dioxus::prelude::*;

mod icons;

pub(crate) use icons::{ChevronIcon, CloseIcon, DeleteIcon, RefreshIcon, UnassignIcon};

const UI_STYLES: Asset = asset!("/assets/ui.css");

/// Install the shared interaction styles. Dioxus deduplicates identical assets,
/// so reusable workspaces can safely install this beside the product shell.
#[component]
pub(crate) fn UiStyles() -> Element {
    rsx! { document::Stylesheet { href: UI_STYLES } }
}

/// The stable verb behind an action. Product code chooses a verb before it
/// chooses an icon or visual treatment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionPurpose {
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
    Fit,
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
            Self::Fit => "fit",
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
pub(crate) enum ActionVariant {
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

    fn action_class(self, size: ActionSize, extra: &str) -> String {
        format!("ui-action {} {}", self.class_name(), size.class_name()).with_class(Some(extra))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ActionSize {
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
pub(crate) struct ActionProps {
    children: Element,
    purpose: ActionPurpose,
    #[props(default)]
    variant: ActionVariant,
    #[props(default)]
    size: ActionSize,
    #[props(extends = GlobalAttributes, extends = button)]
    attributes: Vec<Attribute>,
    #[props(default)]
    onclick: Option<EventHandler<MouseEvent>>,
    #[props(default)]
    auxiliary_handlers: ActionAuxiliaryHandlers,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct ActionAuxiliaryHandlers {
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    ondragstart: Option<EventHandler<DragEvent>>,
    onfocus: Option<EventHandler<FocusEvent>>,
    onblur: Option<EventHandler<FocusEvent>>,
}

impl ActionAuxiliaryHandlers {
    pub(crate) fn keydown(handler: impl FnMut(KeyboardEvent) + 'static) -> Self {
        Self {
            onkeydown: Some(EventHandler::new(handler)),
            ..Self::default()
        }
    }

    pub(crate) fn with_focus(
        mut self,
        onfocus: impl FnMut(FocusEvent) + 'static,
        onblur: impl FnMut(FocusEvent) + 'static,
    ) -> Self {
        self.onfocus = Some(EventHandler::new(onfocus));
        self.onblur = Some(EventHandler::new(onblur));
        self
    }
}

/// The only ordinary button primitive. Native button attributes and events are
/// forwarded, while the purpose remains visible in the DOM and in code review.
#[component]
pub(crate) fn Action(props: ActionProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = props.variant.action_class(props.size, &extra_class);
    let onclick = props.onclick;
    let handlers = props.auxiliary_handlers;
    rsx! {
        button {
            r#type: "button",
            class: class,
            "data-action": props.purpose.as_str(),
            onclick: move |event| if let Some(handler) = onclick { handler.call(event) },
            onkeydown: move |event| if let Some(handler) = handlers.onkeydown { handler.call(event) },
            ondragstart: move |event| if let Some(handler) = handlers.ondragstart { handler.call(event) },
            onfocus: move |event| if let Some(handler) = handlers.onfocus { handler.call(event) },
            onblur: move |event| if let Some(handler) = handlers.onblur { handler.call(event) },
            ..attributes,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct ActionLinkProps {
    children: Element,
    href: String,
    purpose: ActionPurpose,
    #[props(default)]
    variant: ActionVariant,
    #[props(default)]
    title: Option<String>,
    #[props(default)]
    download: Option<String>,
}

/// Anchor counterpart to [`Action()`], for real downloads and navigation.
#[component]
pub(crate) fn ActionLink(props: ActionLinkProps) -> Element {
    let class = props.variant.action_class(ActionSize::Compact, "");
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
pub(crate) struct FileActionProps {
    children: Element,
    input: FileInput,
    #[props(default)]
    disabled: bool,
    purpose: ActionPurpose,
    #[props(default = ActionVariant::Outline)]
    variant: ActionVariant,
    #[props(extends = GlobalAttributes, extends = label)]
    attributes: Vec<Attribute>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct FileInput {
    accept: String,
    onchange: EventHandler<FormEvent>,
    input_id: Option<String>,
}

impl FileInput {
    pub(crate) fn new(
        accept: impl Into<String>,
        onchange: impl FnMut(FormEvent) + 'static,
    ) -> Self {
        Self {
            accept: accept.into(),
            onchange: EventHandler::new(onchange),
            input_id: None,
        }
    }

    pub(crate) fn with_id(mut self, input_id: impl Into<String>) -> Self {
        self.input_id = Some(input_id.into());
        self
    }
}

/// Keyboard-visible file picker styled by the same contract as every action.
#[component]
pub(crate) fn FileAction(props: FileActionProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = props
        .variant
        .action_class(ActionSize::Compact, &extra_class);
    rsx! {
        label {
            class: class,
            "data-action": props.purpose.as_str(),
            "aria-disabled": props.disabled,
            ..attributes,
            input {
                id: props.input.input_id,
                class: "ui-file-input",
                r#type: "file",
                accept: props.input.accept,
                disabled: props.disabled,
                onchange: move |event| props.input.onchange.call(event),
            }
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct ModeGroupProps {
    label: String,
    children: Element,
    #[props(default)]
    class: Option<String>,
}

#[component]
pub(crate) fn ModeGroup(props: ModeGroupProps) -> Element {
    let class = "ui-mode-group".with_class(props.class.as_deref());
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
                    BrowserFocus::select_adjacent_mode(direction);
                }
            },
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct ModeActionProps {
    children: Element,
    checked: bool,
    purpose: ActionPurpose,
    #[props(extends = GlobalAttributes, extends = button)]
    attributes: Vec<Attribute>,
    #[props(default)]
    onclick: Option<EventHandler<MouseEvent>>,
}

/// A mutually exclusive mode control with native radio semantics.
#[component]
pub(crate) fn ModeAction(props: ModeActionProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = ActionVariant::Segment.action_class(ActionSize::Compact, &extra_class);
    let onclick = props.onclick;
    rsx! {
        button {
            r#type: "button",
            class: class,
            "data-action": props.purpose.as_str(),
            role: "radio",
            "aria-checked": props.checked,
            tabindex: if props.checked { "0" } else { "-1" },
            onclick: move |event| if let Some(handler) = onclick { handler.call(event) },
            ..attributes,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct ToggleProps {
    label: String,
    checked: bool,
    onchange: EventHandler<bool>,
    #[props(default)]
    disabled: bool,
    #[props(default)]
    title: Option<String>,
    #[props(default)]
    class: Option<String>,
    #[props(default = ActionPurpose::Toggle)]
    purpose: ActionPurpose,
}

/// A binary option whose label and hit target are always part of the control.
#[component]
pub(crate) fn Toggle(props: ToggleProps) -> Element {
    let class = "ui-toggle".with_class(props.class.as_deref());
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
pub(crate) struct InlineConfirmationProps {
    id: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
    #[props(default)]
    class: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
struct ConfirmationActionsProps {
    cancel_id: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConfirmationCopy {
    message: String,
    confirm_label: String,
    cancel_label: String,
}

impl ConfirmationCopy {
    pub(crate) fn new(
        message: impl Into<String>,
        confirm_label: impl Into<String>,
        cancel_label: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            confirm_label: confirm_label.into(),
            cancel_label: cancel_label.into(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConfirmationResponse {
    purpose: ActionPurpose,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
}

impl ConfirmationResponse {
    pub(crate) fn new(
        purpose: ActionPurpose,
        on_confirm: impl FnMut(()) + 'static,
        on_cancel: impl FnMut(()) + 'static,
    ) -> Self {
        Self {
            purpose,
            on_confirm: EventHandler::new(on_confirm),
            on_cancel: EventHandler::new(on_cancel),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConfirmationDismissal {
    return_focus_id: String,
    pending: bool,
}

impl ConfirmationDismissal {
    pub(crate) fn new(return_focus_id: impl Into<String>) -> Self {
        Self {
            return_focus_id: return_focus_id.into(),
            pending: false,
        }
    }

    pub(crate) const fn with_pending(mut self, pending: bool) -> Self {
        self.pending = pending;
        self
    }
}

#[component]
fn ConfirmationActions(props: ConfirmationActionsProps) -> Element {
    rsx! {
        div { class: "ui-confirmation-actions",
            Action {
                id: props.cancel_id,
                purpose: ActionPurpose::Cancel,
                variant: ActionVariant::Outline,
                disabled: props.dismissal.pending,
                onclick: move |_| BrowserFocus::dismiss_confirmation(
                    props.response.on_cancel,
                    &props.dismissal.return_focus_id,
                ),
                "{props.copy.cancel_label}"
            }
            Action {
                purpose: props.response.purpose,
                variant: ActionVariant::Solid,
                disabled: props.dismissal.pending,
                aria_busy: props.dismissal.pending,
                onclick: move |_| props.response.on_confirm.call(()),
                "{props.copy.confirm_label}"
            }
        }
    }
}

/// Tier-one confirmation for a local destructive action. The safe choice takes
/// focus, Escape cancels, and cancellation returns to the originating action.
#[component]
pub(crate) fn InlineConfirmation(props: InlineConfirmationProps) -> Element {
    let cancel_id = format!("{}-cancel", props.id);
    let class = "ui-inline-confirmation".with_class(props.class.as_deref());
    let initial_focus_id = cancel_id.clone();
    use_effect(move || BrowserFocus::after_render(initial_focus_id.clone()));

    let cancel_from_key = props.response.on_cancel;
    let return_from_key = props.dismissal.return_focus_id.clone();
    let pending = props.dismissal.pending;
    rsx! {
        div {
            id: props.id,
            class,
            role: "group",
            "aria-label": "Confirmation",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape && !pending {
                    event.prevent_default();
                    BrowserFocus::dismiss_confirmation(cancel_from_key, &return_from_key);
                }
            },
            p { "{props.copy.message}" }
            ConfirmationActions {
                cancel_id,
                copy: props.copy,
                response: props.response,
                dismissal: props.dismissal,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct BlockingConfirmationProps {
    id: String,
    title: String,
    consequence: String,
    copy: ConfirmationCopy,
    response: ConfirmationResponse,
    dismissal: ConfirmationDismissal,
}

/// Tier-two confirmation for scope-wide or physical consequences.
#[component]
pub(crate) fn BlockingConfirmation(props: BlockingConfirmationProps) -> Element {
    let dialog_id = props.id.clone();
    let title_id = format!("{}-title", props.id);
    let description_id = format!("{}-description", props.id);
    let cancel_id = format!("{}-cancel", props.id);
    let initial_focus_id = cancel_id.clone();
    use_effect(move || BrowserFocus::after_render(initial_focus_id.clone()));

    let cancel_from_key = props.response.on_cancel;
    let cancel_from_backdrop = props.response.on_cancel;
    let return_from_key = props.dismissal.return_focus_id.clone();
    let return_from_backdrop = props.dismissal.return_focus_id.clone();
    let pending = props.dismissal.pending;
    rsx! {
        div {
            class: "ui-dialog-backdrop",
            onclick: move |_| {
                if !pending {
                    BrowserFocus::dismiss_confirmation(cancel_from_backdrop, &return_from_backdrop);
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
                if event.key() == Key::Escape && !pending {
                    event.prevent_default();
                    BrowserFocus::dismiss_confirmation(cancel_from_key, &return_from_key);
                } else if event.key() == Key::Tab
                    && BrowserFocus::trap_within(&dialog_id, event.modifiers().shift())
                {
                    event.prevent_default();
                }
            },
            header {
                h2 { id: title_id, "{props.title}" }
            }
            div { id: description_id, class: "ui-confirmation-copy",
                p { "{props.copy.message}" }
                p { class: "ui-confirmation-consequence", "{props.consequence}" }
            }
            footer {
                ConfirmationActions {
                    cancel_id,
                    copy: props.copy,
                    response: props.response,
                    dismissal: props.dismissal,
                }
            }
        }
    }
}

trait AttributeClass {
    fn split_class_attribute(self) -> (String, Vec<Attribute>);
}

impl AttributeClass for Vec<Attribute> {
    fn split_class_attribute(self) -> (String, Vec<Attribute>) {
        let mut class_names = Vec::new();
        let mut forwarded = Vec::with_capacity(self.len());
        for attribute in self {
            match attribute {
                Attribute {
                    name: "class",
                    value: AttributeValue::Text(class_name),
                    ..
                } => class_names.push(class_name),
                attribute => forwarded.push(attribute),
            }
        }
        (class_names.join(" "), forwarded)
    }
}

trait ClassNames {
    fn with_class(&self, extra: Option<&str>) -> String;
}

impl ClassNames for str {
    fn with_class(&self, extra: Option<&str>) -> String {
        let extra = extra.map(str::trim).filter(|extra| !extra.is_empty());
        extra.map_or_else(|| self.to_owned(), |extra| format!("{self} {extra}"))
    }
}

pub(crate) struct BrowserFocus;

impl BrowserFocus {
    fn dismiss_confirmation(on_cancel: EventHandler<()>, return_focus_id: &str) {
        on_cancel.call(());
        Self::after_render(return_focus_id.to_owned());
    }

    /// Move focus after Dioxus has committed a replacement subtree.
    pub(crate) fn after_render(id: String) {
        dioxus::dioxus_core::spawn_forever(async move {
            for attempt in 0..16 {
                let delay_ms = if attempt == 0 { 0 } else { 16 };
                gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                if Self::element(&id) {
                    break;
                }
            }
        });
    }

    #[cfg(feature = "web")]
    fn element(id: &str) -> bool {
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
    fn element(_id: &str) -> bool {
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
    fn trap_within(container_id: &str, backwards: bool) -> bool {
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
    fn trap_within(_container_id: &str, _backwards: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionPurpose, ActionSize, ActionVariant};

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
            ActionVariant::Outline.action_class(ActionSize::Standard, "local-layout"),
            "ui-action ui-action--outline ui-action--standard local-layout"
        );
        assert_eq!(
            ActionVariant::Quiet.action_class(ActionSize::Compact, ""),
            "ui-action ui-action--quiet ui-action--compact"
        );
    }
}
