//! Choreographed motion: elements sliding to where a state change put them,
//! and tokens flying from one place to another.
//!
//! Both run on the Web Animations API, so the compositor draws the frames and
//! no Rust runs per frame. Both move `transform` and `opacity` only, both stand
//! still under `prefers-reduced-motion`, and both jump to their end when the
//! operator acts ([`Motion::finish_within`]): motion shows where things went,
//! and never makes anyone wait to see it.

use js_sys::{Array, Object, Promise, Reflect};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// web-sys gates `Element.animate` and `Element.getAnimations` behind a global
// unstable cfg. These narrow bindings reach the standard browser API without
// changing workspace flags.
#[wasm_bindgen]
extern "C" {
    /// Any element, seen as the animation target it is.
    #[wasm_bindgen(extends = web_sys::Element)]
    type Animatable;
    #[wasm_bindgen(method)]
    fn animate(this: &Animatable, keyframes: &Array, options: &Object) -> Animation;
    #[wasm_bindgen(method, js_name = getAnimations)]
    fn get_animations(this: &Animatable, options: &Object) -> Array;

    type Animation;
    #[wasm_bindgen(method, catch)]
    fn finish(this: &Animation) -> Result<(), JsValue>;
    #[wasm_bindgen(method, getter)]
    fn finished(this: &Animation) -> Promise;
}

/// How long a slide takes. Long enough to follow one row, short enough that
/// three hundred of them are over before anyone reaches for the next click.
const SLIDE_MS: f64 = 360.0;
/// How long one token is in the air.
const FLIGHT_MS: f64 = 640.0;
/// A cascade of slides never waits longer than this for its last member.
const MAX_STAGGER_MS: f64 = 420.0;
/// Points a flight's curve is drawn through; enough that the arc reads as a
/// curve rather than two straight legs.
const FLIGHT_SAMPLES: usize = 10;
/// The Studio's one easing, expo out: fast out of the gate, settled early.
/// A slide and a flight are arrivals like any other entrance.
const EASE_OUT: &str = "cubic-bezier(0.16, 1, 0.3, 1)";

/// The questions every choreography asks of the page.
pub struct Motion;

impl Motion {
    /// Whether the operator asked for less motion.
    pub fn reduced() -> bool {
        web_sys::window()
            .and_then(|window| {
                window
                    .match_media("(prefers-reduced-motion: reduce)")
                    .ok()
                    .flatten()
            })
            .is_some_and(|query| query.matches())
    }

    /// Resolve once the page shows the render the caller just caused, so a
    /// measurement taken after it sees the new arrangement. That is two
    /// frames: the framework commits its changes to the page on the first.
    pub async fn after_render() {
        for _ in 0..2 {
            Self::next_frame().await;
        }
    }

    async fn next_frame() {
        let Some(window) = web_sys::window() else {
            return;
        };
        let promise = Promise::new(&mut |resolve, reject| {
            if let Err(error) = window.request_animation_frame(&resolve) {
                let _ = reject.call1(&JsValue::NULL, &error);
            }
        });
        let _ = JsFuture::from(promise).await;
    }

    /// Jump every animation running under `element` to its end.
    pub fn finish_within(element: &web_sys::Element) {
        let options = Record::of(&[("subtree", JsValue::TRUE)]);
        for animation in element
            .unchecked_ref::<Animatable>()
            .get_animations(&options)
            .iter()
        {
            let _ = animation.unchecked_into::<Animation>().finish();
        }
    }

    /// Slide `element` from `from` to wherever it now sits, scaling on the
    /// way: the same thing arriving somewhere new, rather than a new thing.
    pub fn arrive_from(element: &web_sys::Element, from: Place) {
        let to = Place::of(element);
        if Self::reduced() || to.width <= 0.0 || to.height <= 0.0 {
            return;
        }
        let transform = format!(
            "translate({}px, {}px) scale({}, {})",
            from.left - to.left,
            from.top - to.top,
            from.width / to.width,
            from.height / to.height
        );
        let keyframes = Array::of2(
            &Record::of(&[
                ("transform", transform.into()),
                ("transformOrigin", "top left".into()),
            ]),
            &Record::of(&[
                ("transform", "none".into()),
                ("transformOrigin", "top left".into()),
            ]),
        );
        element
            .unchecked_ref::<Animatable>()
            .animate(&keyframes, &Timing::arrival());
    }
}

