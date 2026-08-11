use dioxus::prelude::*;

use crate::{
    DockZone, LayoutNode, PanelId, PanelLayout, PanelPlacement, SplitAxis, SplitId, Tile, TileId,
};

#[cfg(feature = "web")]
use wasm_bindgen::JsCast as _;

const PANEL_STYLES: Asset = asset!("/assets/panels.css");

const RESIZE_KEYBOARD_STEP: f64 = 0.025;
const RESIZE_KEYBOARD_LARGE_STEP: f64 = 0.08;

/// Application content registered with a [`PanelWorkspace`].
#[derive(Clone, PartialEq)]
pub struct Panel {
    pub id: PanelId,
    pub title: String,
    pub home: TileId,
    pub home_zone: DockZone,
    pub class: String,
    pub content: Element,
}

impl Panel {
    pub fn new(
        id: impl Into<PanelId>,
        title: impl Into<String>,
        home: impl Into<TileId>,
        content: Element,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            home: home.into(),
            home_zone: DockZone::Center,
            class: String::new(),
            content,
        }
    }

    pub fn with_class(mut self, class: impl Into<String>) -> Self {
        self.class = class.into();
        self
    }

    /// Split a newly registered panel beside its occupied home instead of
    /// hiding it as another tab. Persisted user placement still wins.
    pub fn with_home_zone(mut self, zone: DockZone) -> Self {
        self.home_zone = zone;
        self
    }

    fn placement(&self) -> PanelPlacement {
        PanelPlacement::new(self.id.clone(), self.home.clone()).with_zone(self.home_zone)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DropPreview {
    tile: TileId,
    zone: DockZone,
}

#[derive(Clone, Debug, PartialEq)]
struct ResizeDrag {
    split: SplitId,
    axis: SplitAxis,
    pointer_origin: f64,
    start_ratio: f64,
    span: f64,
}

#[derive(Clone, PartialEq)]
struct RenderContext {
    panels: Vec<Panel>,
    placements: Vec<PanelPlacement>,
    layout: Signal<PanelLayout>,
    mobile_panel: Signal<Option<PanelId>>,
    dragging: Signal<Option<PanelId>>,
    drop_preview: Signal<Option<DropPreview>>,
    resizing: Signal<Option<ResizeDrag>>,
    storage_key: Option<String>,
    on_layout_change: EventHandler<PanelLayout>,
    on_panel_activate: EventHandler<PanelId>,
    on_resize: EventHandler<()>,
}

/// Render a persisted tree of tabbed panels and resizable splits.
///
/// The application owns panel content and the initial tree. The workspace owns
/// subsequent layout mutation. Drag a tab onto the center of a tile to attach
/// it as a tab, or onto an edge to create a split. `Alt+Shift+Arrow` splits the
/// active tab from the keyboard; `Alt+Shift+PageUp/PageDown` attaches it to the
/// previous or next tile.
#[component]
pub fn PanelWorkspace(
    panels: Vec<Panel>,
    initial_layout: PanelLayout,
    storage_key: Option<String>,
    on_layout_change: EventHandler<PanelLayout>,
    on_panel_activate: EventHandler<PanelId>,
    on_resize: EventHandler<()>,
) -> Element {
    let placements = panels.iter().map(Panel::placement).collect::<Vec<_>>();
    let first_mobile_panel = panels.first().map(|panel| panel.id.clone());
    let initial_key = storage_key.clone();
    let initial_placements = placements.clone();
    let initial_fallback = initial_layout.clone();
    let mut layout = use_signal(move || {
        layout_for_registry(
            initial_key.as_deref(),
            initial_fallback,
            &initial_placements,
        )
    });
    let initial_scope = storage_key.clone();
    let mut layout_scope = use_signal(move || initial_scope);
    let mut mobile_panel = use_signal(move || first_mobile_panel);
    let mut dragging = use_signal(|| None::<PanelId>);
    let mut drop_preview = use_signal(|| None::<DropPreview>);
    let mut resizing = use_signal(|| None::<ResizeDrag>);

    // A workspace component may remain mounted while its host changes from
    // Live to Data. Component keys are a rendering hint, not an ownership
    // boundary, so explicitly reload the layout for the new storage scope.
    // Without this, the new registry could be painted from a reconciled copy
    // of the previous mode while mutations and persistence still targeted the
    // previous tree.
    let next_scope = storage_key.clone();
    let next_fallback = initial_layout.clone();
    let next_placements = placements.clone();
    let next_mobile_panel = panels.first().map(|panel| panel.id.clone());
    use_effect(move || {
        if layout_scope.peek().as_ref() == next_scope.as_ref() {
            return;
        }
        let next = layout_for_registry(
            next_scope.as_deref(),
            next_fallback.clone(),
            &next_placements,
        );
        layout_scope.set(next_scope.clone());
        layout.set(next);
        mobile_panel.set(next_mobile_panel.clone());
        dragging.set(None);
        drop_preview.set(None);
        resizing.set(None);
    });

    let display_layout = layout().reconciled(&placements);
    let context = RenderContext {
        panels,
        placements,
        layout,
        mobile_panel,
        dragging,
        drop_preview,
        resizing,
        storage_key: storage_key.clone(),
        on_layout_change,
        on_panel_activate,
        on_resize,
    };
    let pointer_context = context.clone();
    let release_context = context.clone();
    let cancel_context = context.clone();
    let resizing_active = resizing().is_some();
    let dragging_active = dragging().is_some();
    let mobile_selection = mobile_panel()
        .filter(|id| context.panels.iter().any(|panel| &panel.id == id))
        .or_else(|| context.panels.first().map(|panel| panel.id.clone()));
    let mobile_tabs = context.panels.clone();

    rsx! {
        document::Stylesheet { href: PANEL_STYLES }
        section {
            class: "rp-workspace",
            "data-resizing": resizing_active,
            "data-dragging": dragging_active,
            onpointermove: move |event| {
                let Some(active) = resizing() else { return };
                let pointer = match active.axis {
                    SplitAxis::Horizontal => event.data().client_coordinates().x,
                    SplitAxis::Vertical => event.data().client_coordinates().y,
                };
                let next_ratio = active.start_ratio
                    + (pointer - active.pointer_origin) / active.span;
                let mut next = layout();
                if next.set_split_ratio(&active.split, next_ratio) {
                    layout.set(next.clone());
                    pointer_context.on_layout_change.call(next);
                    pointer_context.on_resize.call(());
                }
            },
            onpointerup: move |_| {
                if resizing().is_some() {
                    resizing.set(None);
                    let next = layout();
                    store_if_requested(release_context.storage_key.as_deref(), &next);
                    release_context.on_layout_change.call(next);
                    release_context.on_resize.call(());
                }
            },
            onpointercancel: move |_| {
                let Some(active) = resizing() else { return };
                resizing.set(None);
                let mut next = layout();
                if next.set_split_ratio(&active.split, active.start_ratio) {
                    layout.set(next.clone());
                    cancel_context.on_layout_change.call(next);
                    cancel_context.on_resize.call(());
                }
            },
            ondragend: move |_| {
                dragging.set(None);
                drop_preview.set(None);
            },
            nav { class: "rp-mobile-switcher", "aria-label": "Workspace panels",
                for panel in mobile_tabs {
                    {
                        let panel_id = panel.id.clone();
                        let activate_id = panel.id.clone();
                        let activate_context = context.clone();
                        let is_active = mobile_selection.as_ref() == Some(&panel.id);
                        rsx! {
                            button {
                                key: "mobile-{panel.id}",
                                r#type: "button",
                                class: if is_active { "rp-mobile-tab rp-mobile-tab-active" } else { "rp-mobile-tab" },
                                "aria-pressed": is_active,
                                onclick: move |_| {
                                    mobile_panel.set(Some(panel_id.clone()));
                                    mutate_layout(activate_context.clone(), |layout| layout.activate(&activate_id));
                                    activate_context.on_panel_activate.call(activate_id.clone());
                                },
                                "{panel.title}"
                            }
                        }
                    }
                }
            }
            {render_node(display_layout.root, context)}
        }
    }
}

fn render_node(node: LayoutNode, context: RenderContext) -> Element {
    match node {
        LayoutNode::Tile(tile) => render_tile(tile, context),
        LayoutNode::Split {
            id,
            axis,
            ratio,
            first,
            second,
        } => {
            let split_dom_id = format!("rp-split-{}", safe_id(id.as_str()));
            let orientation = match axis {
                SplitAxis::Horizontal => "vertical",
                SplitAxis::Vertical => "horizontal",
            };
            let split_class = match axis {
                SplitAxis::Horizontal => "rp-split rp-split-horizontal",
                SplitAxis::Vertical => "rp-split rp-split-vertical",
            };
            let first_style = format!("flex-basis: {:.5}%;", ratio * 100.0);
            let first_context = context.clone();
            let second_context = context.clone();
            let mut pointer_resizing = context.resizing;
            let keyboard_context = context.clone();
            let reset_context = context.clone();
            let pointer_split = id.clone();
            let keyboard_split = id.clone();
            let reset_split = id.clone();

            rsx! {
                div { id: "{split_dom_id}", class: "{split_class}",
                    div { class: "rp-split-child rp-split-first", style: "{first_style}",
                        {render_node(*first, first_context)}
                    }
                    div {
                        class: "rp-splitter",
                        role: "separator",
                        tabindex: "0",
                        "aria-label": "Resize panel groups",
                        "aria-orientation": "{orientation}",
                        "aria-valuemin": 10,
                        "aria-valuemax": 90,
                        "aria-valuenow": (ratio * 100.0).round() as i64,
                        title: "Drag to resize. Arrow keys resize; Shift moves farther; double-click resets.",
                        onpointerdown: move |event| {
                            let Some(span) = element_span(&split_dom_id, axis) else { return };
                            capture(&event);
                            let pointer = match axis {
                                SplitAxis::Horizontal => event.data().client_coordinates().x,
                                SplitAxis::Vertical => event.data().client_coordinates().y,
                            };
                            pointer_resizing.set(Some(ResizeDrag {
                                split: pointer_split.clone(),
                                axis,
                                pointer_origin: pointer,
                                start_ratio: ratio,
                                span,
                            }));
                        },
                        ondoubleclick: move |_| {
                            mutate_layout(reset_context.clone(), |layout| {
                                layout.set_split_ratio(&reset_split, 0.5)
                            });
                        },
                        onkeydown: move |event| {
                            let step = if event.modifiers().shift() {
                                RESIZE_KEYBOARD_LARGE_STEP
                            } else {
                                RESIZE_KEYBOARD_STEP
                            };
                            let delta = match (axis, event.key()) {
                                (SplitAxis::Horizontal, Key::ArrowLeft)
                                | (SplitAxis::Vertical, Key::ArrowUp) => -step,
                                (SplitAxis::Horizontal, Key::ArrowRight)
                                | (SplitAxis::Vertical, Key::ArrowDown) => step,
                                _ => return,
                            };
                            event.prevent_default();
                            mutate_layout(keyboard_context.clone(), |layout| {
                                let Some(current) = layout.split_ratio(&keyboard_split) else {
                                    return false;
                                };
                                layout.set_split_ratio(&keyboard_split, current + delta)
                            });
                        },
                    }
                    div { class: "rp-split-child rp-split-second",
                        {render_node(*second, second_context)}
                    }
                }
            }
        }
    }
}

fn render_tile(tile: Tile, context: RenderContext) -> Element {
    let tile_id = tile.id.clone();
    let preview = (context.drop_preview)().filter(|preview| preview.tile == tile.id);
    let mobile_selection = (context.mobile_panel)()
        .filter(|id| context.panels.iter().any(|panel| &panel.id == id))
        .or_else(|| context.panels.first().map(|panel| panel.id.clone()));
    let mobile_active = mobile_selection
        .as_ref()
        .is_some_and(|panel| tile.panels.contains(panel));
    let mut tile_classes = vec!["rp-tile"];
    if preview.is_some() {
        tile_classes.push("rp-tile-drop-active");
    }
    if mobile_active {
        tile_classes.push("rp-mobile-active");
    }
    if tile.id.as_str().starts_with("tile-") {
        tile_classes.push("rp-tile-enter");
    }
    let tile_class = tile_classes.join(" ");
    let right_context = context.clone();
    let down_context = context.clone();
    let close_context = context.clone();
    let right_tile = tile.id.clone();
    let down_tile = tile.id.clone();
    let close_tile = tile.id.clone();

    rsx! {
        section { key: "{tile.id}", class: "{tile_class}", "data-tile-id": "{tile.id}",
            header { class: "rp-tab-bar",
                div { class: "rp-tabs", role: "tablist", "aria-label": "Panel group",
                    for panel_id in &tile.panels {
                        if let Some(panel) = context.panels.iter().find(|panel| &panel.id == panel_id) {
                            {
                                let panel_id = panel.id.clone();
                                let activate_id = panel.id.clone();
                                let drag_id = panel.id.clone();
                                let key_id = panel.id.clone();
                                let title = panel.title.clone();
                                let is_active = tile.active.as_ref() == Some(&panel.id);
                                let tab_id = tab_dom_id(&panel.id);
                                let controlled_panel_id = panel_dom_id(&panel.id);
                                let tab_index = if is_active { "0" } else { "-1" };
                                let tab_class = if is_active { "rp-tab rp-tab-active" } else { "rp-tab" };
                                let activate_context = context.clone();
                                let mut drag_signal = context.dragging;
                                let mut drag_preview_signal = context.drop_preview;
                                let keyboard_context = context.clone();
                                let drag_tile = tile.id.clone();
                                let keyboard_tile = tile.id.clone();
                                let tab_order = tile.panels.clone();
                                rsx! {
                                    button {
                                        key: "{panel_id}",
                                        id: "{tab_id}",
                                        r#type: "button",
                                        class: "{tab_class}",
                                        role: "tab",
                                        draggable: "true",
                                        tabindex: "{tab_index}",
                                        "aria-selected": is_active,
                                        "aria-controls": "{controlled_panel_id}",
                                        title: "{title}. Drag to dock; Alt+Shift+Arrow splits; Alt+Shift+Page Up or Down moves between groups.",
                                        onclick: move |_| {
                                            mutate_layout(activate_context.clone(), |layout| layout.activate(&activate_id));
                                            activate_context.on_panel_activate.call(activate_id.clone());
                                        },
                                        ondragstart: move |_| {
                                            drag_signal.set(Some(drag_id.clone()));
                                            drag_preview_signal.set(Some(DropPreview {
                                                tile: drag_tile.clone(),
                                                zone: DockZone::Center,
                                            }));
                                        },
                                        onkeydown: move |event| {
                                            if event.modifiers().alt() && event.modifiers().shift() {
                                                let action = match event.key() {
                                                    Key::ArrowLeft => Some(KeyboardDock::Split(DockZone::Left)),
                                                    Key::ArrowRight => Some(KeyboardDock::Split(DockZone::Right)),
                                                    Key::ArrowUp => Some(KeyboardDock::Split(DockZone::Top)),
                                                    Key::ArrowDown => Some(KeyboardDock::Split(DockZone::Bottom)),
                                                    Key::PageUp => Some(KeyboardDock::Move(-1)),
                                                    Key::PageDown => Some(KeyboardDock::Move(1)),
                                                    _ => None,
                                                };
                                                let Some(action) = action else { return };
                                                event.prevent_default();
                                                mutate_layout(keyboard_context.clone(), |layout| match action {
                                                    KeyboardDock::Split(zone) => {
                                                        layout.dock_panel(&key_id, &keyboard_tile, zone)
                                                            || layout.split_tile(&keyboard_tile, zone).is_some()
                                                    }
                                                    KeyboardDock::Move(delta) => layout.move_panel_by_tile(&key_id, delta),
                                                });
                                                return;
                                            }
                                            let Some(current) = tab_order.iter().position(|panel| panel == &key_id) else {
                                                return;
                                            };
                                            let target = match event.key() {
                                                Key::ArrowLeft => (current + tab_order.len() - 1) % tab_order.len(),
                                                Key::ArrowRight => (current + 1) % tab_order.len(),
                                                Key::Home => 0,
                                                Key::End => tab_order.len() - 1,
                                                _ => return,
                                            };
                                            event.prevent_default();
                                            let target = tab_order[target].clone();
                                            mutate_layout(keyboard_context.clone(), |layout| layout.activate(&target));
                                            keyboard_context.on_panel_activate.call(target.clone());
                                            focus_tab(&target);
                                        },
                                        span { class: "rp-tab-label", "{title}" }
                                    }
                                }
                            }
                        }
                    }
                    if tile.panels.is_empty() {
                        span { class: "rp-empty-label", "Empty group" }
                    }
                }
                div { class: "rp-tile-actions",
                    button {
                        r#type: "button",
                        class: "rp-tile-action",
                        "aria-label": "Split panel group right",
                        title: "Split right (Alt+Shift+Right)",
                        onclick: move |_| {
                            mutate_layout(right_context.clone(), |layout| {
                                layout.split_active(&right_tile, DockZone::Right)
                            });
                        },
                        SplitRightIcon {}
                    }
                    button {
                        r#type: "button",
                        class: "rp-tile-action",
                        "aria-label": "Split panel group down",
                        title: "Split down (Alt+Shift+Down)",
                        onclick: move |_| {
                            mutate_layout(down_context.clone(), |layout| {
                                layout.split_active(&down_tile, DockZone::Bottom)
                            });
                        },
                        SplitDownIcon {}
                    }
                    if tile.panels.is_empty() && (context.layout)().tile_count() > 1 {
                        button {
                            r#type: "button",
                            class: "rp-tile-action",
                            "aria-label": "Close empty panel group",
                            title: "Close empty group",
                            onclick: move |_| {
                                mutate_layout(close_context.clone(), |layout| {
                                    layout.remove_empty_tile(&close_tile)
                                });
                            },
                            CloseIcon {}
                        }
                    }
                }
            }
            div { class: "rp-panel-frame",
                for panel_id in &tile.panels {
                    if let Some(panel) = context.panels.iter().find(|panel| &panel.id == panel_id) {
                        {
                            let is_active = tile.active.as_ref() == Some(&panel.id);
                            let panel_id = panel_dom_id(&panel.id);
                            let labelled_by = tab_dom_id(&panel.id);
                            rsx! {
                                div {
                                    key: "{panel.id}",
                                    id: "{panel_id}",
                                    class: "rp-panel-content {panel.class}",
                                    role: "tabpanel",
                                    tabindex: "0",
                                    hidden: !is_active,
                                    "aria-labelledby": "{labelled_by}",
                                    {panel.content.clone()}
                                }
                            }
                        }
                    }
                }
                if tile.panels.is_empty() {
                    div { class: "rp-empty-tile",
                        strong { "Empty panel group" }
                        span { "Drag a panel tab here, or close this group." }
                    }
                }
            }
            if (context.dragging)().is_some() {
                DockOverlay { tile: tile_id, preview, context }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum KeyboardDock {
    Split(DockZone),
    Move(isize),
}

#[component]
fn DockOverlay(tile: TileId, preview: Option<DropPreview>, context: RenderContext) -> Element {
    let preview_class = preview
        .as_ref()
        .map(|preview| {
            format!(
                "rp-drop-preview rp-drop-preview-{}",
                zone_name(preview.zone)
            )
        })
        .unwrap_or_else(|| "rp-drop-preview".to_owned());

    rsx! {
        div { class: "rp-dock-overlay", "aria-hidden": "true",
            for zone in [DockZone::Left, DockZone::Right, DockZone::Top, DockZone::Bottom, DockZone::Center] {
                {
                    let drop_context = context.clone();
                    let mut enter_preview_signal = context.drop_preview;
                    let mut drop_dragging_signal = context.dragging;
                    let mut drop_preview_signal = context.drop_preview;
                    let enter_tile = tile.clone();
                    let drop_tile = tile.clone();
                    rsx! {
                        div {
                            key: "{zone_name(zone)}",
                            class: "rp-drop-zone rp-drop-zone-{zone_name(zone)}",
                            ondragenter: move |event| {
                                event.prevent_default();
                                enter_preview_signal.set(Some(DropPreview {
                                    tile: enter_tile.clone(),
                                    zone,
                                }));
                            },
                            ondragover: move |event| event.prevent_default(),
                            ondrop: move |event| {
                                event.prevent_default();
                                let Some(panel) = drop_dragging_signal() else { return };
                                mutate_layout(drop_context.clone(), |layout| {
                                    layout.dock_panel(&panel, &drop_tile, zone)
                                });
                                drop_context.on_panel_activate.call(panel);
                                drop_dragging_signal.set(None);
                                drop_preview_signal.set(None);
                            },
                        }
                    }
                }
            }
            if preview.is_some() {
                div { class: "{preview_class}" }
                div { class: "rp-drop-compass",
                    DockCompassIcon {}
                }
            }
        }
    }
}

fn mutate_layout(context: RenderContext, mutation: impl FnOnce(&mut PanelLayout) -> bool) {
    let mut layout_signal = context.layout;
    let mut next = layout_signal();
    next.reconcile(&context.placements);
    if !mutation(&mut next) {
        return;
    }
    store_if_requested(context.storage_key.as_deref(), &next);
    layout_signal.set(next.clone());
    context.on_layout_change.call(next);
    context.on_resize.call(());
}

fn layout_for_registry(
    storage_key: Option<&str>,
    initial_layout: PanelLayout,
    placements: &[PanelPlacement],
) -> PanelLayout {
    let mut layout = storage_key.and_then(read_stored).unwrap_or(initial_layout);
    layout.reconcile(placements);
    layout
}

fn zone_name(zone: DockZone) -> &'static str {
    match zone {
        DockZone::Center => "center",
        DockZone::Left => "left",
        DockZone::Right => "right",
        DockZone::Top => "top",
        DockZone::Bottom => "bottom",
    }
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

fn tab_dom_id(panel: &PanelId) -> String {
    format!("rp-tab-{}", safe_id(panel.as_str()))
}

fn panel_dom_id(panel: &PanelId) -> String {
    format!("rp-panel-{}", safe_id(panel.as_str()))
}

#[cfg(feature = "web")]
fn focus_tab(panel: &PanelId) {
    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&tab_dom_id(panel)))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = element.focus();
}

#[cfg(not(feature = "web"))]
fn focus_tab(_panel: &PanelId) {}

#[cfg(feature = "web")]
fn read_stored(key: &str) -> Option<PanelLayout> {
    let value = web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()??;
    PanelLayout::decode(&value)
}

#[cfg(not(feature = "web"))]
fn read_stored(_key: &str) -> Option<PanelLayout> {
    None
}

#[cfg(feature = "web")]
fn store_if_requested(key: Option<&str>, layout: &PanelLayout) {
    let (Some(key), Some(value)) = (key, layout.encode()) else {
        return;
    };
    if let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok()?) {
        let _ = storage.set_item(key, &value);
    }
}

#[cfg(not(feature = "web"))]
fn store_if_requested(_key: Option<&str>, _layout: &PanelLayout) {}

#[cfg(feature = "web")]
fn element_span(id: &str, axis: SplitAxis) -> Option<f64> {
    let bounds = web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .get_bounding_client_rect();
    let span = match axis {
        SplitAxis::Horizontal => bounds.width(),
        SplitAxis::Vertical => bounds.height(),
    };
    (span.is_finite() && span > 0.0).then_some(span)
}

#[cfg(not(feature = "web"))]
fn element_span(_id: &str, _axis: SplitAxis) -> Option<f64> {
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

#[component]
fn SplitRightIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "3", width: "11", height: "10", rx: "1", fill: "none", stroke: "currentColor" }
            path { d: "M9 3v10", fill: "none", stroke: "currentColor" }
        }
    }
}

#[component]
fn SplitDownIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "3", width: "11", height: "10", rx: "1", fill: "none", stroke: "currentColor" }
            path { d: "M2.5 8h11", fill: "none", stroke: "currentColor" }
        }
    }
}

#[component]
fn CloseIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "m4 4 8 8m0-8-8 8", fill: "none", stroke: "currentColor", stroke_linecap: "round" }
        }
    }
}

#[component]
fn DockCompassIcon() -> Element {
    rsx! {
        svg { view_box: "0 0 28 28", "aria-hidden": "true",
            rect { x: "10", y: "10", width: "8", height: "8", rx: "1" }
            path { d: "M5 10h4v8H5zm14 0h4v8h-4zM10 5h8v4h-8zm0 14h8v4h-8z" }
        }
    }
}
