//! Stylesheet installation and class composition shared by every primitive.

use dioxus::dioxus_core::AttributeValue;
use dioxus::prelude::*;

const UI_STYLES: Asset = asset!("/assets/ui.css");

/// Install the shared interface styles. Dioxus deduplicates identical assets,
/// so a reusable workspace can safely install this beside the product shell.
#[component]
pub fn UiStyles() -> Element {
    rsx! {
        document::Style { {Typeface::font_faces()} }
        document::Stylesheet { href: UI_STYLES }
    }
}

/// One self-hosted face file: Geist and Geist Mono ship with the kit so the
/// interface reads the same on every platform. Each family is split into the
/// Latin and Latin Extended ranges, so a page of plain Latin text fetches one
/// small file per family.
struct Typeface {
    family: &'static str,
    file: Asset,
    unicode_range: &'static str,
}

impl Typeface {
    const LATIN: &'static str = "U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+0304, U+0308, U+0329, U+2000-206F, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD";
    const LATIN_EXTENDED: &'static str = "U+0100-02BA, U+02BD-02C5, U+02C7-02CC, U+02CE-02D7, U+02DD-02FF, U+0304, U+0308, U+0329, U+1D00-1DBF, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F, U+A720-A7FF";

    const ALL: [Self; 4] = [
        Self {
            family: "Geist",
            file: asset!("/assets/fonts/geist-latin.woff2"),
            unicode_range: Self::LATIN,
        },
        Self {
            family: "Geist",
            file: asset!("/assets/fonts/geist-latin-ext.woff2"),
            unicode_range: Self::LATIN_EXTENDED,
        },
        Self {
            family: "Geist Mono",
            file: asset!("/assets/fonts/geist-mono-latin.woff2"),
            unicode_range: Self::LATIN,
        },
        Self {
            family: "Geist Mono",
            file: asset!("/assets/fonts/geist-mono-latin-ext.woff2"),
            unicode_range: Self::LATIN_EXTENDED,
        },
    ];

    /// The `@font-face` rules for every file. The faces are variable, so one
    /// rule per range covers every weight.
    fn font_faces() -> String {
        Self::ALL
            .iter()
            .map(|face| {
                format!(
                    "@font-face{{font-family:\"{}\";font-style:normal;font-weight:100 900;font-display:swap;src:url(\"{}\") format(\"woff2\");unicode-range:{};}}",
                    face.family, face.file, face.unicode_range
                )
            })
            .collect()
    }
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