/// A rectangle on screen, in viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Place {
    pub fn of(element: &web_sys::Element) -> Self {
        let rect = element.get_bounding_client_rect();
        Self {
            left: rect.left(),
            top: rect.top(),
            width: rect.width(),
            height: rect.height(),
        }
    }
    pub fn center(&self) -> (f64, f64) {
        (self.left + self.width / 2.0, self.top + self.height / 2.0)
    }
    fn overlaps_rows_of(&self, other: &Self) -> bool {
        self.top < other.top + other.height && other.top < self.top + self.height
    }
    /// The part of this rectangle the window shows, top to bottom. A list
    /// taller than the screen is mostly out of view however it scrolls.
    fn within(self, window_height: f64) -> Self {
        let top = self.top.max(0.0);
        let bottom = (self.top + self.height).min(window_height);
        Self {
            top,
            height: (bottom - top).max(0.0),
            ..self
        }
    }
}

/// Where each keyed element under one container sat, taken before a state
/// change, so that after it every element can slide from there to where it
/// landed (the FLIP technique: first, last, invert, play).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement(BTreeMap<String, Place>);

impl Arrangement {
    /// The attribute that names an element across renders.
    pub const KEY: &'static str = "data-motion-key";

    /// Every element under `container` carrying [`Self::KEY`].
    pub fn measure(container: &web_sys::Element) -> Self {
        let mut places = BTreeMap::new();
        Self::each_keyed(container, |key, element| {
            places.insert(key, Place::of(element));
        });
        Self(places)
    }

    /// Slide every keyed element that moved from its measured place to its
    /// current one, `stagger_ms` apart in the order they now read. An element
    /// that is out of the container's view before and after moves without
    /// animating, so a long list costs only the rows anyone can see.
    pub fn settle(&self, container: &web_sys::Element, stagger_ms: f64) {
        if Motion::reduced() {
            return;
        }
        let window_height = web_sys::window()
            .and_then(|window| window.inner_height().ok())
            .and_then(|height| height.as_f64())
            .unwrap_or(f64::INFINITY);
        let view = Place::of(container).within(window_height);
        let mut order = 0.0;
        Self::each_keyed(container, |key, element| {
            let Some(before) = self.0.get(&key) else {
                return;
            };
            let after = Place::of(element);
            let (dx, dy) = (before.left - after.left, before.top - after.top);
            if dx.abs() < 0.5 && dy.abs() < 0.5 {
                return;
            }
            if !before.overlaps_rows_of(&view) && !after.overlaps_rows_of(&view) {
                return;
            }
            let keyframes = Array::of2(
                &Record::of(&[("transform", format!("translate({dx}px, {dy}px)").into())]),
                &Record::of(&[("transform", "none".into())]),
            );
            let delay = (order * stagger_ms).min(MAX_STAGGER_MS);
            element
                .unchecked_ref::<Animatable>()
                .animate(&keyframes, &Timing::slide(delay));
            order += 1.0;
        });
    }

    fn each_keyed(container: &web_sys::Element, mut visit: impl FnMut(String, &web_sys::Element)) {
        let Ok(found) = container.query_selector_all(&format!("[{}]", Self::KEY)) else {
            return;
        };
        for position in 0..found.length() {
            let Some(element) = found
                .item(position)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            if let Some(key) = element.get_attribute(Self::KEY) {
                visit(key, &element);
            }
        }
    }
}

/// One token flying across the page along a curve: from one point, bending
/// toward a second, landing on a third.
#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    from: (f64, f64),
    via: (f64, f64),
    to: (f64, f64),
    delay_ms: f64,
    class: String,
    style: String,
}

