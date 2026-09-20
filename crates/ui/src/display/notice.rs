//! Notices and empty states: what a surface says when it has nothing else to
//! show, or has something the operator must read first.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NoticeTone {
    #[default]
    Quiet,
    Caution,
    Danger,
}

impl NoticeTone {
    fn class_suffix(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Caution => "caution",
            Self::Danger => "danger",
        }
    }

    /// A failure interrupts; everything else waits its turn.
    fn role(self) -> &'static str {
        match self {
            Self::Danger => "alert",
            Self::Quiet | Self::Caution => "status",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NoticeProps {
    children: Element,
    #[props(default)]
    tone: NoticeTone,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// One line of status inside a panel, announced to assistive technology.
#[component]
pub fn Notice(props: NoticeProps) -> Element {
    let base = format!("ui-notice ui-notice--{}", props.tone.class_suffix());
    let (class, attributes) = props.attributes.with_base_class(&base);
    rsx! {
        div { class, role: props.tone.role(), ..attributes, {props.children} }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct EmptyStateProps {
    #[props(default)]
    icon: Option<Element>,
    /// Without a title the state is a quiet aside rather than a heading.
    #[props(default)]
    title: Option<String>,
    /// The sentence or two beneath the title, and any action that resolves it.
    children: Element,
    #[props(default)]
    tone: NoticeTone,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// A panel with nothing to show says so in its centre.
#[component]
pub fn EmptyState(props: EmptyStateProps) -> Element {
    let base = if props.title.is_some() {
        "ui-empty"
    } else {
        "ui-empty ui-empty--quiet"
    };
    let (class, attributes) = props.attributes.with_base_class(base);
    rsx! {
        div { class, role: props.tone.role(), ..attributes,
            if let Some(icon) = props.icon {
                {icon}
            }
            if let Some(title) = props.title {
                strong { "{title}" }
            }
            {props.children}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NoticeTone;

    #[test]
    fn only_danger_interrupts() {
        assert_eq!(NoticeTone::Danger.role(), "alert");
        assert_eq!(NoticeTone::Caution.role(), "status");
        assert_eq!(NoticeTone::Quiet.role(), "status");
    }
}
