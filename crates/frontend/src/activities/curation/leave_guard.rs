//! The browser's own "Leave site?" question, asked only while leaving would
//! lose work nothing else keeps: edits not yet saved, or a run still reading.
use dioxus::prelude::*;
use std::rc::Rc;
use wasm_bindgen::{JsCast as _, closure::Closure};

/// A window `beforeunload` listener owned by the component that installs it.
pub(super) struct LeaveGuard;

impl LeaveGuard {
    /// Ask before the page unloads whenever `would_lose` answers true at that
    /// moment. It is asked then, not tracked, so it reads without subscribing.
    pub(super) fn use_while(would_lose: impl Fn() -> bool + 'static) {
        let listener = use_hook(move || {
            let listener =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    if would_lose() {
                        event.prevent_default();
                        // Browsers that predate `preventDefault` here ask when the
                        // event carries a return value.
                        let _ = js_sys::Reflect::set(&event, &"returnValue".into(), &"".into());
                    }
                });
            if let Some(window) = web_sys::window() {
                let _ = window.add_event_listener_with_callback(
                    "beforeunload",
                    listener.as_ref().unchecked_ref(),
                );
            }
            Rc::new(listener)
        });
        use_drop(move || {
            if let Some(window) = web_sys::window() {
                let _ = window.remove_event_listener_with_callback(
                    "beforeunload",
                    listener.as_ref().as_ref().unchecked_ref(),
                );
            }
        });
    }
}
