//! Two columns side by side, parted by a hairline the operator drags — or
//! moves with the arrow keys — to share the width between them.
//!
//! The caller owns the leading column's width, so a surface keeps it while
//! what the columns hold changes, and sets the first width in its stylesheet
//! as `--ui-split-lead` until the operator moves the line. Neither column is
//! let narrower than `least`, however the room changes.

use dioxus::prelude::*;

use crate::foundation::AttributeClass;

/// How far one arrow press moves the line, and one with Shift held.
const KEY_STEP: f64 = 16.0;
const SHIFTED_KEY_STEP: f64 = 64.0;

#[derive(Props, Clone, PartialEq)]
pub struct SplitColumnsProps {
    lead: Element,
    rest: Element,
    /// The leading column's width in CSS pixels, once the operator has moved
    /// the line; until then the stylesheet's `--ui-split-lead` decides it.
    lead_width: Option<f64>,
    /// The narrowest either column may be, in CSS pixels.
    least: f64,
    /// The leading column's width the operator moved the line to.
    on_resize: EventHandler<f64>,
    /// What moving the line resizes, for assistive technology, such as
    /// "Resize the episode index".
    label: String,
    #[props(extends = GlobalAttributes, extends = div)]
    attributes: Vec<Attribute>,
}

/// A drag in progress: where the pointer and the line started.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LineDrag {
    pointer_x: f64,
    lead_width: f64,
}

/// The widths the leading column may take in a room `room` wide.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LeadBounds {
    least: f64,
    most: f64,
}

impl LeadBounds {
    fn in_room(room: f64, least: f64) -> Self {
        Self {
            least,
            most: (room - least).max(least),
        }
    }
    fn hold(self, width: f64) -> f64 {
        width.clamp(self.least, self.most)
    }
}

#[component]
pub fn SplitColumns(props: SplitColumnsProps) -> Element {
    let (class, attributes) = props.attributes.with_base_class("ui-split");
    let mut room = use_signal(|| 0.0_f64);
    let mut drawn = use_signal(|| 0.0_f64);
    let mut drag = use_signal(|| None::<LineDrag>);
    let bounds = LeadBounds::in_room(room(), props.least);
    let least = props.least;
    let lead = props.lead_width.map_or(String::new(), |width| {
        format!("--ui-split-lead: {width}px; ")
    });
    let style = format!("{lead}--ui-split-least: {least}px");
    let on_resize = props.on_resize;
    rsx! {
        div { class, style, "data-dragging": drag().is_some(),
            onresize: move |event: Event<ResizeData>| {
                if let Ok(size) = event.data().get_border_box_size() && size.width != *room.peek() {
                    room.set(size.width);
                }
            },
            ..attributes,
            div { class: "ui-split-lead",
                onresize: move |event: Event<ResizeData>| {
                    if let Ok(size) = event.data().get_border_box_size() && size.width != *drawn.peek() {
                        drawn.set(size.width);
                    }
                },
                {props.lead}
            }
            div { class: "ui-split-line", role: "separator", tabindex: "0",
                "aria-orientation": "vertical", "aria-label": props.label,
                "aria-valuenow": drawn().round(), "aria-valuemin": bounds.least.round(), "aria-valuemax": bounds.most.round(),
                onpointerdown: move |event: PointerEvent| {
                    event.prevent_default();
                    drag.set(Some(LineDrag { pointer_x: event.client_coordinates().x, lead_width: *drawn.peek() }));
                },
                onkeydown: move |event: KeyboardEvent| {
                    let step = if event.modifiers().shift() { SHIFTED_KEY_STEP } else { KEY_STEP };
                    let moved = match event.key() {
                        Key::ArrowLeft => -step,
                        Key::ArrowRight => step,
                        _ => return,
                    };
                    event.prevent_default();
                    on_resize.call(bounds.hold(drawn() + moved));
                },
            }
            div { class: "ui-split-rest", {props.rest} }
            // While the line moves, one sheet over the page takes the pointer,
            // so neither column's content — a canvas, a video — swallows it.
            if let Some(started) = drag() {
                div { class: "ui-split-drag", "aria-hidden": "true",
                    onpointermove: move |event: PointerEvent| {
                        if event.held_buttons().is_empty() {
                            drag.set(None);
                            return;
                        }
                        let moved = event.client_coordinates().x - started.pointer_x;
                        on_resize.call(bounds.hold(started.lead_width + moved));
                    },
                    onpointerup: move |_| drag.set(None),
                    onpointercancel: move |_| drag.set(None),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_keeps_both_columns_at_least_their_least_width() {
        let bounds = LeadBounds::in_room(1000.0, 280.0);
        assert_eq!(bounds.hold(100.0), 280.0);
        assert_eq!(bounds.hold(500.0), 500.0);
        assert_eq!(bounds.hold(900.0), 720.0);
        // A room narrower than two columns keeps the leading one readable.
        let cramped = LeadBounds::in_room(400.0, 280.0);
        assert_eq!(cramped.hold(350.0), 280.0);
    }
}
