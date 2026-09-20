//! Stylesheet installation and class composition shared by every primitive.

use dioxus::dioxus_core::AttributeValue;
use dioxus::prelude::*;

const UI_STYLES: Asset = asset!("/assets/ui.css");

/// Install the shared interface styles. Dioxus deduplicates identical assets,
/// so a reusable workspace can safely install this beside the product shell.
#[component]
pub fn UiStyles() -> Element {
    rsx! { document::Stylesheet { href: UI_STYLES } }
}

/// Forwarded attributes carry the caller's `class` beside everything else.
/// A primitive owns its own class list, so the caller's classes are lifted
/// out and joined after it, and the rest is spread onto the element.
pub(crate) trait AttributeClass {
    fn split_class_attribute(self) -> (String, Vec<Attribute>);

    /// The primitive's class list followed by the caller's, and the
    /// attributes left to forward.
    fn with_base_class(self, base: &str) -> (String, Vec<Attribute>);
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

    fn with_base_class(self, base: &str) -> (String, Vec<Attribute>) {
        let (extra, forwarded) = self.split_class_attribute();
        (base.with_class(Some(&extra)), forwarded)
    }
}

pub(crate) trait ClassNames {
    fn with_class(&self, extra: Option<&str>) -> String;
}

impl ClassNames for str {
    fn with_class(&self, extra: Option<&str>) -> String {
        let extra = extra.map(str::trim).filter(|extra| !extra.is_empty());
        extra.map_or_else(|| self.to_owned(), |extra| format!("{self} {extra}"))
    }
}

#[cfg(test)]
mod tests {
    use super::ClassNames;

    #[test]
    fn caller_classes_follow_the_primitive_and_blank_ones_vanish() {
        assert_eq!("ui-field".with_class(Some(" local ")), "ui-field local");
        assert_eq!("ui-field".with_class(Some("  ")), "ui-field");
        assert_eq!("ui-field".with_class(None), "ui-field");
    }
}
