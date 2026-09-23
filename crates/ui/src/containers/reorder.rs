//! Reordering a vertical list: a handle the pointer drags or the keyboard
//! picks up, neighbours that spring aside to preview the new order, an
//! insertion marker, and a live announcement. Only dropping changes the
//! caller's order, through one [`ReorderMove`].

use dioxus::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::foundation::{AttributeClass, GripIcon};

#[derive(Clone)]
struct MovingItem {
    id: String,
    /// Where the item rests when nothing is moving.
    base: f64,
    height: f64,
    /// The spring's current offset from `base`.
    position: f64,
    velocity: f64,
    target: f64,
}

struct PointerGrab {
    id: i32,
    /// Pointer offset within the grabbed item, so it does not jump to the cursor.
    offset: f64,
    /// Recent (y, timestamp) pairs for the release velocity.
    samples: Vec<(f64, f64)>,
}

struct Gesture {
    id: String,
    original: Vec<String>,
    destination: usize,
    /// `None` while the keyboard, not the pointer, holds the item.
    pointer: Option<PointerGrab>,
}

#[derive(Default)]
struct AnimationClock {
    running: bool,
    previous: Option<f64>,
}

#[derive(Default)]
struct ReorderState {
    order: Vec<String>,
    items: Vec<MovingItem>,
    gesture: Option<Gesture>,
    animation: AnimationClock,
    announcement: String,
    /// The insertion line's top, relative to the list, while an item is held.
    marker: Option<f64>,
    reduced_motion: bool,
}

/// One committed move: the item now belongs at `to` in the caller's order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorderMove {
    id: String,
    to: usize,
}

impl ReorderMove {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn to(&self) -> usize {
        self.to
    }
}

/// The gesture bound to one list: element naming, spring state, and the
/// handler that commits a drop.
#[derive(Clone, Copy, PartialEq)]
pub struct Reorder {
    prefix: &'static str,
    state: Signal<ReorderState>,
    on_move: EventHandler<ReorderMove>,
}

impl Reorder {
    /// Bind to the list whose items are `order`, in display order. Call once
    /// in the list owner's render like any hook; the binding re-measures
    /// whenever `order` changes and drops a gesture the order changed under.
    pub fn use_binding(
        prefix: &'static str,
        order: Vec<String>,
        on_move: EventHandler<ReorderMove>,
    ) -> Self {
        let binding = Self {
            prefix,
            state: use_signal(ReorderState::default),
            on_move,
        };
        use_effect(use_reactive((&order,), move |(order,)| {
            binding.synchronize(&order)
        }));
        binding
    }

    /// The id of the list element; the caller renders it through [`ReorderList`].
    fn list_id(self) -> String {
        self.prefix.to_owned()
    }
    fn help_id(self) -> String {
        format!("{}-help", self.prefix)
    }
    /// The id the caller must give the element that moves for `id`.
    pub fn item_id(self, id: &str) -> String {
        format!("{}-item-{id}", self.prefix)
    }
    fn handle_id(self, id: &str) -> String {
        format!("{}-handle-{id}", self.prefix)
    }

    fn element(self, id: &str, handle: bool) -> Option<web_sys::HtmlElement> {
        let element_id = if handle {
            self.handle_id(id)
        } else {
            self.item_id(id)
        };
        web_sys::window()?
            .document()?
            .get_element_by_id(&element_id)?
            .dyn_into()
            .ok()
    }

    fn synchronize(mut self, ids: &[String]) {
        let changed_during_drag = self
            .state
            .peek()
            .gesture
            .as_ref()
            .is_some_and(|gesture| gesture.original != ids);
        if changed_during_drag {
            self.finish(false);
        }
        let previous = self.state.peek().items.clone();
        let items = ids
            .iter()
            .filter_map(|id| {
                let rect = self.element(id, false)?.get_bounding_client_rect();
                let old = previous.iter().find(|item| &item.id == id);
                let position = old.map_or(0.0, |item| item.position);
                let base = rect.top() - position;
                Some(MovingItem {
                    id: id.clone(),
                    base,
                    height: rect.height(),
                    position: old.map_or(0.0, |item| item.position + item.base - base),
                    velocity: old.map_or(0.0, |item| item.velocity),
                    target: 0.0,
                })
            })
            .collect();
        {
            let mut state = self.state.write();
            state.order = ids.to_vec();
            state.items = items;
        }
        self.animate();
    }

