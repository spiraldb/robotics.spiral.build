//! A tree: nested rows the keyboard walks with arrows. A branch discloses
//! with a chevron, every row is one selectable surface, and the caller owns
//! what a row says.

use dioxus::prelude::*;

use crate::foundation::{AttributeClass, BrowserFocus, ChevronIcon, RovingGroup, RovingStep};

#[derive(Props, Clone, PartialEq)]
pub struct TreeProps {
    label: String,
    children: Element,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// The root list. ↑ and ↓ move between visible rows, Home and End jump to the
/// first and last; a row's own keys open and close its branch.
#[component]
pub fn Tree(props: TreeProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-tree");
    rsx! {
        ul { class, role: "tree", "aria-label": props.label,
            onkeydown: move |event| {
                let step = match event.key() {
                    Key::ArrowUp => RovingStep::Adjacent(-1),
                    Key::ArrowDown => RovingStep::Adjacent(1),
                    Key::Home => RovingStep::First,
                    Key::End => RovingStep::Last,
                    _ => return,
                };
                event.prevent_default();
                event.stop_propagation();
                BrowserFocus::rove(RovingGroup::TREE, step, false);
            },
            ..attributes,
            {props.children}
        }
    }
}

/// How a branch row discloses the rows beneath it.
#[derive(Clone, PartialEq)]
pub struct TreeBranch {
    name: String,
    expanded: bool,
    ontoggle: EventHandler<bool>,
}

impl TreeBranch {
    /// `name` is what the expander's label says it opens.
    pub fn new(
        name: impl Into<String>,
        expanded: bool,
        ontoggle: impl FnMut(bool) + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            expanded,
            ontoggle: EventHandler::new(ontoggle),
        }
    }

    fn toggle_label(&self) -> String {
        if self.expanded {
            format!("Collapse {}", self.name)
        } else {
            format!("Expand {}", self.name)
        }
    }

    /// → opens a closed branch, ← closes an open one; either way the key is
    /// spent here. A key this branch cannot use falls through so the row can
    /// move focus instead.
    fn handle_key(&self, key: &Key) -> bool {
        match key {
            Key::ArrowRight if !self.expanded => self.ontoggle.call(true),
            Key::ArrowLeft if self.expanded => self.ontoggle.call(false),
            _ => return false,
        }
        true
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TreeItemProps {
    /// The row's content: an icon and a name, or a small grid of them.
    label: Element,
    #[props(default)]
    branch: Option<TreeBranch>,
    #[props(default)]
    selected: bool,
    #[props(default)]
    onselect: Option<EventHandler<()>>,
    /// Counts and row-local actions after the label.
    #[props(default)]
    trailing: Option<Element>,
    /// The nested rows, rendered only while the branch is expanded.
    #[props(default)]
    children: Element,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

#[component]
pub fn TreeItem(props: TreeItemProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-tree-item");
    let expanded = props.branch.as_ref().map(|branch| branch.expanded);
    let onselect = props.onselect;
    let keyed_branch = props.branch.clone();
    let row_keys = move |event: KeyboardEvent| {
        let key = event.key();
        if !matches!(key, Key::ArrowLeft | Key::ArrowRight) {
            return;
        }
        event.prevent_default();
        event.stop_propagation();
        if keyed_branch
            .as_ref()
            .is_some_and(|branch| branch.handle_key(&key))
        {
            return;
        }
        match key {
            // An open branch's → steps into its first child.
            Key::ArrowRight if keyed_branch.is_some() => {
                BrowserFocus::rove(RovingGroup::TREE, RovingStep::Adjacent(1), false);
            }
            Key::ArrowLeft => BrowserFocus::focus_parent_tree_item(),
            _ => {}
        }
    };
    rsx! {
        li { class, role: "treeitem", "aria-expanded": expanded, "data-selected": props.selected, ..attributes,
            // One row is one surface. The chevron and the label are plain
            // buttons inside it so the row's wash, not a pill, shows state.
            div { class: "ui-tree-row",
                if let Some(branch) = props.branch.as_ref() {
                    button {
                        r#type: "button",
                        class: "ui-tree-expander",
                        tabindex: "-1",
                        "aria-expanded": branch.expanded,
                        "aria-label": branch.toggle_label(),
                        onclick: {
                            let ontoggle = branch.ontoggle;
                            let expanded = branch.expanded;
                            move |event: MouseEvent| {
                                event.stop_propagation();
                                ontoggle.call(!expanded);
                            }
                        },
                        ChevronIcon { open: branch.expanded }
                    }
                } else {
                    span { class: "ui-tree-indent", "aria-hidden": "true" }
                }
                button {
                    r#type: "button",
                    class: "ui-tree-label",
                    "aria-pressed": props.selected,
                    onclick: move |_| if let Some(handler) = onselect { handler.call(()) },
                    onkeydown: row_keys,
                    {props.label}
                }
                if let Some(trailing) = props.trailing {
                    {trailing}
                }
            }
            if expanded == Some(true) {
                ul { class: "ui-tree-group", role: "group", {props.children} }
            }
        }
    }
}
