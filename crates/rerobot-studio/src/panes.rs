//! The Studio's stable three-pane bench.
//!
//! Robots stays on the left, Scene remains the elastic center, and Properties
//! stays on the right. The two side panes may be resized or hidden, but the
//! workspace has no arbitrary topology: the product has three semantic
//! surfaces, so its layout state is a handful of facts rather than an editor
//! tree.
//!
//! The one place that stacks is the Robots column, which carries Signals under
//! the arm list. That is not a fourth pane: the arms and their traces are the
//! same subject seen twice — what the bench is holding, and what it just did —
//! and putting the plots there keeps the Scene at full size while a take is
//! being read.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "web")]
use wasm_bindgen::JsCast;

use crate::control::JointPanel;
use crate::icons::{PropertiesPanel, RobotPanelIcon, ScenePanel};
use crate::setup::RobotPanel;
use crate::signals::SignalsPanel;
use crate::state::studio;
use crate::viewport::Viewport;

#[cfg(feature = "web")]
const STORAGE_KEY: &str = "rerobot.panes.v2";
const DEFAULT_ROBOTS_RATIO: f64 = 0.22;
const DEFAULT_PROPERTIES_RATIO: f64 = 0.23;
/// The plots ship taller than half the column: the arm list is four lines and
/// a footer, while this half carries two traces, a zoom, and a readout.
const DEFAULT_SIGNALS_RATIO: f64 = 0.55;
const MIN_RATIO: f64 = 0.14;
const MAX_RATIO: f64 = 0.52;
const MAX_COMBINED_SIDE_RATIO: f64 = 0.72;
/// The signals slot may take most of its column but never all of it: the arm
/// list above it is how an arm gets connected in the first place.
const MIN_SIGNALS_RATIO: f64 = 0.18;
const MAX_SIGNALS_RATIO: f64 = 0.78;
#[cfg(feature = "web")]
const MIN_SIDE_PANE: f64 = 180.0;
#[cfg(feature = "web")]
const MIN_SCENE_PANE: f64 = 280.0;
#[cfg(feature = "web")]
const MIN_ROBOT_LIST: f64 = 96.0;
#[cfg(feature = "web")]
const MIN_SIGNALS_SLOT: f64 = 140.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StudioPane {
    Robots,
    Scene,
    Properties,
}

impl StudioPane {
    pub(crate) const ALL: [Self; 3] = [Self::Robots, Self::Scene, Self::Properties];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Robots => "Robots",
            Self::Scene => "Scene",
            Self::Properties => "Properties",
        }
    }
}

/// The only layout preferences a three-surface bench needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PaneLayout {
    version: u8,
    robots_open: bool,
    properties_open: bool,
    signals_open: bool,
    robots_ratio: f64,
    properties_ratio: f64,
    /// Share of the Robots column's height given to Signals.
    signals_ratio: f64,
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::bench()
    }
}

impl PaneLayout {
    pub(crate) fn bench() -> Self {
        Self {
            version: 2,
            robots_open: true,
            properties_open: true,
            signals_open: true,
            robots_ratio: DEFAULT_ROBOTS_RATIO,
            properties_ratio: DEFAULT_PROPERTIES_RATIO,
            signals_ratio: DEFAULT_SIGNALS_RATIO,
        }
    }

    pub(crate) fn contains(&self, pane: StudioPane) -> bool {
        match pane {
            StudioPane::Robots => self.robots_open,
            StudioPane::Scene => true,
            StudioPane::Properties => self.properties_open,
        }
    }

    /// True when Signals is showing, which requires the column that holds it.
    pub(crate) fn signals_open(&self) -> bool {
        self.signals_open && self.robots_open
    }

    pub(crate) fn toggle(&mut self, pane: StudioPane) {
        match pane {
            StudioPane::Robots => self.robots_open = !self.robots_open,
            StudioPane::Scene => {}
            StudioPane::Properties => self.properties_open = !self.properties_open,
        }
    }

    /// Show or hide the plots.
    ///
    /// Turning them on opens the column they live in rather than quietly doing
    /// nothing: a control that reports itself pressed and changes nothing on
    /// screen is worse than one that is missing.
    pub(crate) fn toggle_signals(&mut self) {
        self.signals_open = !self.signals_open();
        if self.signals_open {
            self.robots_open = true;
        }
    }

