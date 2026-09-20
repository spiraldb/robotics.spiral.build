//! A side sheet: the docked inspector that opens beside a table or a grid
//! for the one item the operator selected. It owns the heading, the
//! previous / next / close controls, the keyboard (Escape closes, ↑ and ↓
//! step), and the entering motion; the caller owns what the sheet says.

use dioxus::prelude::*;

use crate::controls::{ActionPurpose, Button, ButtonVariant};
use crate::foundation::{ArrowDownIcon, ArrowUpIcon, CloseIcon};

/// Where the inspected item sits among its siblings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SheetPosition {
    index: usize,
    count: usize,
}

impl SheetPosition {
    pub const fn new(index: usize, count: usize) -> Self {
        Self { index, count }
    }
    pub const fn index(self) -> usize {
        self.index
    }
    pub const fn count(self) -> usize {
        self.count
    }
    /// The neighbouring position, when there is one.
    pub fn stepped(self, delta: isize) -> Option<Self> {
        let index = self.index.checked_add_signed(delta)?;
        (index < self.count).then_some(Self {
            index,
            count: self.count,
        })
    }
}

/// What the sheet is about: the heading's id (focus lands there when the
/// sheet opens), its text, the faint detail after it, and the noun the step
/// controls name.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetHeading {
    id: String,
    title: String,
    detail: Option<String>,
    noun: &'static str,
}

impl SheetHeading {
    pub fn new(id: impl Into<String>, title: impl Into<String>, noun: &'static str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            noun,
        }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SideSheetProps {
    heading: SheetHeading,
    position: SheetPosition,
    on_step: EventHandler<isize>,
    on_close: EventHandler<()>,
    children: Element,
}

#[component]
pub fn SideSheet(props: SideSheetProps) -> Element {
    let SideSheetProps {
        heading,
        position,
        on_step,
        on_close,
        children,
    } = props;
    let noun = heading.noun;
    rsx! {
        aside { class: "ui-side-sheet", "aria-label": "{heading.title} details",
            onkeydown: move |event: KeyboardEvent| {
                match event.key() {
                    Key::Escape => {
                        event.prevent_default();
                        on_close.call(());
                    }
                    Key::ArrowUp => {
                        event.prevent_default();
                        on_step.call(-1);
                    }
                    Key::ArrowDown => {
                        event.prevent_default();
                        on_step.call(1);
                    }
                    _ => {}
                }
            },
            header { class: "ui-side-sheet-heading",
                h3 { id: "{heading.id}", tabindex: "-1",
                    "{heading.title}"
                    if let Some(detail) = heading.detail.as_ref() {
                        span { class: "ui-side-sheet-count", " {detail}" }
                    }
                }
                div { class: "ui-side-sheet-nav", role: "group", "aria-label": "Step between {noun}s",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                        aria_label: "Previous {noun}", title: "Previous {noun} · ↑",
                        disabled: position.stepped(-1).is_none(),
                        onclick: move |_| on_step.call(-1), ArrowUpIcon {} }
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                        aria_label: "Next {noun}", title: "Next {noun} · ↓",
                        disabled: position.stepped(1).is_none(),
                        onclick: move |_| on_step.call(1), ArrowDownIcon {} }
                    Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Icon,
                        aria_label: "Close {noun} details", title: "Close · Esc",
                        onclick: move |_| on_close.call(()), CloseIcon {} }
                }
            }
            {children}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SheetPosition;

    #[test]
    fn stepping_stops_at_either_end() {
        let first = SheetPosition::new(0, 3);
        assert_eq!(first.stepped(-1), None);
        assert_eq!(first.stepped(1), Some(SheetPosition::new(1, 3)));
        assert_eq!(SheetPosition::new(2, 3).stepped(1), None);
    }
}