    /// The inline style that moves the item for `id` to its previewed place.
    pub fn style(self, id: &str) -> String {
        self.state
            .read()
            .items
            .iter()
            .find(|item| item.id == id)
            .map_or_else(String::new, |item| {
                format!("transform:translateY({}px)", item.position)
            })
    }

    /// Whether the item for `id` is the one being held.
    pub fn picked(self, id: &str) -> bool {
        self.state
            .read()
            .gesture
            .as_ref()
            .is_some_and(|gesture| gesture.id == id)
    }

    fn announcement(self) -> String {
        self.state.read().announcement.clone()
    }

    fn marker_style(self) -> String {
        self.state.read().marker.map_or_else(
            || "display:none".into(),
            |top| format!("display:block;top:{top}px"),
        )
    }

    fn pointer_down(self, id: &str, event: Event<PointerData>) {
        let Some(raw) = event.downcast::<web_sys::PointerEvent>() else {
            return;
        };
        if raw.button() != 0 {
            return;
        }
        event.prevent_default();
        if let Some(handle) = self.element(id, true) {
            let _ = handle.focus();
            let _ = handle.set_pointer_capture(raw.pointer_id());
        }
        let rect = self
            .element(id, false)
            .map(|element| element.get_bounding_client_rect());
        let Some(rect) = rect else {
            return;
        };
        self.begin(
            id,
            Some(PointerGrab {
                id: raw.pointer_id(),
                offset: raw.client_y() as f64 - rect.top(),
                samples: vec![(raw.client_y() as f64, raw.time_stamp())],
            }),
        );
    }

    fn begin(mut self, id: &str, pointer: Option<PointerGrab>) {
        if self.state.peek().gesture.is_some() {
            self.finish(false);
        }
        let ids = self.state.peek().order.clone();
        self.synchronize(&ids);
        let Some(index) = ids.iter().position(|candidate| candidate == id) else {
            return;
        };
        let reduced = web_sys::window()
            .and_then(|window| {
                window
                    .match_media("(prefers-reduced-motion: reduce)")
                    .ok()
                    .flatten()
            })
            .is_some_and(|query| query.matches());
        {
            let mut state = self.state.write();
            state.reduced_motion = reduced;
            state.announcement = format!(
                "Picked up item {} of {}. Use arrow keys to move, Space to drop, Escape to cancel.",
                index + 1,
                ids.len()
            );
            state.gesture = Some(Gesture {
                id: id.to_owned(),
                original: ids,
                destination: index,
                pointer,
            });
        }
        self.preview(index);
    }

    fn pointer_move(mut self, event: Event<PointerData>) {
        let Some(raw) = event.downcast::<web_sys::PointerEvent>() else {
            return;
        };
        let destination = {
            let mut state = self.state.write();
            let Some(gesture) = state.gesture.as_mut() else {
                return;
            };
            let Some(pointer) = gesture
                .pointer
                .as_mut()
                .filter(|pointer| pointer.id == raw.pointer_id())
            else {
                return;
            };
            let y = raw.client_y() as f64;
            let time = raw.time_stamp();
            pointer.samples.push((y, time));
            pointer
                .samples
                .retain(|(_, timestamp)| time - timestamp <= 100.0);
            let (first_y, first_time) = pointer.samples[0];
            let velocity = if time > first_time {
                (y - first_y) * 1000.0 / (time - first_time)
            } else {
                0.0
            };
            let id = gesture.id.clone();
            let offset = pointer.offset;
            let Some(item) = state.items.iter_mut().find(|item| item.id == id) else {
                return;
            };
            item.position = y - offset - item.base;
            item.velocity = velocity;
            let center = item.base + item.position + item.height / 2.0;
            state
                .items
                .iter()
                .filter(|item| item.id != id && center > item.base + item.height / 2.0)
                .count()
        };
        self.preview(destination);
    }