    pub(crate) fn show(&mut self, pane: StudioPane) {
        match pane {
            StudioPane::Robots => self.robots_open = true,
            StudioPane::Scene => {}
            StudioPane::Properties => self.properties_open = true,
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        self == &Self::bench()
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::bench();
    }

    fn ratio(&self, side: ResizeSide) -> f64 {
        match side {
            ResizeSide::Robots => self.robots_ratio,
            ResizeSide::Properties => self.properties_ratio,
            ResizeSide::Signals => self.signals_ratio,
        }
    }

    fn other_ratio(&self, side: ResizeSide) -> f64 {
        match side {
            ResizeSide::Robots if self.properties_open => self.properties_ratio,
            ResizeSide::Properties if self.robots_open => self.robots_ratio,
            _ => 0.0,
        }
    }

    fn minimum_ratio(&self, side: ResizeSide) -> f64 {
        match side {
            ResizeSide::Signals => MIN_SIGNALS_RATIO,
            _ => MIN_RATIO,
        }
    }

    fn maximum_ratio(&self, side: ResizeSide) -> f64 {
        match side {
            ResizeSide::Signals => MAX_SIGNALS_RATIO,
            _ => (MAX_COMBINED_SIDE_RATIO - self.other_ratio(side)).clamp(MIN_RATIO, MAX_RATIO),
        }
    }

    fn set_ratio(&mut self, side: ResizeSide, ratio: f64) {
        let ratio = ratio.clamp(self.minimum_ratio(side), self.maximum_ratio(side));
        match side {
            ResizeSide::Robots => self.robots_ratio = ratio,
            ResizeSide::Properties => self.properties_ratio = ratio,
            ResizeSide::Signals => self.signals_ratio = ratio,
        }
    }

    #[cfg(any(feature = "web", test))]
    fn valid(&self) -> bool {
        self.version == 2
            && self.robots_ratio.is_finite()
            && self.properties_ratio.is_finite()
            && self.signals_ratio.is_finite()
            && (MIN_RATIO..=MAX_RATIO).contains(&self.robots_ratio)
            && (MIN_RATIO..=MAX_RATIO).contains(&self.properties_ratio)
            && (MIN_SIGNALS_RATIO..=MAX_SIGNALS_RATIO).contains(&self.signals_ratio)
            && self.robots_ratio + self.properties_ratio <= MAX_COMBINED_SIDE_RATIO
    }

    #[cfg(feature = "web")]
    pub(crate) fn stored() -> Option<Self> {
        let value = web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(STORAGE_KEY)
            .ok()??;
        serde_json::from_str::<Self>(&value)
            .ok()
            .filter(Self::valid)
    }

    #[cfg(feature = "web")]
    pub(crate) fn store(&self) {
        let Ok(value) = serde_json::to_string(self) else {
            return;
        };
        if let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok()?) {
            let _ = storage.set_item(STORAGE_KEY, &value);
        }
    }

