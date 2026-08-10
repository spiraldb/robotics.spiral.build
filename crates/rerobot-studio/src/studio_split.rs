//! The persisted left-workbench / right-visualization split.
//!
//! This is application chrome rather than domain state. It owns one ratio and
//! one separator; Live/Data content remains owned by the shell and is passed in
//! as stable children so resizing cannot reset either region's local state.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "web")]
use wasm_bindgen::JsCast as _;

#[cfg(feature = "web")]
const STORAGE_KEY: &str = "rerobot.workbench-split.v1";
const LAYOUT_VERSION: u8 = 1;
const DEFAULT_WORKBENCH_RATIO: f64 = 1.0 / 3.0;
const LEGACY_DEFAULT_WORKBENCH_RATIO: f64 = 0.40;
const MIN_WORKBENCH_RATIO: f64 = 0.25;
const MAX_WORKBENCH_RATIO: f64 = 0.65;
const KEYBOARD_STEP: f64 = 0.02;
const KEYBOARD_LARGE_STEP: f64 = 0.08;
const SPLIT_ID: &str = "studio-workbench-split";
const WORKBENCH_ID: &str = "studio-workbench-region";
const VISUALIZATION_ID: &str = "studio-visualization-region";

/// The complete persisted layout contract for the two-region Studio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkbenchSplitLayout {
    version: u8,
    workbench_ratio: f64,
}

impl Default for WorkbenchSplitLayout {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            workbench_ratio: DEFAULT_WORKBENCH_RATIO,
        }
    }
}

impl WorkbenchSplitLayout {
    fn ratio(self) -> f64 {
        self.workbench_ratio
    }

    fn valid(self) -> bool {
        self.version == LAYOUT_VERSION
            && self.workbench_ratio.is_finite()
            && (MIN_WORKBENCH_RATIO..=MAX_WORKBENCH_RATIO).contains(&self.workbench_ratio)
    }

    fn decode(value: &str) -> Option<Self> {
        serde_json::from_str(value)
            .ok()
            .filter(|layout: &Self| layout.valid())
    }

    fn set_ratio(&mut self, ratio: f64, bounds: RatioBounds) -> bool {
        if !ratio.is_finite() {
            return false;
        }
        let next = bounds.clamp(ratio);
        if (next - self.workbench_ratio).abs() <= f64::EPSILON {
            return false;
        }
        self.workbench_ratio = next;
        true
    }

    fn reset(&mut self, bounds: RatioBounds) -> bool {
        self.set_ratio(DEFAULT_WORKBENCH_RATIO, bounds)
    }

    fn migrate_default(self) -> Self {
        if (self.workbench_ratio - LEGACY_DEFAULT_WORKBENCH_RATIO).abs() <= f64::EPSILON {
            Self::default()
        } else {
            self
        }
    }

    #[cfg(feature = "web")]
    fn stored() -> Option<Self> {
        let value = web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(STORAGE_KEY)
            .ok()??;
        Self::decode(&value).map(Self::migrate_default)
    }

    #[cfg(not(feature = "web"))]
    fn stored() -> Option<Self> {
        None
    }

    #[cfg(feature = "web")]
    fn store(self) {
        if !self.valid() {
            return;
        }
        let Ok(value) = serde_json::to_string(&self) else {
            return;
        };
        if let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok()?) {
            let _ = storage.set_item(STORAGE_KEY, &value);
        }
    }

    #[cfg(not(feature = "web"))]
    fn store(self) {}
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RatioBounds {
    minimum: f64,
    maximum: f64,
}

impl RatioBounds {
    fn for_span(_span: Option<f64>) -> Self {
        // A proportional contract adapts to every desktop width without
        // persisting device-specific pixels. The Timeline owns its 42rem
        // content floor and scrolls internally when the operator deliberately
        // gives the visualization less room.
        Self {
            minimum: MIN_WORKBENCH_RATIO,
            maximum: MAX_WORKBENCH_RATIO,
        }
    }

