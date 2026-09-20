//! Facts: a label and the value it names, drawn the same way whether three
//! of them sit in a stat strip or twelve stack down an inspector.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

/// How the facts are arranged. The label-over-value or label-beside-value
/// type is the same in every layout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FactLayout {
    /// Labels down the left, values down the right.
    #[default]
    Columns,
    /// Each fact a small tile, label over value, wrapping to fit.
    Tiles,
    /// Tiles in one row divided by hairlines, for a compact readout strip.
    Strip,
}

impl FactLayout {
    fn class_name(self) -> &'static str {
        match self {
            Self::Columns => "ui-facts ui-facts--columns",
            Self::Tiles => "ui-facts ui-facts--tiles",
            Self::Strip => "ui-facts ui-facts--strip",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FactListProps {
    children: Element,
    #[props(default)]
    layout: FactLayout,
    /// Values are data (counts, durations, ids) and set in mono with tabular
    /// figures, rather than sentences.
    #[props(default)]
    mono: bool,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// A description list of [`Fact`] children.
#[component]
pub fn FactList(props: FactListProps) -> Element {
    let base = if props.mono {
        format!("{} ui-facts--mono", props.layout.class_name())
    } else {
        props.layout.class_name().to_owned()
    };
    let (class, attributes) = props.attributes.with_base_class(&base);
    rsx! {
        dl { class, ..attributes, {props.children} }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FactProps {
    label: String,
    children: Element,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// One labelled value inside a [`FactList`].
#[component]
pub fn Fact(props: FactProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-fact");
    rsx! {
        div { class, ..attributes,
            dt { "{props.label}" }
            dd { {props.children} }
        }
    }
}
