//! Labelled entry: one field wrapper, one text input, one select. The
//! hairline box, 24px target, paper fill, and accent focus are spelled once
//! here; a surface owns only the grid its fields sit in.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

/// Where the label sits relative to its control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldLayout {
    /// The label above the control, for forms that read top to bottom.
    #[default]
    Stacked,
    /// The label beside the control in a fixed column, for one-line settings.
    Inline,
}

impl FieldLayout {
    fn class_name(self) -> &'static str {
        match self {
            Self::Stacked => "ui-field ui-field--stacked",
            Self::Inline => "ui-field ui-field--inline",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FieldProps {
    label: String,
    children: Element,
    #[props(default)]
    layout: FieldLayout,
    #[props(extends = GlobalAttributes, extends = label)]
    attributes: Vec<Attribute>,
}

/// A label and the control it names, as one hit target.
#[component]
pub fn Field(props: FieldProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class(props.layout.class_name());
    rsx! {
        label { class, ..attributes,
            span { class: "ui-field-label", "{props.label}" }
            {props.children}
        }
    }
}

/// What a text input accepts, which decides the browser's keyboard and
/// whether the value reads as a number.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextInputKind {
    #[default]
    Text,
    Search,
    /// The browser's numeric control, with `min`, `max`, and `step` forwarded.
    Number,
}

impl TextInputKind {
    fn input_type(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Search => "search",
            Self::Number => "number",
        }
    }

    fn class_name(self) -> &'static str {
        match self {
            Self::Number => "ui-input ui-input--numeric",
            Self::Text | Self::Search => "ui-input",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TextInputProps {
    value: String,
    oninput: EventHandler<String>,
    #[props(default)]
    kind: TextInputKind,
    #[props(default)]
    disabled: bool,
    /// Take focus when the input first appears, for an entry the operator
    /// opened on purpose such as a rename.
    #[props(default)]
    autofocus: bool,
    #[props(default)]
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    /// Focus left the input, as when an operator tabs or clicks away.
    #[props(default)]
    onblur: Option<EventHandler<FocusEvent>>,
    #[props(extends = GlobalAttributes, extends = input)]
    attributes: Vec<Attribute>,
}

/// Single-line text entry. Native input attributes such as `placeholder`,
/// `list`, `min`, `max`, and `step` are forwarded.
#[component]
pub fn TextInput(props: TextInputProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class(props.kind.class_name());
    let oninput = props.oninput;
    let onkeydown = props.onkeydown;
    let onblur = props.onblur;
    rsx! {
        input {
            class,
            r#type: props.kind.input_type(),
            value: props.value,
            disabled: props.disabled,
            autofocus: props.autofocus,
            oninput: move |event| oninput.call(event.value()),
            onkeydown: move |event| if let Some(handler) = onkeydown { handler.call(event) },
            onblur: move |event| if let Some(handler) = onblur { handler.call(event) },
            ..attributes,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SelectProps {
    value: String,
    onchange: EventHandler<String>,
    /// The `option` and `optgroup` elements to choose among.
    children: Element,
    #[props(default)]
    disabled: bool,
    #[props(extends = GlobalAttributes, extends = select)]
    attributes: Vec<Attribute>,
}

/// One choice among a closed list, reported as the chosen option's value.
#[component]
pub fn Select(props: SelectProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-select");
    let onchange = props.onchange;
    rsx! {
        select {
            class,
            value: props.value,
            disabled: props.disabled,
            onchange: move |event| onchange.call(event.value()),
            ..attributes,
            {props.children}
        }
    }
}