    #[cfg(not(feature = "web"))]
    pub(crate) fn store(&self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeSide {
    Robots,
    Properties,
    Signals,
}

impl ResizeSide {
    /// Signals stacks, so its separator runs across the column rather than
    /// down it, and it reads the pointer's other axis.
    const fn vertical(self) -> bool {
        matches!(self, Self::Signals)
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Robots => "Resize Robots panel",
            Self::Properties => "Resize Properties panel",
            Self::Signals => "Resize Signals region",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeDrag {
    side: ResizeSide,
    pointer_origin: f64,
    start_ratio: f64,
    span: f64,
    minimum: f64,
    maximum: f64,
}

#[component]
pub(crate) fn PaneWorkspace(mut layout: Signal<PaneLayout>, mobile_pane: StudioPane) -> Element {
    let mut resizing = use_signal(|| None::<ResizeDrag>);
    let robots_open = layout.read().robots_open;
    let properties_open = layout.read().properties_open;
    let signals_open = layout.read().signals_open();
    let robots_style = format!("flex-basis: {:.4}%", layout.read().robots_ratio * 100.0);
    let properties_style = format!("flex-basis: {:.4}%", layout.read().properties_ratio * 100.0);
    let signals_style = format!("flex-basis: {:.4}%", layout.read().signals_ratio * 100.0);
    let resize_class = match resizing.read().as_ref().map(|active| active.side.vertical()) {
        Some(true) => " pane-workspace-resizing pane-workspace-resizing-row",
        Some(false) => " pane-workspace-resizing",
        None => "",
    };

    rsx! {
        div {
            class: "pane-workspace{resize_class}",
            onpointermove: move |event| {
                let Some(active) = *resizing.read() else { return };
                let pointer = if active.side.vertical() {
                    event.data().client_coordinates().y
                } else {
                    event.data().client_coordinates().x
                };
                let delta = (pointer - active.pointer_origin) / active.span;
                let next = match active.side {
                    ResizeSide::Robots => active.start_ratio + delta,
                    ResizeSide::Properties | ResizeSide::Signals => active.start_ratio - delta,
                };
                layout
                    .write()
                    .set_ratio(active.side, next.clamp(active.minimum, active.maximum));
            },
            onpointerup: move |_| {
                if resizing.read().is_some() {
                    resizing.set(None);
                    layout.read().store();
                }
            },
            onpointercancel: move |_| resizing.set(None),

            if robots_open {
                aside {
                    class: "workspace-pane workspace-pane-side studio-panel pane-column {mobile_class(StudioPane::Robots, mobile_pane)}",
                    style: "{robots_style}",
                    div { class: "pane-slot pane-slot-elastic",
                        PaneHeading { pane: StudioPane::Robots, context: workspace_name() }
                        RobotPanel { heading: false }
                    }
                    if signals_open {
                        PaneSplitter {
                            side: ResizeSide::Signals,
                            layout,
                            resizing,
                        }
                        div { class: "pane-slot pane-slot-fixed", style: "{signals_style}",
                            SignalsPanel {}
                        }
                    }
                    if !studio().controls_available() { ReconnectGuard {} }
                }
                PaneSplitter {
                    side: ResizeSide::Robots,
                    layout,
                    resizing,
                }
            }

            main {
                class: "workspace-pane workspace-pane-scene studio-workspace {mobile_class(StudioPane::Scene, mobile_pane)}",
                PaneHeading { pane: StudioPane::Scene }
                section { class: "viewport-tile", Viewport {} }
            }

            if properties_open {
                PaneSplitter {
                    side: ResizeSide::Properties,
                    layout,
                    resizing,
                }
                aside {
                    class: "workspace-pane workspace-pane-side studio-panel {mobile_class(StudioPane::Properties, mobile_pane)}",
                    style: "{properties_style}",
                    PaneHeading { pane: StudioPane::Properties }
                    JointPanel {}
                    if !studio().controls_available() { ReconnectGuard {} }
                }
            }
        }
    }
}

#[component]
fn PaneHeading(pane: StudioPane, #[props(optional)] context: Option<String>) -> Element {
    rsx! {
        header { class: "pane-heading",
            h2 {
                match pane {
                    StudioPane::Robots => rsx! { RobotPanelIcon {} },
                    StudioPane::Scene => rsx! { ScenePanel {} },
                    StudioPane::Properties => rsx! { PropertiesPanel {} },
                }
                span { "{pane.label()}" }
            }
            if let Some(context) = context {
                span { class: "pane-heading-context", title: "Robot files are read and written here", "{context}" }
            }
        }
    }
}

#[component]
fn PaneSplitter(
    side: ResizeSide,
    mut layout: Signal<PaneLayout>,
    mut resizing: Signal<Option<ResizeDrag>>,
) -> Element {
    let ratio = layout.read().ratio(side);
    let minimum = layout.read().minimum_ratio(side);
    let maximum = layout.read().maximum_ratio(side);
    let vertical = side.vertical();

    rsx! {
        div {
            class: if vertical { "pane-splitter pane-splitter-row" } else { "pane-splitter" },
            role: "separator",
            "aria-label": "{side.label()}",
            "aria-orientation": if vertical { "horizontal" } else { "vertical" },
            "aria-valuenow": (ratio * 100.0).round() as i64,
            "aria-valuemin": (minimum * 100.0).round() as i64,
            "aria-valuemax": (maximum * 100.0).round() as i64,
            tabindex: 0,
            title: "Drag, or focus and use arrow keys, to resize",
            onpointerdown: move |event| {
                if let Some(active) = resize_start(&event, side, &layout.read()) {
                    capture(&event);
                    resizing.set(Some(active));
                }
            },
            onkeydown: move |event| {
                let step = if event.modifiers().shift() { 0.08 } else { 0.02 };
                let current = layout.read().ratio(side);
                let next = match (side, event.key()) {
                    (ResizeSide::Robots, Key::ArrowLeft)
                    | (ResizeSide::Properties, Key::ArrowRight)
                    | (ResizeSide::Signals, Key::ArrowDown) => current - step,
                    (ResizeSide::Robots, Key::ArrowRight)
                    | (ResizeSide::Properties, Key::ArrowLeft)
                    | (ResizeSide::Signals, Key::ArrowUp) => current + step,
                    (_, Key::Home) => layout.read().minimum_ratio(side),
                    (_, Key::End) => layout.read().maximum_ratio(side),
                    _ => return,
                };
                event.prevent_default();
                layout.write().set_ratio(side, next);
                layout.read().store();
            },
        }
    }
}

fn mobile_class(pane: StudioPane, mobile_pane: StudioPane) -> &'static str {
    if pane == mobile_pane {
        "surface-active-mobile"
    } else {
        "surface-hidden-mobile"
    }
}

fn workspace_name() -> Option<String> {
    let workspace = studio().snapshot.read().workspace.clone();
    workspace
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .map(ToOwned::to_owned)
}

#[component]
fn ReconnectGuard() -> Element {
    rsx! {
        div { class: "reconnect-guard", role: "status",
            div { class: "reconnect-card",
                span { class: "status-dot", style: "background: var(--tone-danger)" }
                span { "Telemetry unavailable — controls paused" }
            }
        }
    }
}

#[cfg(feature = "web")]
fn resize_start(
    event: &Event<PointerData>,
    side: ResizeSide,
    layout: &PaneLayout,
) -> Option<ResizeDrag> {
    let raw = event.downcast::<web_sys::PointerEvent>()?;
    let target = raw.target()?.dyn_into::<web_sys::Element>().ok()?;
    let parent = target.parent_element()?;
    let bounds = parent.get_bounding_client_rect();
    let (span, pointer_origin) = if side.vertical() {
        (bounds.height(), event.data().client_coordinates().y)
    } else {
        (bounds.width(), event.data().client_coordinates().x)
    };
    if span <= 0.0 {
        return None;
    }
    let (minimum, maximum) = if side.vertical() {
        (
            (MIN_SIGNALS_SLOT / span).clamp(MIN_SIGNALS_RATIO, MAX_SIGNALS_RATIO),
            (1.0 - MIN_ROBOT_LIST / span).clamp(MIN_SIGNALS_RATIO, MAX_SIGNALS_RATIO),
        )
    } else {
        let minimum = (MIN_SIDE_PANE / span).clamp(MIN_RATIO, 0.32);
        let other = layout.other_ratio(side);
        (
            minimum,
            (1.0 - other - MIN_SCENE_PANE / span).clamp(minimum, layout.maximum_ratio(side)),
        )
    };
    Some(ResizeDrag {
        side,
        pointer_origin,
        start_ratio: layout.ratio(side),
        span,
        minimum,
        maximum: maximum.max(minimum),
    })
}

#[cfg(not(feature = "web"))]
fn resize_start(
    _event: &Event<PointerData>,
    _side: ResizeSide,
    _layout: &PaneLayout,
) -> Option<ResizeDrag> {
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
    fn default_bench_is_valid_and_keeps_scene_visible() {
        let layout = PaneLayout::bench();
        assert!(layout.valid());
        assert!(
            StudioPane::ALL
                .into_iter()
                .all(|pane| layout.contains(pane))
        );
        assert!(layout.signals_open(), "the plots ship on");
    }

    #[test]
    fn only_side_panes_can_be_hidden() {
        let mut layout = PaneLayout::bench();
        layout.toggle(StudioPane::Scene);
        assert!(layout.contains(StudioPane::Scene));
        layout.toggle(StudioPane::Robots);
        layout.toggle(StudioPane::Properties);
        assert!(!layout.contains(StudioPane::Robots));
        assert!(!layout.contains(StudioPane::Properties));
        assert!(layout.valid());
    }

    #[test]
    fn signals_cannot_be_shown_without_the_column_that_holds_it() {
        let mut layout = PaneLayout::bench();
        layout.toggle(StudioPane::Robots);
        assert!(!layout.signals_open(), "a hidden column hides its contents");

        layout.toggle_signals();
        assert!(layout.signals_open());
        assert!(
            layout.contains(StudioPane::Robots),
            "asking for the plots opens the column they are in"
        );

        layout.toggle_signals();
        assert!(!layout.signals_open());
        assert!(layout.contains(StudioPane::Robots), "and leaves it open");
    }

    #[test]
    fn side_widths_preserve_a_usable_center() {
        let mut layout = PaneLayout::bench();
        layout.set_ratio(ResizeSide::Robots, 0.9);
        assert!(layout.robots_ratio <= MAX_COMBINED_SIDE_RATIO - layout.properties_ratio);
        assert!(layout.valid());
    }

    #[test]
    fn the_signals_slot_never_swallows_the_arm_list() {
        let mut layout = PaneLayout::bench();
        layout.set_ratio(ResizeSide::Signals, 5.0);
        assert_eq!(layout.signals_ratio, MAX_SIGNALS_RATIO);
        layout.set_ratio(ResizeSide::Signals, 0.0);
        assert_eq!(layout.signals_ratio, MIN_SIGNALS_RATIO);
        assert!(layout.valid());
    }

    #[test]
    fn persisted_preferences_round_trip() {
        let mut layout = PaneLayout::bench();
        layout.toggle(StudioPane::Properties);
        layout.set_ratio(ResizeSide::Robots, 0.31);
        layout.set_ratio(ResizeSide::Signals, 0.6);
        let encoded = serde_json::to_string(&layout).unwrap();
        let decoded: PaneLayout = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, layout);
        assert!(decoded.valid());
    }

    #[test]
    fn invalid_saved_preferences_are_rejected() {
        let invalid = PaneLayout {
            version: 3,
            robots_open: true,
            properties_open: true,
            signals_open: true,
            robots_ratio: 0.9,
            properties_ratio: 0.9,
            signals_ratio: 0.9,
        };
        assert!(!invalid.valid());
    }
}
