//! A menu: a short list of actions opened from one control. It takes focus
//! when it opens, the arrows walk it, and focus goes back to the control that
//! opened it when Escape closes it.

use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::{JsCast as _, closure::Closure};

use crate::foundation::{AttributeClass, BrowserFocus, RovingGroup, RovingStep};

#[derive(Props, Clone, PartialEq)]
pub struct MenuProps {
    /// What the menu is for, as a screen reader announces it.
    label: String,
    /// The id of the control that opened the menu. Focus returns there on
    /// Escape, and pressing or focusing it leaves the menu open so the
    /// control's own press can close it.
    opener: ReadSignal<String>,
    /// Close the menu. Called on Escape, on a press anywhere but the menu or
    /// its opener, and when focus moves to another control.
    on_close: EventHandler<()>,
    /// The items: `Button`s with `role: "menuitem"`.
    children: Element,
    #[props(extends = GlobalAttributes, extends = div)]
    attributes: Vec<Attribute>,
}

#[component]
pub fn Menu(props: MenuProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-menu");
    let mut element = use_signal(|| None::<web_sys::Element>);
    let on_close = props.on_close;
    let opener = props.opener;
    OutsidePress::use_listener(element, opener, on_close);
    rsx! {
        div {
            class,
            role: "menu",
            "aria-label": "{props.label}",
            // Safari and Firefox do not focus a pressed button; they focus
            // its nearest focusable ancestor. Being that ancestor keeps a
            // press on an item inside the menu, so the item's click lands
            // rather than the menu closing under it.
            tabindex: "-1",
            onmounted: move |event: Event<MountedData>| {
                let mounted = event.data().downcast::<web_sys::Element>().cloned();
                if let Some(menu) = mounted.as_ref() {
                    MenuFocus::first(menu);
                }
                element.set(mounted);
            },
            onkeydown: move |event: KeyboardEvent| {
                let step = match event.key() {
                    Key::ArrowDown => RovingStep::Adjacent(1),
                    Key::ArrowUp => RovingStep::Adjacent(-1),
                    Key::Home => RovingStep::First,
                    Key::End => RovingStep::Last,
                    Key::Escape => {
                        event.prevent_default();
                        event.stop_propagation();
                        on_close.call(());
                        BrowserFocus::after_render(opener());
                        return;
                    }
                    _ => return,
                };
                event.prevent_default();
                BrowserFocus::rove(RovingGroup::MENU, step, false);
            },
            // Only focus moving onto another control closes the menu; a press
            // that leaves focus nowhere is `OutsidePress`'s to answer.
            onfocusout: move |event: FocusEvent| {
                let moved_to = event
                    .downcast::<web_sys::FocusEvent>()
                    .and_then(|raw| raw.related_target())
                    .and_then(|target| target.dyn_into::<web_sys::Node>().ok());
                let left = moved_to.is_some_and(|target| {
                    element
                        .peek()
                        .as_ref()
                        .is_some_and(|menu| !MenuFocus::holds(menu, &target, &opener.peek()))
                });
                if left {
                    on_close.call(());
                }
            },
            ..attributes,
            {props.children}
        }
    }
}

/// Where focus stands relative to one open menu.
struct MenuFocus;

impl MenuFocus {
    /// Focus the menu's first item without scrolling whatever holds the menu:
    /// a menu on a pan-and-zoom canvas would otherwise drag the canvas along.
    fn first(menu: &web_sys::Element) {
        use wasm_bindgen::JsCast as _;

        let Some(item) = menu
            .query_selector("[role='menuitem']:not([disabled])")
            .ok()
            .flatten()
            .and_then(|item| item.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let options = web_sys::FocusOptions::new();
        options.set_prevent_scroll(true);
        let _ = item.focus_with_options(&options);
    }

    /// Whether `target` is inside the menu or the control that opened it.
    fn holds(menu: &web_sys::Element, target: &web_sys::Node, opener: &str) -> bool {
        menu.contains(Some(target))
            || web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id(opener))
                .is_some_and(|opener| opener.contains(Some(target)))
    }
}

/// A press anywhere but the open menu or its opener closes the menu. It is
/// heard on the document before any handler below it, so a surface that
/// stops its own presses still closes the menu, and whatever was pressed
/// still gets its click.
struct OutsidePress;

impl OutsidePress {
    fn use_listener(
        menu: Signal<Option<web_sys::Element>>,
        opener: ReadSignal<String>,
        on_close: EventHandler<()>,
    ) {
        let listener = use_hook(move || {
            let listener = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(
                move |event: web_sys::PointerEvent| {
                    let Some(target) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                    else {
                        return;
                    };
                    let outside = menu
                        .peek()
                        .as_ref()
                        .is_some_and(|menu| !MenuFocus::holds(menu, &target, &opener.peek()));
                    if outside {
                        on_close.call(());
                    }
                },
            );
            if let Some(document) = web_sys::window().and_then(|window| window.document()) {
                let _ = document.add_event_listener_with_callback_and_bool(
                    "pointerdown",
                    listener.as_ref().unchecked_ref(),
                    true,
                );
            }
            Rc::new(listener)
        });
        use_drop(move || {
            if let Some(document) = web_sys::window().and_then(|window| window.document()) {
                let _ = document.remove_event_listener_with_callback_and_bool(
                    "pointerdown",
                    listener.as_ref().as_ref().unchecked_ref(),
                    true,
                );
            }
        });
    }
}