    fn clamp(self, ratio: f64) -> f64 {
        ratio.clamp(self.minimum, self.maximum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeDrag {
    pointer_origin: f64,
    start_ratio: f64,
    span: f64,
    bounds: RatioBounds,
}

/// A stable two-region shell with one accessible vertical separator.
///
/// `on_resize` fires during pointer movement and after every keyboard/reset
/// change. The host can feed a monotonically increasing revision into WebGL
/// surfaces that otherwise only observe window-level resize events.
#[component]
pub(crate) fn StudioSplit(
    workbench: Element,
    visualization: Element,
    on_resize: EventHandler<()>,
) -> Element {
    let mut layout = use_signal(|| WorkbenchSplitLayout::stored().unwrap_or_default());
    let mut resizing = use_signal(|| None::<ResizeDrag>);
    let ratio = layout().ratio();
    let bounds = RatioBounds::for_span(split_span());
    let workbench_style = format!("flex-basis: {:.5}%", ratio * 100.0);
    let aria_value_text = format!(
        "{}% workbench, {}% visualization",
        (ratio * 100.0).round() as i64,
        ((1.0 - ratio) * 100.0).round() as i64,
    );
    let resize_class = if resizing().is_some() {
        " studio-workbench-split-resizing"
    } else {
        ""
    };

    rsx! {
        div {
            id: SPLIT_ID,
            class: "studio-workbench-split{resize_class}",
            onpointermove: move |event| {
                let Some(active) = resizing() else { return };
                let pointer = event.data().client_coordinates().x;
                let next = active.start_ratio + (pointer - active.pointer_origin) / active.span;
                if layout.write().set_ratio(next, active.bounds) {
                    on_resize.call(());
                }
            },
            onpointerup: move |_| {
                if resizing().is_some() {
                    resizing.set(None);
                    layout().store();
                    on_resize.call(());
                }
            },
            onpointercancel: move |_| {
                let Some(active) = resizing() else { return };
                resizing.set(None);
                if layout.write().set_ratio(active.start_ratio, active.bounds) {
                    on_resize.call(());
                }
            },

            div {
                id: WORKBENCH_ID,
                class: "studio-workbench-split-region studio-workbench-split-workbench",
                style: "{workbench_style}",
                {workbench}
            }
            div {
                class: "studio-workbench-splitter",
                role: "separator",
                tabindex: "0",
                "aria-label": "Resize operating workbench",
                "aria-orientation": "vertical",
                "aria-controls": "{WORKBENCH_ID} {VISUALIZATION_ID}",
                "aria-valuemin": (bounds.minimum * 100.0).round() as i64,
                "aria-valuemax": (bounds.maximum * 100.0).round() as i64,
                "aria-valuenow": (ratio * 100.0).round() as i64,
                "aria-valuetext": "{aria_value_text}",
                title: "Drag to resize. Arrow keys resize; Shift moves farther; Home and End jump to the limits; double-click resets.",
                onpointerdown: move |event| {
                    let Some(span) = split_span().filter(|span| *span > 0.0) else { return };
                    capture(&event);
                    resizing.set(Some(ResizeDrag {
                        pointer_origin: event.data().client_coordinates().x,
                        start_ratio: layout().ratio(),
                        span,
                        bounds: RatioBounds::for_span(Some(span)),
                    }));
                },
                ondoubleclick: move |_| {
                    let bounds = RatioBounds::for_span(split_span());
                    if layout.write().reset(bounds) {
                        layout().store();
                        on_resize.call(());
                    }
                },
                onkeydown: move |event| {
                    let bounds = RatioBounds::for_span(split_span());
                    let current = layout().ratio();
                    let step = if event.modifiers().shift() {
                        KEYBOARD_LARGE_STEP
                    } else {
                        KEYBOARD_STEP
                    };
                    let next = match event.key() {
                        Key::ArrowLeft => current - step,
                        Key::ArrowRight => current + step,
                        Key::Home => bounds.minimum,
                        Key::End => bounds.maximum,
                        _ => return,
                    };
                    event.prevent_default();
                    if layout.write().set_ratio(next, bounds) {
                        layout().store();
                        on_resize.call(());
                    }
                },
            }
            div {
                id: VISUALIZATION_ID,
                class: "studio-workbench-split-region studio-workbench-split-visualization",
                {visualization}
            }
        }
    }
}

#[cfg(feature = "web")]
fn split_span() -> Option<f64> {
    let width = web_sys::window()?
        .document()?
        .get_element_by_id(SPLIT_ID)?
        .get_bounding_client_rect()
        .width();
    (width.is_finite() && width > 0.0).then_some(width)
}

#[cfg(not(feature = "web"))]
fn split_span() -> Option<f64> {
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

    #[test]
    fn default_is_the_requested_one_to_two_split() {
        let layout = WorkbenchSplitLayout::default();
        assert!(layout.valid());
        assert_eq!(layout.ratio(), 1.0 / 3.0);
    }

    #[test]
    fn invalid_or_stale_persisted_layouts_are_rejected() {
        assert!(WorkbenchSplitLayout::decode(r#"{"version":1,"workbench_ratio":0.4}"#).is_some());
        assert!(WorkbenchSplitLayout::decode(r#"{"version":2,"workbench_ratio":0.4}"#).is_none());
        assert!(WorkbenchSplitLayout::decode(r#"{"version":1,"workbench_ratio":0.9}"#).is_none());
        assert!(
            !WorkbenchSplitLayout {
                version: LAYOUT_VERSION,
                workbench_ratio: f64::NAN,
            }
            .valid()
        );
    }

    #[test]
    fn the_old_default_migrates_but_an_adjusted_split_is_preserved() {
        let old_default = WorkbenchSplitLayout {
            version: LAYOUT_VERSION,
            workbench_ratio: LEGACY_DEFAULT_WORKBENCH_RATIO,
        };
        assert_eq!(
            old_default.migrate_default(),
            WorkbenchSplitLayout::default()
        );

        let adjusted = WorkbenchSplitLayout {
            version: LAYOUT_VERSION,
            workbench_ratio: 0.37,
        };
        assert_eq!(adjusted.migrate_default(), adjusted);
    }

    #[test]
    fn ratio_clamping_never_leaves_the_safe_proportional_range() {
        let bounds = RatioBounds::for_span(None);
        assert_eq!(bounds.clamp(-10.0), MIN_WORKBENCH_RATIO);
        assert_eq!(bounds.clamp(10.0), MAX_WORKBENCH_RATIO);
    }

    #[test]
    fn roomy_splits_keep_the_same_predictable_contract() {
        let bounds = RatioBounds::for_span(Some(1_600.0));
        assert_eq!(bounds.minimum, MIN_WORKBENCH_RATIO);
        assert_eq!(bounds.maximum, MAX_WORKBENCH_RATIO);
    }

    #[test]
    fn narrow_splits_fall_back_to_valid_proportions() {
        let bounds = RatioBounds::for_span(Some(700.0));
        assert_eq!(bounds.minimum, MIN_WORKBENCH_RATIO);
        assert_eq!(bounds.maximum, MAX_WORKBENCH_RATIO);
        assert!(bounds.minimum <= bounds.maximum);
    }
}
