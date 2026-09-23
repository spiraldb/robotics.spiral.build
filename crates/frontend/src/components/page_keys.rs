//! Keys pressed anywhere on the page: shortcuts that answer wherever focus was
//! last left, and stay quiet while the operator is typing.
use crate::components::timeline::KeyboardFocus;
use dioxus::prelude::*;
use std::rc::Rc;
use wasm_bindgen::{JsCast as _, closure::Closure};

/// A document-level keydown listener owned by the component that installs it.
pub(crate) struct PageKeys;

impl PageKeys {
    /// Install `handler` on the document for this component's lifetime. It
    /// returns whether it consumed the key, which prevents the browser's own
    /// response to it.
    pub(crate) fn use_listener(handler: impl FnMut(&web_sys::KeyboardEvent) -> bool + 'static) {
        let listener = use_hook(move || {
            let mut handler = handler;
            let listener = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |event: web_sys::KeyboardEvent| {
                    if handler(&event) {
                        event.prevent_default();
                    }
                },
            );
            if let Some(document) = web_sys::window().and_then(|window| window.document()) {
                let _ = document
                    .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
            }
            Rc::new(listener)
        });
        use_drop(move || {
            if let Some(document) = web_sys::window().and_then(|window| window.document()) {
                let _ = document.remove_event_listener_with_callback(
                    "keydown",
                    listener.as_ref().as_ref().unchecked_ref(),
                );
            }
        });
    }

    /// Whether the key landed on the page itself rather than inside a control.
    pub(crate) fn on_page(event: &web_sys::KeyboardEvent) -> bool {
        event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|element| matches!(element.tag_name().as_str(), "BODY" | "HTML"))
    }

    /// The modifier a shortcut names on this platform: ⌘ on a Mac, Ctrl
    /// elsewhere.
    pub(crate) fn command_symbol() -> &'static str {
        let mac = web_sys::window()
            .and_then(|window| window.navigator().platform().ok())
            .is_some_and(|platform| platform.starts_with("Mac"));
        if mac { "⌘" } else { "Ctrl " }
    }

    /// A plain key press — no modifier, not typed into a field — that a
    /// shortcut may claim.
    pub(crate) fn is_shortcut(event: &web_sys::KeyboardEvent) -> bool {
        !(event.ctrl_key() || event.meta_key() || event.alt_key() || event.repeat())
            && !KeyboardFocus::editing()
    }
}
