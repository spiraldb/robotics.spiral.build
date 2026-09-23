//! A kind mark: one letter in a thin outline of its hue, naming what kind of
//! thing a row or heading is before its name is read.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

#[derive(Props, Clone, PartialEq)]
pub struct KindMarkProps {
    /// The one letter the mark shows.
    letter: char,
    /// A CSS colour the outline and letter take, usually a host token.
    hue: String,
    /// Read aloud in place of the letter.
    label: String,
    #[props(extends = GlobalAttributes, extends = span)]
    attributes: Vec<Attribute>,
}

#[component]
pub fn KindMark(props: KindMarkProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-kind-mark");
    rsx! {
        span { class, style: "--ui-kind-hue: {props.hue}", role: "img", "aria-label": props.label.clone(), title: props.label,
            ..attributes,
            "{props.letter}"
        }
    }
}
