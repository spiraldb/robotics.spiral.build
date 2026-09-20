//! A disclosure: one chevron, one summary line, and whatever the caller keeps
//! behind it. Native `details` does the toggling; this owns how it looks.

use dioxus::prelude::*;

use crate::foundation::{AttributeClass, ChevronIcon};

#[derive(Props, Clone, PartialEq)]
pub struct DisclosureProps {
    summary: Element,
    children: Element,
    /// `Some` forces the state; `None` leaves it to the operator.
    #[props(default)]
    open: Option<bool>,
    #[props(extends = GlobalAttributes, extends = details)]
    attributes: Vec<Attribute>,
}

#[component]
pub fn Disclosure(props: DisclosureProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-disclosure");
    rsx! {
        details { class, open: props.open, ..attributes,
            summary { class: "ui-disclosure-summary",
                ChevronIcon { open: false }
                span { class: "ui-disclosure-title", {props.summary} }
            }
            {props.children}
        }
    }
}
