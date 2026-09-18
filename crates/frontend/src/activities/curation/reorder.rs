//! A rule drag previews spring positions; only dropping edits the recipe.
use dioxus::prelude::*;
use spiral_robotics_common::curation::StepDirection;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::state::CurationController;

#[derive(Clone)]
struct MovingRule {
    id: String,
    base: f64,
    height: f64,
    position: f64,
    velocity: f64,
    target: f64,
}

struct PointerGrab {
    id: i32,
    offset: f64,
    samples: Vec<(f64, f64)>,
}

struct RuleGesture {
    id: String,
    original: Vec<String>,
    destination: usize,
    pointer: Option<PointerGrab>,
}

#[derive(Default)]
struct AnimationClock {
    running: bool,
    previous: Option<f64>,
}

#[derive(Default)]
struct ReorderState {
    rules: Vec<MovingRule>,
    gesture: Option<RuleGesture>,
    animation: AnimationClock,
    announcement: String,
    marker: Option<f64>,
    reduced_motion: bool,
}

#[derive(Clone, Copy)]
pub(super) struct RuleReorder {
    state: Signal<ReorderState>,
    controller: CurationController,
}

impl RuleReorder {
    pub(super) fn use_binding(controller: CurationController) -> Self {
        let owner = Self {
            state: use_signal(ReorderState::default),
            controller,
        };
        let ids = controller
            .spec()
            .steps()
            .iter()
            .map(|step| step.id().to_owned())
            .collect::<Vec<_>>();
        use_effect(use_reactive((&ids,), move |(ids,)| owner.synchronize(&ids)));
        owner
    }
    fn element(id: &str, handle: bool) -> Option<web_sys::HtmlElement> {
        let prefix = if handle {
            "curation-rule-handle"
        } else {
            "curation-rule"
        };
        web_sys::window()?
            .document()?
            .get_element_by_id(&format!("{prefix}-{id}"))?
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
        let previous = self.state.peek().rules.clone();
        let rules = ids
            .iter()
            .filter_map(|id| {
                let rect = Self::element(id, false)?.get_bounding_client_rect();
                let old = previous.iter().find(|rule| &rule.id == id);
                let position = old.map_or(0.0, |rule| rule.position);
                let base = rect.top() - position;
                Some(MovingRule {
                    id: id.clone(),
                    base,
                    height: rect.height(),
                    position: old.map_or(0.0, |rule| rule.position + rule.base - base),
                    velocity: old.map_or(0.0, |rule| rule.velocity),
                    target: 0.0,
                })
            })
            .collect();
        self.state.write().rules = rules;
        self.animate();
    }
    pub(super) fn style(self, id: &str) -> String {
        self.state
            .read()
            .rules
            .iter()
            .find(|rule| rule.id == id)
            .map_or_else(String::new, |rule| {
                format!("transform:translateY({}px)", rule.position)
            })
    }
    pub(super) fn picked(self, id: &str) -> bool {
        self.state
            .read()
            .gesture
            .as_ref()
            .is_some_and(|gesture| gesture.id == id)
    }
    pub(super) fn announcement(self) -> String {
        self.state.read().announcement.clone()
    }
    pub(super) fn marker_style(self) -> String {
        self.state.read().marker.map_or_else(
            || "display:none".into(),
            |top| format!("display:block;top:{top}px"),
        )
    }
    pub(super) fn pointer_down(self, id: &str, event: Event<PointerData>) {
        let Some(raw) = event.downcast::<web_sys::PointerEvent>() else {
            return;
        };
        if raw.button() != 0 {
            return;
        }
        event.prevent_default();
        if let Some(handle) = Self::element(id, true) {
            let _ = handle.focus();
            let _ = handle.set_pointer_capture(raw.pointer_id());
        }
        let rect = Self::element(id, false).map(|element| element.get_bounding_client_rect());
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
        let ids = self
            .controller
            .spec()
            .steps()
            .iter()
            .map(|step| step.id().to_owned())
            .collect::<Vec<_>>();
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
                "Picked up rule {} of {}. Use arrow keys to move, Space to drop, Escape to cancel.",
                index + 1,
                ids.len()
            );
            state.gesture = Some(RuleGesture {
                id: id.to_owned(),
                original: ids,
                destination: index,
                pointer,
            });
        }
        self.preview(index);
    }
    pub(super) fn pointer_move(mut self, event: Event<PointerData>) {
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
            let Some(rule) = state.rules.iter_mut().find(|rule| rule.id == id) else {
                return;
            };
            rule.position = y - offset - rule.base;
            rule.velocity = velocity;
            let center = rule.base + rule.position + rule.height / 2.0;
            state
                .rules
                .iter()
                .filter(|rule| rule.id != id && center > rule.base + rule.height / 2.0)
                .count()
        };
        self.preview(destination);
    }
    pub(super) fn pointer_up(self, event: Event<PointerData>) {
        if self.matches_pointer(&event) {
            self.finish(true);
        }
    }
    pub(super) fn pointer_cancel(self, event: Event<PointerData>) {
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
    fn preview(mut self, destination: usize) {
        let root_top = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("curation-ordered-rules"))
            .map_or(0.0, |root| root.get_bounding_client_rect().top());
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
            let Some(first) = state.rules.first() else {
                return;
            };
            let mut top = first.base;
            let gap = state.rules.get(1).map_or(0.0, |second| {
                (second.base - first.base - first.height).max(0.0)
            });
            for candidate in order {
                if let Some(rule) = state.rules.iter_mut().find(|rule| rule.id == candidate) {
                    rule.target = top - rule.base;
                    let height = rule.height;
                    if candidate == id {
                        state.marker = Some(top - root_top - gap / 2.0);
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
                state.rules.len()
            );
        }
        self.animate();
    }
    pub(super) fn keyboard(self, id: &str, event: Event<KeyboardData>) {
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
                let destination = match event.key() {
                    Key::Home => 0,
                    Key::End => gesture.original.len().saturating_sub(1),
                    Key::ArrowUp => gesture.destination.saturating_sub(1),
                    _ => (gesture.destination + 1).min(gesture.original.len().saturating_sub(1)),
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
                for rule in &mut state.rules {
                    rule.target = 0.0;
                }
            }
            state.announcement = if commit {
                format!("Dropped at position {}.", gesture.destination + 1)
            } else {
                "Reorder cancelled.".into()
            };
        }
        if let Some(pointer) = &gesture.pointer {
            if let Some(handle) = Self::element(&gesture.id, true) {
                let _ = handle.release_pointer_capture(pointer.id);
            }
        }
        let origin = gesture
            .original
            .iter()
            .position(|id| id == &gesture.id)
            .unwrap_or(gesture.destination);
        if commit && origin != gesture.destination {
            let id = gesture.id.clone();
            self.controller.edit(move |spec| {
                let Some(start) = spec.steps().iter().position(|step| step.id() == id) else {
                    return;
                };
                let destination = gesture
                    .destination
                    .min(spec.steps().len().saturating_sub(1));
                let direction = if destination < start {
                    StepDirection::Earlier
                } else {
                    StepDirection::Later
                };
                for _ in 0..start.abs_diff(destination) {
                    spec.move_step(&id, direction);
                }
            });
            spawn(async move {
                Self::next_frame().await;
                if let Some(handle) = Self::element(&gesture.id, true) {
                    let _ = handle.focus();
                }
            });
        } else {
            for rule in &mut self.state.write().rules {
                rule.target = 0.0;
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
        for rule in &mut state.rules {
            if held.as_ref() == Some(&rule.id) {
                continue;
            }
            if reduced {
                rule.position = rule.target;
                rule.velocity = 0.0;
            } else {
                for _ in 0..4 {
                    rule.velocity +=
                        (625.0 * (rule.target - rule.position) - 50.0 * rule.velocity) * dt / 4.0;
                    rule.position += rule.velocity * dt / 4.0;
                }
            }
            if (rule.target - rule.position).abs() < 0.1 && rule.velocity.abs() < 0.5 {
                rule.position = rule.target;
                rule.velocity = 0.0;
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
