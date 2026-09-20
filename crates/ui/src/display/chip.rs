//! A chip: one word of state on a tonal wash, present only while it matters.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChipTone {
    #[default]
    Neutral,
    Accent,
    Caution,
    Danger,
    Good,
}

impl ChipTone {
    fn class_name(self) -> &'static str {
        match self {
            Self::Neutral => "ui-chip ui-chip--neutral",
            Self::Accent => "ui-chip ui-chip--accent",
            Self::Caution => "ui-chip ui-chip--caution",
            Self::Danger => "ui-chip ui-chip--danger",
            Self::Good => "ui-chip ui-chip--good",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ChipProps {
    children: Element,
    #[props(default)]
    tone: ChipTone,
    #[props(extends = GlobalAttributes, extends = span)]
    attributes: Vec<Attribute>,
}

#[component]
pub fn Chip(props: ChipProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class(props.tone.class_name());
    rsx! {
        span { class, ..attributes, {props.children} }
    }
}