    fn pointer_up(self, event: Event<PointerData>) {
        if self.matches_pointer(&event) {
            self.finish(true);
        }
    }

    fn pointer_cancel(self, event: Event<PointerData>) {
        if self.matches_pointer(&event) {
            self.finish(false);
        }
    }

    fn matches_pointer(self, event: &Event<PointerData>) -> bool {
        event
            .downcast::<web_sys::PointerEvent>()
            .is_some_and(|raw| {
                self.state
                    .peek()
                    .gesture
                    .as_ref()
                    .and_then(|gesture| gesture.pointer.as_ref())
                    .is_some_and(|pointer| pointer.id == raw.pointer_id())
            })
    }

    /// Lay the items out as if the held one were dropped at `destination`.
    fn preview(mut self, destination: usize) {
        let list_top = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(&self.list_id()))
            .map_or(0.0, |list| list.get_bounding_client_rect().top());
        {
            let mut state = self.state.write();
            let Some(gesture) = state.gesture.as_ref() else {
                return;
            };
            let id = gesture.id.clone();
            let mut order = gesture
                .original
                .iter()
                .filter(|candidate| **candidate != id)
                .cloned()
                .collect::<Vec<_>>();
            let destination = destination.min(order.len());
            order.insert(destination, id.clone());
            let Some(first) = state.items.first() else {
                return;
            };
            let mut top = first.base;
            let gap = state.items.get(1).map_or(0.0, |second| {
                (second.base - first.base - first.height).max(0.0)
            });
            for candidate in order {
                if let Some(item) = state.items.iter_mut().find(|item| item.id == candidate) {
                    item.target = top - item.base;
                    let height = item.height;
                    if candidate == id {
                        state.marker = Some(top - list_top - gap / 2.0);
                    }
                    top += height + gap;
                }
            }
            if let Some(gesture) = state.gesture.as_mut() {
                gesture.destination = destination;
            }
            state.announcement = format!(
                "Position {} of {}. Space to drop, Escape to cancel.",
                destination + 1,
                state.items.len()
            );
        }
        self.animate();
    }

    fn keyboard(self, id: &str, event: Event<KeyboardData>) {
        let picked = self.picked(id);
        match event.key() {
            Key::Escape if picked => {
                event.prevent_default();
                event.stop_propagation();
                self.finish(false);
            }
            Key::Enter => {
                event.prevent_default();
                if picked {
                    self.finish(true);
                } else {
                    self.begin(id, None);
                }
            }
            Key::Character(value) if value == " " => {
                event.prevent_default();
                if picked {
                    self.finish(true);
                } else {
                    self.begin(id, None);
                }
            }
            Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End if picked => {
                event.prevent_default();
                let state = self.state.peek();
                let Some(gesture) = state.gesture.as_ref() else {
                    return;
                };
                let last = gesture.original.len().saturating_sub(1);
                let destination = match event.key() {
                    Key::Home => 0,
                    Key::End => last,
                    Key::ArrowUp => gesture.destination.saturating_sub(1),
                    _ => (gesture.destination + 1).min(last),
                };
                drop(state);
                self.preview(destination);
            }
            Key::Tab if picked => self.finish(false),
            _ => {}
        }
    }

    fn finish(mut self, commit: bool) {
        let Some(gesture) = self.state.write().gesture.take() else {
            return;
        };
        {
            let mut state = self.state.write();
            state.marker = None;
            if !commit {
                for item in &mut state.items {
                    item.target = 0.0;
                }
            }
            state.announcement = if commit {
                format!("Dropped at position {}.", gesture.destination + 1)
            } else {
                "Reorder cancelled.".into()
            };
        }
        if let Some(pointer) = &gesture.pointer
            && let Some(handle) = self.element(&gesture.id, true)
        {
            let _ = handle.release_pointer_capture(pointer.id);
        }
        let origin = gesture
            .original
            .iter()
            .position(|id| id == &gesture.id)
            .unwrap_or(gesture.destination);
        if commit && origin != gesture.destination {
            self.on_move.call(ReorderMove {
                id: gesture.id.clone(),
                to: gesture.destination,
            });
            spawn(async move {
                Self::next_frame().await;
                if let Some(handle) = self.element(&gesture.id, true) {
                    let _ = handle.focus();
                }
            });
        } else {
            for item in &mut self.state.write().items {
                item.target = 0.0;
            }
        }
        self.animate();
    }

    fn animate(mut self) {
        if self.state.peek().animation.running {
            return;
        }
        self.state.write().animation = AnimationClock {
            running: true,
            previous: None,
        };
        spawn(async move {
            while let Some(time) = Self::next_frame().await {
                if !self.advance(time) {
                    break;
                }
            }
            self.state.write().animation.running = false;
        });
    }

    /// One spring step; `true` while anything is still settling.
    fn advance(mut self, time: f64) -> bool {
        let mut state = self.state.write();
        let dt = state.animation.previous.map_or(0.016, |previous| {
            ((time - previous) / 1000.0).clamp(0.0, 0.032)
        });
        state.animation.previous = Some(time);
        let held = state
            .gesture
            .as_ref()
            .filter(|gesture| gesture.pointer.is_some())
            .map(|gesture| gesture.id.clone());
        let reduced = state.reduced_motion;
        let mut unsettled = false;
        for item in &mut state.items {
            if held.as_ref() == Some(&item.id) {
                continue;
            }
            if reduced {
                item.position = item.target;
                item.velocity = 0.0;
            } else {
                for _ in 0..4 {
                    item.velocity +=
                        (625.0 * (item.target - item.position) - 50.0 * item.velocity) * dt / 4.0;
                    item.position += item.velocity * dt / 4.0;
                }
            }
            if (item.target - item.position).abs() < 0.1 && item.velocity.abs() < 0.5 {
                item.position = item.target;
                item.velocity = 0.0;
            } else {
                unsettled = true;
            }
        }
        unsettled
    }

    async fn next_frame() -> Option<f64> {
        let window = web_sys::window()?;
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            if let Err(error) = window.request_animation_frame(&resolve) {
                let _ = reject.call1(&JsValue::NULL, &error);
            }
        });
        JsFuture::from(promise).await.ok()?.as_f64()
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ReorderListProps {
    reorder: Reorder,
    label: String,
    /// The items, each carrying `reorder.item_id(..)`, `reorder.style(..)`,
    /// and a [`ReorderHandle`].
    children: Element,
    #[props(extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
}

