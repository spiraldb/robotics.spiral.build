//! Buttons: the one vocabulary every interactive control speaks.
//!
//! Product code chooses a verb ([`ActionPurpose`]) before it chooses a look
//! ([`ButtonVariant`]). Interaction state, target sizing, the destructive
//! treatment, and focus movement live here; a surface owns only placement.

use dioxus::prelude::*;

use crate::foundation::{AttributeClass, BrowserFocus, ClassNames, RovingGroup, RovingStep};

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
    Copy,
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
            Self::Copy => "copy",
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
pub enum ButtonVariant {
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

impl ButtonVariant {
    fn class_name(self) -> &'static str {
        match self {
            Self::Quiet => "ui-button--quiet",
            Self::Outline => "ui-button--outline",
            Self::Solid => "ui-button--solid",
            Self::Link => "ui-button--link",
            Self::Icon => "ui-button--icon",
            Self::Row => "ui-button--row",
            Self::Tab => "ui-button--tab",
            Self::Segment => "ui-button--segment",
        }
    }

    fn action_class(self, size: ButtonSize, extra: &str) -> String {
        format!("ui-button {} {}", self.class_name(), size.class_name()).with_class(Some(extra))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonSize {
    #[default]
    Compact,
    Standard,
    Prominent,
}

impl ButtonSize {
    fn class_name(self) -> &'static str {
        match self {
            Self::Compact => "ui-button--compact",
            Self::Standard => "ui-button--standard",
            Self::Prominent => "ui-button--prominent",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ButtonProps {
    children: Element,
    purpose: ActionPurpose,
    #[props(default)]
    variant: ButtonVariant,
    #[props(default)]
    size: ButtonSize,
    #[props(extends = GlobalAttributes, extends = button)]
    attributes: Vec<Attribute>,
    /// A handler that does nothing by default rather than an `Option`:
    /// Dioxus 0.7 frees an optional handler that a re-render adds to a
    /// mounted component (one diffed in place from a button without one),
    /// and the next render after that panics on it.
    #[props(default)]
    onclick: EventHandler<MouseEvent>,
    #[props(default)]
    auxiliary_handlers: ButtonAuxiliaryHandlers,
}

#[derive(Clone, Default, PartialEq)]
pub struct ButtonAuxiliaryHandlers {
    onfocus: Option<EventHandler<FocusEvent>>,
    onblur: Option<EventHandler<FocusEvent>>,
}

impl ButtonAuxiliaryHandlers {
    pub fn with_focus(
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
pub fn Button(props: ButtonProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = props.variant.action_class(props.size, &extra_class);
    let onclick = props.onclick;
    let handlers = props.auxiliary_handlers;
    rsx! {
        button {
            r#type: "button",
            class: class,
            "data-action": props.purpose.as_str(),
            onclick: move |event| onclick.call(event),
            onfocus: move |event| if let Some(handler) = handlers.onfocus { handler.call(event) },
            onblur: move |event| if let Some(handler) = handlers.onblur { handler.call(event) },
            ..attributes,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct LinkButtonProps {
    children: Element,
    href: String,
    purpose: ActionPurpose,
    #[props(default)]
    variant: ButtonVariant,
    #[props(default)]
    title: Option<String>,
    #[props(default)]
    download: Option<String>,
    /// Open the page in a new tab that cannot reach back into this one.
    #[props(default)]
    new_tab: bool,
}

/// Anchor counterpart to [`Button()`], for real downloads and navigation.
#[component]
pub fn LinkButton(props: LinkButtonProps) -> Element {
    let class = props.variant.action_class(ButtonSize::Compact, "");
    rsx! {
        a {
            class,
            "data-action": props.purpose.as_str(),
            href: props.href,
            title: props.title,
            download: props.download,
            target: props.new_tab.then_some("_blank"),
            rel: props.new_tab.then_some("noopener noreferrer"),
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FileButtonProps {
    children: Element,
    input: FileInput,
    #[props(default)]
    disabled: bool,
    purpose: ActionPurpose,
    #[props(default = ButtonVariant::Outline)]
    variant: ButtonVariant,
    #[props(extends = GlobalAttributes, extends = label)]
    attributes: Vec<Attribute>,
}

#[derive(Clone, PartialEq)]
pub struct FileInput {
    accept: String,
    onchange: EventHandler<FormEvent>,
    input_id: Option<String>,
}

impl FileInput {
    pub fn new(accept: impl Into<String>, onchange: impl FnMut(FormEvent) + 'static) -> Self {
        Self {
            accept: accept.into(),
            onchange: EventHandler::new(onchange),
            input_id: None,
        }
    }

    pub fn with_id(mut self, input_id: impl Into<String>) -> Self {
        self.input_id = Some(input_id.into());
        self
    }
}

/// Keyboard-visible file picker styled by the same contract as every action.
#[component]
pub fn FileButton(props: FileButtonProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = props
        .variant
        .action_class(ButtonSize::Compact, &extra_class);
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
pub struct ModeGroupProps {
    label: String,
    children: Element,
    #[props(default)]
    class: Option<String>,
}

#[component]
pub fn ModeGroup(props: ModeGroupProps) -> Element {
    let class = "ui-mode-group".with_class(props.class.as_deref());
    rsx! {
        div {
            class,
            role: "radiogroup",
            "aria-label": props.label,
            onkeydown: move |event| {
                let step = match event.key() {
                    Key::ArrowLeft | Key::ArrowUp => RovingStep::Adjacent(-1),
                    Key::ArrowRight | Key::ArrowDown => RovingStep::Adjacent(1),
                    _ => return,
                };
                event.prevent_default();
                event.stop_propagation();
                BrowserFocus::rove(RovingGroup::MODES, step, true);
            },
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ModeButtonProps {
    children: Element,
    checked: bool,
    purpose: ActionPurpose,
    #[props(extends = GlobalAttributes, extends = button)]
    attributes: Vec<Attribute>,
    /// Not an `Option`, for the reason [`ButtonProps`]' is not.
    #[props(default)]
    onclick: EventHandler<MouseEvent>,
}

/// A mutually exclusive mode control with native radio semantics.
#[component]
pub fn ModeButton(props: ModeButtonProps) -> Element {
    let (extra_class, attributes) = props.attributes.split_class_attribute();
    let class = ButtonVariant::Segment.action_class(ButtonSize::Compact, &extra_class);
    let onclick = props.onclick;
    rsx! {
        button {
            r#type: "button",
            class: class,
            "data-action": props.purpose.as_str(),
            role: "radio",
            "aria-checked": props.checked,
            tabindex: if props.checked { "0" } else { "-1" },
            onclick: move |event| onclick.call(event),
            ..attributes,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleProps {
    label: String,
    checked: bool,
    onchange: EventHandler<bool>,
    #[props(default)]
    disabled: bool,
    #[props(default)]
    title: Option<String>,
    #[props(default)]
    class: Option<String>,
}

/// A binary option whose label and hit target are always part of the control.
#[component]
pub fn Toggle(props: ToggleProps) -> Element {
    let class = "ui-toggle".with_class(props.class.as_deref());
    rsx! {
        label {
            class,
            title: props.title,
            "data-action": ActionPurpose::Toggle.as_str(),
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

#[cfg(test)]
mod tests {
    use super::{ActionPurpose, Button, ButtonSize, ButtonVariant};
    use dioxus::prelude::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A form's Search button that becomes Stop while it runs, as one
    /// button diffed in place.
    #[derive(Clone, Default)]
    struct SearchOrStop {
        running: Rc<RefCell<Option<Signal<u32>>>>,
    }

    impl SearchOrStop {
        fn render(self) -> Element {
            let running = use_signal(|| 0_u32);
            *self.running.borrow_mut() = Some(running);
            rsx! {
                if running() > 0 {
                    Button { purpose: ActionPurpose::Stop, onclick: move |_| {}, "Stop" }
                } else {
                    Button { purpose: ActionPurpose::Start, r#type: "submit", "Search" }
                }
            }
        }
    }

    /// The regression for the panic that froze Find similar: a button that
    /// gained a click handler on one render lost it to Dioxus, and the next
    /// render panicked on the freed handler.
    #[test]
    fn a_button_that_gains_a_handler_keeps_it_through_later_renders() {
        let harness = SearchOrStop::default();
        let mut dom = VirtualDom::new_with_props(SearchOrStop::render, harness.clone());
        dom.rebuild_in_place();
        let mut running = harness.running.borrow().expect("the harness rendered");
        for step in 1..=3 {
            dom.in_scope(ScopeId::ROOT, || running.set(step));
            dom.render_immediate_to_vec();
        }
    }

    #[test]
    fn action_vocabulary_is_stable() {
        assert_eq!(ActionPurpose::Delete.as_str(), "delete");
        assert_eq!(ActionPurpose::Remove.as_str(), "remove");
        assert_eq!(ActionPurpose::Close.as_str(), "close");
        assert_eq!(ActionPurpose::Discard.as_str(), "discard");
        assert_eq!(ActionPurpose::Sync.as_str(), "sync");
    }

    #[test]
    fn button_classes_separate_presentation_from_layout() {
        assert_eq!(
            ButtonVariant::Outline.action_class(ButtonSize::Standard, "local-layout"),
            "ui-button ui-button--outline ui-button--standard local-layout"
        );
        assert_eq!(
            ButtonVariant::Quiet.action_class(ButtonSize::Compact, ""),
            "ui-button ui-button--quiet ui-button--compact"
        );
    }
}
