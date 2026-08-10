//! Persisted editor-group layout for the Studio visualization region.
//!
//! The workbench/visualization boundary is owned by `studio_split`; this module
//! owns the structure inside visualization. Scene and camera groups can be
//! reordered from their tabs, adjacent groups can be resized, and the Timeline
//! has its own horizontal splitter. Content is keyed by its durable panel id so
//! moving a group does not reset its camera or WebGL state.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "web")]
use wasm_bindgen::JsCast as _;

const LAYOUT_VERSION: u8 = 1;
const DEFAULT_UPPER_RATIO: f64 = 0.58;
const MIN_UPPER_RATIO: f64 = 0.28;
const MAX_UPPER_RATIO: f64 = 0.82;
const MIN_PANEL_RATIO: f64 = 0.14;
const RESIZE_KEYBOARD_STEP: f64 = 0.03;

#[derive(Clone, PartialEq)]
pub(crate) struct DockPanel {
    pub id: String,
    pub label: String,
    pub class: &'static str,
    pub preferred_weight: f64,
    pub content: Element,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct DockLayout {
    version: u8,
    upper_ratio: f64,
    order: Vec<String>,
    weights: BTreeMap<String, f64>,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            upper_ratio: DEFAULT_UPPER_RATIO,
            order: Vec::new(),
            weights: BTreeMap::new(),
        }
    }
}

impl DockLayout {
    fn valid(&self) -> bool {
        self.version == LAYOUT_VERSION
            && self.upper_ratio.is_finite()
            && (MIN_UPPER_RATIO..=MAX_UPPER_RATIO).contains(&self.upper_ratio)
            && self
                .weights
                .values()
                .all(|weight| weight.is_finite() && *weight > 0.0)
    }