/// The list the items live in. It carries the insertion marker, the keyboard
/// help, and the live announcement; the caller renders the items.
#[component]
pub fn ReorderList(props: ReorderListProps) -> Element {
    let reorder = props.reorder;
    let (class, attributes) = props.attributes.with_base_class("ui-reorder-list");
    rsx! {
        section { id: reorder.list_id(), class, "aria-label": props.label, ..attributes,
            div { class: "ui-reorder-marker", style: reorder.marker_style(), "aria-hidden": "true" }
            p { id: reorder.help_id(), class: "ui-sr-only",
                "Drag to reorder. Or press Space, then arrow keys to move, Space to drop, and Escape to cancel."
            }
            div { class: "ui-sr-only", "aria-live": "polite", "{reorder.announcement()}" }
            {props.children}
        }
    }
}

/// The grip an item is picked up by. `label` names the item for assistive
/// technology, such as "Reorder step 2".
#[component]
pub fn ReorderHandle(reorder: Reorder, id: String, label: String) -> Element {
    let picked = reorder.picked(&id);
    let pressed_id = id.clone();
    let keyed_id = id.clone();
    rsx! {
        button {
            r#type: "button",
            class: "ui-reorder-handle",
            id: reorder.handle_id(&id),
            "aria-label": label,
            "aria-describedby": reorder.help_id(),
            "aria-pressed": picked,
            title: "Drag to reorder · Space then arrow keys",
            onpointerdown: move |event| reorder.pointer_down(&pressed_id, event),
            onpointermove: move |event| reorder.pointer_move(event),
            onpointerup: move |event| reorder.pointer_up(event),
            onpointercancel: move |event| reorder.pointer_cancel(event),
            onlostpointercapture: move |event| reorder.pointer_cancel(event),
            onkeydown: move |event| reorder.keyboard(&keyed_id, event),
            GripIcon {}
        }
    }
}
