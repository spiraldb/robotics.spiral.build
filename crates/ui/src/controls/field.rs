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
    Password,
    /// The browser's numeric control, with `min`, `max`, and `step` forwarded.
    Number,
    /// Free text with a decimal keypad, for thresholds the browser must not
    /// reformat while they are typed.
    Decimal,
    /// Free text with a digit keypad, for sizes and counts.
    Integer,
}

impl TextInputKind {
    fn input_type(self) -> &'static str {
        match self {
            Self::Text | Self::Decimal | Self::Integer => "text",
            Self::Search => "search",
            Self::Password => "password",
            Self::Number => "number",
        }
    }

    fn input_mode(self) -> Option<&'static str> {
        match self {
            Self::Decimal => Some("decimal"),
            Self::Integer => Some("numeric"),
            Self::Text | Self::Search | Self::Password | Self::Number => None,
        }
    }

    fn class_name(self) -> &'static str {
        match self {
            Self::Number | Self::Decimal | Self::Integer => "ui-input ui-input--numeric",
            Self::Text | Self::Search | Self::Password => "ui-input",
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
    rsx! {
        input {
            class,
            r#type: props.kind.input_type(),
            inputmode: props.kind.input_mode(),
            value: props.value,
            disabled: props.disabled,
            autofocus: props.autofocus,
            oninput: move |event| oninput.call(event.value()),
            onkeydown: move |event| if let Some(handler) = onkeydown { handler.call(event) },
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

#[cfg(test)]
mod tests {
    use super::TextInputKind;

    #[test]
    fn numeric_kinds_keep_free_text_but_ask_for_a_number_keypad() {
        assert_eq!(TextInputKind::Decimal.input_type(), "text");
        assert_eq!(TextInputKind::Decimal.input_mode(), Some("decimal"));
        assert_eq!(TextInputKind::Integer.input_mode(), Some("numeric"));
        assert_eq!(TextInputKind::Number.input_type(), "number");
        assert_eq!(TextInputKind::Number.input_mode(), None);
        assert_eq!(TextInputKind::Text.class_name(), "ui-input");
        assert_eq!(
            TextInputKind::Number.class_name(),
            "ui-input ui-input--numeric"
        );
    }
}