    fn active_order(&self, panels: &[(String, f64)]) -> Vec<String> {
        let known = panels.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>();
        let mut order = self
            .order
            .iter()
            .filter(|id| known.iter().any(|known| *known == id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for (panel_id, _) in panels {
            if !order.iter().any(|id| id == panel_id) {
                order.push(panel_id.clone());
            }
        }
        order
    }

    fn active_weights(&self, order: &[String], panels: &[(String, f64)]) -> Vec<f64> {
        let mut weights = order
            .iter()
            .map(|id| {
                self.weights.get(id).copied().unwrap_or_else(|| {
                    panels
                        .iter()
                        .find(|(panel_id, _)| panel_id == id)
                        .map(|(_, preferred_weight)| *preferred_weight)
                        .unwrap_or(1.0)
                })
            })
            .collect::<Vec<_>>();
        normalize(&mut weights);
        weights
    }

    fn reconcile(&mut self, panels: &[(String, f64)]) {
        let order = self.active_order(panels);
        let weights = self.active_weights(&order, panels);
        self.order = order.clone();
        self.weights = order.into_iter().zip(weights).collect();
    }

    fn move_panel(&mut self, panels: &[(String, f64)], dragged: &str, target: &str) -> bool {
        self.reconcile(panels);
        let Some(from) = self.order.iter().position(|id| id == dragged) else {
            return false;
        };
        let Some(to) = self.order.iter().position(|id| id == target) else {
            return false;
        };
        if from == to {
            return false;
        }
        let id = self.order.remove(from);
        self.order.insert(to, id);
        true
    }

    fn move_panel_by(&mut self, panels: &[(String, f64)], id: &str, delta: isize) -> bool {
        self.reconcile(panels);
        let Some(from) = self.order.iter().position(|candidate| candidate == id) else {
            return false;
        };
        let to = (from as isize + delta).clamp(0, self.order.len() as isize - 1) as usize;
        if from == to {
            return false;
        }
        self.order.swap(from, to);
        true
    }

    fn resize_pair(
        &mut self,
        panels: &[(String, f64)],
        left: &str,
        right: &str,
        delta: f64,
    ) -> bool {
        self.reconcile(panels);
        let Some(left_weight) = self.weights.get(left).copied() else {
            return false;
        };
        let Some(right_weight) = self.weights.get(right).copied() else {
            return false;
        };
        let pair = left_weight + right_weight;
        let minimum = MIN_PANEL_RATIO.min(pair / 2.0);
        let next_left = (left_weight + delta).clamp(minimum, pair - minimum);
        if (next_left - left_weight).abs() <= f64::EPSILON {
            return false;
        }
        self.weights.insert(left.to_owned(), next_left);
        self.weights.insert(right.to_owned(), pair - next_left);
        true
    }

    fn set_upper_ratio(&mut self, ratio: f64) -> bool {
        if !ratio.is_finite() {
            return false;
        }
        let next = ratio.clamp(MIN_UPPER_RATIO, MAX_UPPER_RATIO);
        if (next - self.upper_ratio).abs() <= f64::EPSILON {
            return false;
        }
        self.upper_ratio = next;
        true
    }

    #[cfg(feature = "web")]
    fn stored(scope: &str) -> Option<Self> {
        let value = web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(&storage_key(scope))
            .ok()??;
        serde_json::from_str::<Self>(&value)
            .ok()
            .filter(Self::valid)
    }

    #[cfg(not(feature = "web"))]
    fn stored(_scope: &str) -> Option<Self> {
        None
    }

    #[cfg(feature = "web")]
    fn store(&self, scope: &str) {
        if !self.valid() {
            return;
        }
        let Ok(value) = serde_json::to_string(self) else {
            return;
        };
        if let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok()?) {
            let _ = storage.set_item(&storage_key(scope), &value);
        }
    }

    #[cfg(not(feature = "web"))]
    fn store(&self, _scope: &str) {}
}

#[derive(Clone, Debug, PartialEq)]
struct PanelResize {
    pointer_origin: f64,
    left: String,
    right: String,
    start_layout: DockLayout,
    span: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct TimelineResize {
    pointer_origin: f64,
    start_ratio: f64,
    span: f64,
}

#[component]
pub(crate) fn StudioDock(
    scope: String,
    panels: Vec<DockPanel>,
    timeline: Element,
    on_resize: EventHandler<()>,
    on_activate: EventHandler<String>,
) -> Element {
    let initial_scope = scope.clone();
    let mut layout = use_signal(move || DockLayout::stored(&initial_scope).unwrap_or_default());
    let mut dragging_panel = use_signal(|| None::<String>);
    let mut panel_resize = use_signal(|| None::<PanelResize>);
    let mut timeline_resize = use_signal(|| None::<TimelineResize>);
    let panel_specs = panels
        .iter()
        .map(|panel| (panel.id.clone(), panel.preferred_weight))
        .collect::<Vec<_>>();
    let order = layout().active_order(&panel_specs);
    let weights = layout().active_weights(&order, &panel_specs);
    let upper_style = format!(
        "grid-template-rows: minmax(0, {:.5}%) 0.5rem minmax(0, {:.5}%);",
        layout().upper_ratio * 100.0,
        (1.0 - layout().upper_ratio) * 100.0,
    );
    let dock_id = format!("studio-dock-{}", safe_id(&scope));
    let deck_id = format!("{dock_id}-deck");
    let move_specs = panel_specs.clone();
    let up_specs = panel_specs.clone();
    let up_scope = scope.clone();
    let timeline_dock_id = dock_id.clone();
    let reset_scope = scope.clone();
    let keyboard_scope = scope.clone();

    rsx! {
        section {
            id: "{dock_id}",
            class: "studio-editor-dock",
            style: "{upper_style}",
            onpointermove: move |event| {
                if let Some(active) = panel_resize() {
                    let delta = (event.data().client_coordinates().x - active.pointer_origin) / active.span;
                    let mut next = active.start_layout.clone();
                    if next.resize_pair(&move_specs, &active.left, &active.right, delta) {
                        layout.set(next);
                        on_resize.call(());
                    }
                } else if let Some(active) = timeline_resize() {
                    let delta = (event.data().client_coordinates().y - active.pointer_origin) / active.span;
                    if layout.write().set_upper_ratio(active.start_ratio + delta) {
                        on_resize.call(());
                    }
                }
            },
            onpointerup: move |_| {
                if panel_resize().is_some() || timeline_resize().is_some() {
                    panel_resize.set(None);
                    timeline_resize.set(None);
                    let mut saved = layout();
                    saved.reconcile(&up_specs);
                    saved.store(&up_scope);
                    layout.set(saved);
                    on_resize.call(());
                }
            },
            onpointercancel: move |_| {
                if let Some(active) = panel_resize() {
                    layout.set(active.start_layout);
                } else if let Some(active) = timeline_resize() {
                    layout.write().set_upper_ratio(active.start_ratio);
                }
                panel_resize.set(None);
                timeline_resize.set(None);
                on_resize.call(());
            },

            div { id: "{deck_id}", class: "studio-dock-deck",
                for (position, id) in order.iter().enumerate() {
                    if let Some(panel) = panels.iter().find(|panel| &panel.id == id) {
                        {
                            let panel_id = panel.id.clone();
                            let target_id = panel.id.clone();
                            let label = panel.label.clone();
                            let panel_class = panel.class;
                            let width = weights.get(position).copied().unwrap_or(1.0);
                            let width_style = format!("flex: {:.6} 1 0;", width);
                            let right_neighbor = order.get(position + 1).cloned();
                            let drag_panel_id = panel_id.clone();
                            let click_panel_id = panel_id.clone();
                            let keyboard_panel_id = panel_id.clone();
                            let drop_specs = panel_specs.clone();
                            let keyboard_specs = panel_specs.clone();
                            let drop_scope = scope.clone();
                            let panel_keyboard_scope = scope.clone();
                            rsx! {
                                section {
                                    key: "{panel_id}",
                                    class: "editor-group studio-dock-panel {panel_class}",
                                    style: "{width_style}",
                                    header {
                                        class: "editor-tab-bar editor-tab-bar-active studio-dock-tab-bar",
                                        draggable: "true",
                                        tabindex: "0",
                                        title: "Drag to reorder this editor group. Alt+Shift+Left or Right also moves it.",
                                        ondragstart: move |_| dragging_panel.set(Some(drag_panel_id.clone())),
                                        ondragend: move |_| dragging_panel.set(None),
                                        ondragover: move |event| event.prevent_default(),
                                        ondrop: move |event| {
                                            event.prevent_default();
                                            let Some(dragged) = dragging_panel() else { return };
                                            if layout.write().move_panel(&drop_specs, &dragged, &target_id) {
                                                layout().store(&drop_scope);
                                                on_resize.call(());
                                            }
                                            dragging_panel.set(None);
                                        },
                                        onclick: move |_| on_activate.call(click_panel_id.clone()),
                                        onkeydown: move |event| {
                                            if !(event.modifiers().alt() && event.modifiers().shift()) {
                                                return;
                                            }
                                            let delta = match event.key() {
                                                Key::ArrowLeft => -1,
                                                Key::ArrowRight => 1,
                                                _ => return,
                                            };
                                            event.prevent_default();
                                            if layout.write().move_panel_by(&keyboard_specs, &keyboard_panel_id, delta) {
                                                layout().store(&panel_keyboard_scope);
                                                on_resize.call(());
                                            }
                                        },
                                        span { class: "editor-tab", "{label}" }
                                    }
                                    div { class: "editor-content", {panel.content.clone()} }
                                }
                                if let Some(right) = right_neighbor {
                                    {
                                        let left = id.clone();
                                        let left_key = left.clone();
                                        let right_key = right.clone();
                                        let split_deck_id = deck_id.clone();
                                        let resize_specs = panel_specs.clone();
                                        let resize_scope = scope.clone();
                                        rsx! {
                                            div {
                                                key: "split-{left_key}-{right_key}",
                                                class: "studio-dock-splitter studio-dock-splitter-vertical",
                                                role: "separator",
                                                tabindex: "0",
                                                "aria-label": "Resize {label} and the next editor group",
                                                "aria-orientation": "vertical",
                                                title: "Drag to resize adjacent editor groups. Arrow keys resize from the keyboard.",
                                                onpointerdown: move |event| {
                                                    let Some(span) = element_span(&split_deck_id, true) else { return };
                                                    capture(&event);
                                                    panel_resize.set(Some(PanelResize {
                                                        pointer_origin: event.data().client_coordinates().x,
                                                        left: left.clone(),
                                                        right: right.clone(),
                                                        start_layout: layout(),
                                                        span,
                                                    }));
                                                },
                                                onkeydown: move |event| {
                                                    let delta = match event.key() {
                                                        Key::ArrowLeft => -RESIZE_KEYBOARD_STEP,
                                                        Key::ArrowRight => RESIZE_KEYBOARD_STEP,
                                                        _ => return,
                                                    };
                                                    event.prevent_default();
                                                    if layout.write().resize_pair(&resize_specs, &left_key, &right_key, delta) {
                                                        layout().store(&resize_scope);
                                                        on_resize.call(());
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div {
                class: "studio-dock-splitter studio-dock-splitter-horizontal",
                role: "separator",
                tabindex: "0",
                "aria-label": "Resize editors and Timeline",
                "aria-orientation": "horizontal",
                "aria-valuemin": (MIN_UPPER_RATIO * 100.0).round() as i64,
                "aria-valuemax": (MAX_UPPER_RATIO * 100.0).round() as i64,
                "aria-valuenow": (layout().upper_ratio * 100.0).round() as i64,
                title: "Drag to resize the Timeline. Arrow keys resize; double-click resets.",
                onpointerdown: move |event| {
                    let Some(span) = element_span(&timeline_dock_id, false) else { return };
                    capture(&event);
                    timeline_resize.set(Some(TimelineResize {
                        pointer_origin: event.data().client_coordinates().y,
                        start_ratio: layout().upper_ratio,
                        span,
                    }));
                },
                ondoubleclick: move |_| {
                    if layout.write().set_upper_ratio(DEFAULT_UPPER_RATIO) {
                        layout().store(&reset_scope);
                        on_resize.call(());
                    }
                },
                onkeydown: move |event| {
                    let next = match event.key() {
                        Key::ArrowUp => layout().upper_ratio - RESIZE_KEYBOARD_STEP,
                        Key::ArrowDown => layout().upper_ratio + RESIZE_KEYBOARD_STEP,
                        Key::Home => MIN_UPPER_RATIO,
                        Key::End => MAX_UPPER_RATIO,
                        _ => return,
                    };
                    event.prevent_default();
                    if layout.write().set_upper_ratio(next) {
                        layout().store(&keyboard_scope);
                        on_resize.call(());
                    }
                },
            }

            div { class: "visualization-timeline studio-dock-timeline", {timeline} }
        }
    }
}

fn normalize(weights: &mut [f64]) {
    let sum = weights.iter().sum::<f64>();
    if !sum.is_finite() || sum <= 0.0 {
        let equal = 1.0 / weights.len().max(1) as f64;
        weights.fill(equal);
        return;
    }
    for weight in weights {
        *weight /= sum;
    }
}

fn storage_key(scope: &str) -> String {
    format!("rerobot.editor-dock.v1.{scope}")
}

fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(feature = "web")]
fn element_span(id: &str, horizontal: bool) -> Option<f64> {
    let bounds = web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .get_bounding_client_rect();
    let span = if horizontal {
        bounds.width()
    } else {
        bounds.height()
    };
    (span.is_finite() && span > 0.0).then_some(span)
}

#[cfg(not(feature = "web"))]
fn element_span(_id: &str, _horizontal: bool) -> Option<f64> {
    None
}

#[cfg(feature = "web")]
fn capture(event: &Event<PointerData>) {
    let Some(raw) = event.downcast::<web_sys::PointerEvent>() else {
        return;
    };
    if let Some(target) = raw
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = target.set_pointer_capture(raw.pointer_id());
    }
}

#[cfg(not(feature = "web"))]
fn capture(_event: &Event<PointerData>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(id: &str, weight: f64) -> DockPanel {
        DockPanel {
            id: id.to_owned(),
            label: id.to_owned(),
            class: "",
            preferred_weight: weight,
            content: rsx! { div {} },
        }
    }

    fn specs(panels: &[DockPanel]) -> Vec<(String, f64)> {
        panels
            .iter()
            .map(|panel| (panel.id.clone(), panel.preferred_weight))
            .collect()
    }

    #[test]
    fn new_panels_join_after_the_persisted_order() {
        let layout = DockLayout {
            order: vec!["camera".to_owned(), "scene".to_owned()],
            ..DockLayout::default()
        };
        let panels = vec![panel("scene", 2.0), panel("camera", 1.0), panel("top", 1.0)];
        assert_eq!(
            layout.active_order(&specs(&panels)),
            ["camera", "scene", "top"]
        );
    }

    #[test]
    fn preferred_weights_make_scene_primary_without_hiding_cameras() {
        let layout = DockLayout::default();
        let panels = vec![
            panel("scene", 2.0),
            panel("camera-a", 1.0),
            panel("camera-b", 1.0),
        ];
        let specs = specs(&panels);
        let order = layout.active_order(&specs);
        assert_eq!(layout.active_weights(&order, &specs), [0.5, 0.25, 0.25]);
    }

    #[test]
    fn reordering_keeps_panel_weights_attached_to_panel_identity() {
        let panels = vec![panel("scene", 2.0), panel("camera", 1.0)];
        let mut layout = DockLayout::default();
        let specs = specs(&panels);
        layout.reconcile(&specs);
        assert!(layout.move_panel(&specs, "camera", "scene"));
        assert_eq!(layout.order, ["camera", "scene"]);
        assert_eq!(layout.weights["scene"], 2.0 / 3.0);
    }

    #[test]
    fn split_resize_preserves_total_and_minimums() {
        let panels = vec![panel("scene", 1.0), panel("camera", 1.0)];
        let mut layout = DockLayout::default();
        assert!(layout.resize_pair(&specs(&panels), "scene", "camera", 0.9));
        let total = layout.weights["scene"] + layout.weights["camera"];
        assert!((total - 1.0).abs() < 1e-9);
        assert!(layout.weights["camera"] >= MIN_PANEL_RATIO);
    }
}
