//! Focus movement: after a render, inside a dialog, and around a group.
//!
//! One owner for every place the interface moves focus itself, so a dialog,
//! a mode group, a tree, and a confirmation all reach the DOM the same way.

use dioxus::prelude::*;

/// Where a roving arrow key looks for its neighbours: the nearest container
/// around the focused element, and the members inside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RovingGroup {
    container: &'static str,
    members: &'static str,
}

impl RovingGroup {
    /// The mutually exclusive modes of a [`ModeGroup`](crate::ModeGroup).
    pub(crate) const MODES: Self = Self {
        container: "[role='radiogroup']",
        members: "[role='radio']:not([disabled])",
    };
    /// The visible rows of a [`Tree`](crate::Tree).
    pub(crate) const TREE: Self = Self {
        container: "[role='tree']",
        members: ".ui-tree-label:not([disabled])",
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RovingStep {
    /// One member forward or back, wrapping at either end.
    Adjacent(isize),
    First,
    Last,
}

/// The one owner of focus movement.
pub struct BrowserFocus;

impl BrowserFocus {
    pub(crate) fn dismiss_confirmation(on_cancel: EventHandler<()>, return_focus_id: &str) {
        on_cancel.call(());
        Self::after_render(return_focus_id.to_owned());
    }

    /// Move focus after Dioxus has committed a replacement subtree.
    pub fn after_render(id: String) {
        dioxus::dioxus_core::spawn_forever(async move {
            for attempt in 0..16 {
                let delay_ms = if attempt == 0 { 0 } else { 16 };
                gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                if Self::element(&id) {
                    break;
                }
            }
        });
    }

    fn element(id: &str) -> bool {
        use wasm_bindgen::JsCast as _;

        let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(id))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return false;
        };
        element.focus().is_ok()
    }

    /// Move focus among a group's members, found from the active element, and
    /// press the member reached when `activate`: a mode group changes mode as
    /// the arrow moves, a tree only moves its focus.
    pub(crate) fn rove(group: RovingGroup, step: RovingStep, activate: bool) {
        use wasm_bindgen::JsCast as _;

        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(active) = document.active_element() else {
            return;
        };
        let Ok(Some(container)) = active.closest(group.container) else {
            return;
        };
        let Ok(nodes) = container.query_selector_all(group.members) else {
            return;
        };
        let members = (0..nodes.length())
            .filter_map(|index| nodes.item(index))
            .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
            .collect::<Vec<_>>();
        if members.is_empty() {
            return;
        }
        let current = members
            .iter()
            .position(|member| active.is_same_node(Some(member.as_ref())));
        let next = match step {
            RovingStep::Adjacent(direction) => {
                let current = current.unwrap_or_default() as isize;
                (current + direction).rem_euclid(members.len() as isize) as usize
            }
            RovingStep::First => 0,
            RovingStep::Last => members.len() - 1,
        };
        if let Some(member) = members.get(next) {
            let _ = member.focus();
            if activate {
                member.click();
            }
        }
    }

    /// From a tree row, focus the row of the branch that contains it.
    pub(crate) fn focus_parent_tree_item() {
        use wasm_bindgen::JsCast as _;

        let Some(active) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
        else {
            return;
        };
        let parent = active
            .closest("[role='treeitem']")
            .ok()
            .flatten()
            .and_then(|item| item.parent_element())
            .and_then(|group| group.closest("[role='treeitem']").ok().flatten())
            .and_then(|parent| parent.query_selector(".ui-tree-label").ok().flatten())
            .and_then(|label| label.dyn_into::<web_sys::HtmlElement>().ok());
        if let Some(parent) = parent {
            let _ = parent.focus();
        }
    }

    pub(crate) fn trap_within(container_id: &str, backwards: bool) -> bool {
        use wasm_bindgen::JsCast as _;

        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return false;
        };
        let Some(container) = document.get_element_by_id(container_id) else {
            return false;
        };
        let Ok(nodes) = container.query_selector_all(
            "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
        ) else {
            return false;
        };
        let focusable = (0..nodes.length())
            .filter_map(|index| nodes.item(index))
            .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
            .collect::<Vec<_>>();
        if focusable.is_empty() {
            return false;
        }
        let current = document.active_element().and_then(|active| {
            focusable
                .iter()
                .position(|item| active.is_same_node(Some(item.as_ref())))
        });
        let next = if backwards {
            current
                .and_then(|index| index.checked_sub(1))
                .unwrap_or_else(|| focusable.len() - 1)
        } else {
            current.map_or(0, |index| (index + 1) % focusable.len())
        };
        focusable
            .get(next)
            .is_some_and(|element| element.focus().is_ok())
    }
}