impl Flight {
    /// A flight between viewport points. `via` pulls the curve toward itself
    /// the way a quadratic Bézier's control point does.
    pub fn arc(from: (f64, f64), via: (f64, f64), to: (f64, f64)) -> Self {
        Self {
            from,
            via,
            to,
            delay_ms: 0.0,
            class: String::new(),
            style: String::new(),
        }
    }
    /// Leave `delay_ms` after launch.
    pub fn after(mut self, delay_ms: f64) -> Self {
        self.delay_ms = delay_ms;
        self
    }
    /// What the token looks like: its class and inline style.
    pub fn token(mut self, class: &str, style: &str) -> Self {
        self.class = class.to_owned();
        self.style = style.to_owned();
        self
    }
    /// Draw the token in `layer`, an element positioned over the path, and
    /// remove it when it lands.
    pub fn launch(self, layer: &web_sys::Element) {
        if Motion::reduced() {
            return;
        }
        let Some(token) = layer
            .owner_document()
            .and_then(|document| document.create_element("span").ok())
        else {
            return;
        };
        let origin = Place::of(layer);
        let _ = token.set_attribute("class", &self.class);
        let _ = token.set_attribute("aria-hidden", "true");
        let _ = token.set_attribute(
            "style",
            &format!(
                "{};position:absolute;left:{}px;top:{}px;pointer-events:none",
                self.style,
                self.from.0 - origin.left,
                self.from.1 - origin.top
            ),
        );
        let _ = layer.append_child(&token);
        let keyframes = Array::new();
        for sample in 0..=FLIGHT_SAMPLES {
            let t = sample as f64 / FLIGHT_SAMPLES as f64;
            let (x, y) = self.at(t);
            // It rises a little leaving and shrinks into its landing, so a
            // hundred of them arriving read as filling something up.
            let scale = 1.0 + 0.35 * (std::f64::consts::PI * t).sin() - 0.45 * t * t;
            keyframes.push(&Record::of(&[
                (
                    "transform",
                    format!(
                        "translate(-50%, -50%) translate({}px, {}px) scale({scale})",
                        x - self.from.0,
                        y - self.from.1
                    )
                    .into(),
                ),
                ("opacity", (if t < 1.0 { 1.0 } else { 0.0 }).into()),
                ("offset", t.into()),
            ]));
        }
        let flight = token
            .unchecked_ref::<Animatable>()
            .animate(&keyframes, &Timing::flight(self.delay_ms));
        let landed = flight.finished();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = JsFuture::from(landed).await;
            token.remove();
        });
    }
    /// The point `t` of the way along the curve.
    fn at(&self, t: f64) -> (f64, f64) {
        let along = |from: f64, via: f64, to: f64| {
            (1.0 - t) * (1.0 - t) * from + 2.0 * (1.0 - t) * t * via + t * t * to
        };
        (
            along(self.from.0, self.via.0, self.to.0),
            along(self.from.1, self.via.1, self.to.1),
        )
    }
}

/// A small plain object as the browser reads one: a keyframe, or the
/// options of a call.
struct Record;

impl Record {
    fn of(properties: &[(&str, JsValue)]) -> Object {
        let object = Object::new();
        for (name, value) in properties {
            let _ = Reflect::set(&object, &JsValue::from_str(name), value);
        }
        object
    }
}

/// When each kind of animation runs and how it eases.
struct Timing;

impl Timing {
    /// A cascade member holds its old place while it waits its turn.
    fn slide(delay_ms: f64) -> Object {
        Self::record(SLIDE_MS, delay_ms, EASE_OUT, "backwards")
    }
    /// One element carried from where it was to where it is.
    fn arrival() -> Object {
        Self::record(SLIDE_MS * 1.4, 0.0, EASE_OUT, "backwards")
    }
    /// A token keeps its landing until it is removed, so it never flashes
    /// back to where it took off.
    fn flight(delay_ms: f64) -> Object {
        Self::record(FLIGHT_MS, delay_ms, EASE_OUT, "both")
    }
    fn record(duration_ms: f64, delay_ms: f64, easing: &str, fill: &str) -> Object {
        Record::of(&[
            ("duration", duration_ms.into()),
            ("delay", delay_ms.into()),
            ("easing", easing.into()),
            ("fill", fill.into()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flight_starts_where_it_left_bends_toward_via_and_lands() {
        let flight = Flight::arc((0.0, 100.0), (50.0, 0.0), (100.0, 100.0));
        assert_eq!(flight.at(0.0), (0.0, 100.0));
        assert_eq!(flight.at(1.0), (100.0, 100.0));
        let (x, y) = flight.at(0.5);
        assert_eq!(x, 50.0);
        assert!(y < 100.0, "the curve rises toward its control point");
    }

    #[test]
    fn places_overlap_only_when_their_rows_do() {
        let view = Place {
            left: 0.0,
            top: 100.0,
            width: 10.0,
            height: 100.0,
        };
        let inside = Place { top: 150.0, ..view };
        let below = Place { top: 400.0, ..view };
        assert!(inside.overlaps_rows_of(&view));
        assert!(!below.overlaps_rows_of(&view));
        let tall = Place {
            top: -500.0,
            height: 5_000.0,
            ..view
        };
        let shown = tall.within(900.0);
        assert_eq!((shown.top, shown.height), (0.0, 900.0));
        assert!(
            !Place {
                top: 1_200.0,
                ..view
            }
            .overlaps_rows_of(&shown)
        );
    }
}
