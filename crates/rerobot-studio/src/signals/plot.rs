//! Painting the signals panel, and reading the operator's gestures back out.
//!
//! The chart is a description built fresh in Rust and handed over; the renderer
//! is never asked to remember anything the studio does not also know. That is
//! what makes the two plots agree: they are one instance with one time axis and
//! one zoom, so a leader and a follower cannot drift a pixel apart.
//!
//! Redrawing is paced here rather than by the telemetry stream. Samples arrive
//! twenty times a second and a canvas repainted that often for a line that
//! moved two pixels is heat, not information — so the loop wakes on its own
//! clock and skips entirely when nothing it draws has changed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use charming::component::{Axis, DataZoom, DataZoomType, FilterMode, Grid, Title};
use charming::datatype::CompositeValue;
use charming::element::{
    AxisLabel, AxisLine, AxisPointer, AxisPointerLink, AxisPointerType, AxisTick, AxisType, Color,
    LineStyle, SplitLine, TextStyle, Tooltip, Trigger,
};
use charming::series::Line;
use charming::{Chart, WasmRenderer};
use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::{
    CANVAS_ID, POINT_BUDGET, SignalStore, ViewWindow, decimate, nice_range, shared_bounds,
    signal_color, time_ticks,
};
use crate::state::Studio;

/// How often the loop considers repainting.
const FRAME_MS: u32 = 90;

/// Vertical budget, in CSS pixels.
const TITLE_BAND: f64 = 15.0;
const AXIS_BAND: f64 = 17.0;
const SLIDER_BAND: f64 = 32.0;
/// Small enough that two arms, their time axis, and the zoom all fit the
/// shortest region the layout allows — a clipped zoom control reads as broken
/// even when the region behind it scrolls.
const PLOT_MIN: f64 = 52.0;
const PLOT_MAX: f64 = 190.0;
const GUTTER_LEFT: f64 = 38.0;
const GUTTER_RIGHT: f64 = 8.0;

/// Gridlines a plot this tall can carry without its own labels crowding out
/// the trace they are there to measure.
fn divisions(plot: f64) -> u32 {
    match plot {
        height if height >= 130.0 => 4,
        height if height >= 88.0 => 3,
        _ => 2,
    }
}

/// The shell tokens this canvas needs, read from the stylesheet that declares
/// them rather than copied out of it.
///
/// A chart painted into a canvas cannot inherit a CSS variable, so the values
/// have to arrive as strings — but they do not have to arrive as a *second*
/// spelling. These are resolved from `:root` on every rebuild, so a token moved
/// in `tailwind.css` moves here too, and the fallbacks below are a stylesheet
/// that failed to load, not a copy of one that did.
struct Tones {
    ink_faint: String,
    ink_soft: String,
    line: String,
    sunken: String,
    accent: String,
    sans: String,
    mono: String,
}

impl Tones {
    fn read() -> Self {
        let root = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element());
        let style = root
            .as_ref()
            .and_then(|root| web_sys::window()?.get_computed_style(root).ok()?);
        let tone = |name: &str, fallback: &str| -> String {
            style
                .as_ref()
                .and_then(|style| style.get_property_value(name).ok())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        Self {
            ink_faint: tone("--tone-ink-faint", "#71706c"),
            ink_soft: tone("--tone-ink-soft", "#65645f"),
            line: tone("--tone-line", "#e8e8e6"),
            sunken: tone("--tone-sunken", "#f7f7f5"),
            accent: tone("--tone-accent", "#0868b8"),
            // The body face is the theme's default rather than a named token,
            // so it is read off the element that wears it.
            sans: style
                .as_ref()
                .and_then(|style| style.get_property_value("font-family").ok())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "system-ui, -apple-system, sans-serif".to_owned()),
            mono: tone(
                "--font-mono",
                "ui-monospace, SF Mono, JetBrains Mono, Menlo, monospace",
            ),
        }
    }
}

#[derive(Default)]
struct Painter {
    chart: Option<charming::Echarts>,
    /// Held for as long as the instance is: a dropped closure is a dead
    /// callback, and ECharts would keep calling it.
    listeners: Vec<Closure<dyn FnMut(JsValue)>>,
    /// What the instance was built for. A change here means the shape of the
    /// chart changed, not just its data, so it is rebuilt rather than merged —
    /// a merge cannot take a series away.
    signature: String,
    size: (u32, u32),
    drawn: Option<DrawState>,
}

/// Everything the last paint depended on, so an unchanged studio costs nothing.
#[derive(Clone, Debug, PartialEq)]
struct DrawState {
    samples: usize,
    window: ViewWindow,
    hidden: Vec<String>,
    /// The joint the readout is pointing at, which lifts its trace.
    pointed: Option<String>,
    size: (u32, u32),
}

impl Painter {
    fn dispose(&mut self) {
        if let Some(chart) = self.chart.take() {
            chart.unchecked_ref::<EchartsInstance>().dispose();
        }
        self.listeners.clear();
        self.signature.clear();
        self.drawn = None;
    }
}

#[wasm_bindgen]
extern "C" {
    /// The bits of an ECharts instance charming does not surface.
    ///
    /// Reached by unchecked cast rather than by an `instanceof` test, so no
    /// constructor is ever looked up on the page.
    #[wasm_bindgen(js_name = Object)]
    type EchartsInstance;

    #[wasm_bindgen(method, js_name = "on")]
    fn on(this: &EchartsInstance, event: &str, handler: &js_sys::Function);

    #[wasm_bindgen(method, js_name = "dispose")]
    fn dispose(this: &EchartsInstance);
}

/// Run the paint loop for as long as the panel is mounted.
pub(super) fn use_plot(
    studio: Studio,
    window: Signal<ViewWindow>,
    hidden: Signal<Vec<String>>,
    pointed: Signal<Option<String>>,
    cursor: Signal<Option<f32>>,
    mut ready: Signal<bool>,
) {
    let painter = use_hook(|| Rc::new(RefCell::new(Painter::default())));
    // The window in seconds the instance was last given, so a zoom reported as
    // a percentage can be turned back into the seconds the studio thinks in.
    let extent = use_hook(|| Rc::new(Cell::new((0.0f32, 1.0f32))));

    use_future({
        let painter = painter.clone();
        let extent = extent.clone();
        move || {
            let painter = painter.clone();
            let extent = extent.clone();
            async move {
                // The runtime is a separate script and may still be arriving.
                // Silence for a couple of seconds is loading; silence after
                // that is a failure, and the panel says so rather than showing
                // an empty rectangle where two plots belong.
                let mut waited = 0u32;
                loop {
                    gloo_timers::future::TimeoutFuture::new(FRAME_MS).await;
                    if echarts_available() {
                        if !*ready.peek() {
                            ready.set(true);
                        }
                    } else {
                        waited = waited.saturating_add(FRAME_MS);
                        if waited > 2_500 && *ready.peek() {
                            ready.set(false);
                        }
                        continue;
                    }
                    paint(&painter, &extent, studio, window, hidden, pointed, cursor);
                }
            }
        }
    });

    use_drop({
        let painter = painter.clone();
        move || painter.borrow_mut().dispose()
    });
}

fn paint(
    painter: &Rc<RefCell<Painter>>,
    extent_cell: &Rc<Cell<(f32, f32)>>,
    studio: Studio,
    window: Signal<ViewWindow>,
    hidden: Signal<Vec<String>>,
    pointed: Signal<Option<String>>,
    mut cursor: Signal<Option<f32>>,
) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(element) = document.get_element_by_id(CANVAS_ID) else {
        // The panel is collapsed or empty. Let go of the instance rather than
        // leaving one bound to an element that is no longer on the page.
        painter.borrow_mut().dispose();
        return;
    };

    // Borrowed, never cloned: at the cap this is a megabyte of samples, and
    // copying it ten times a second to draw six hundred points would cost more
    // than the drawing.
    let store = studio.signals.peek();
    let Some(extent) = store.extent() else {
        drop(store);
        painter.borrow_mut().dispose();
        return;
    };
    extent_cell.set(extent);

    let hidden_now = hidden.peek().clone();
    let window_now = *window.peek();
    let (start, end) = window_now.resolve(extent);
    let (lo, hi) = store.range(start, end);

    let arms = store.arms().len().max(1);
    let width = element.client_width().max(120) as f64;
    let stage = element
        .parent_element()
        .map_or(320.0, |parent| f64::from(parent.client_height().max(160)));
    let plot = ((stage - AXIS_BAND - SLIDER_BAND - TITLE_BAND * arms as f64) / arms as f64)
        .clamp(PLOT_MIN, PLOT_MAX);
    let height = (TITLE_BAND + plot) * arms as f64 + AXIS_BAND + SLIDER_BAND;
    let size = (width.round() as u32, height.round() as u32);

    let pointed_now = pointed.peek().clone();
    let state = DrawState {
        samples: store.len(),
        window: window_now,
        hidden: hidden_now.clone(),
        pointed: pointed_now.clone(),
        size,
    };
    if painter.borrow().drawn.as_ref() == Some(&state) {
        return;
    }

    let signature = signature(&store, size);
    let chart = build(
        &store,
        &hidden_now,
        pointed_now.as_deref(),
        lo,
        hi,
        extent,
        start,
        end,
        plot,
        size,
    );
    drop(store);

    let mut held = painter.borrow_mut();
    if held.signature != signature {
        held.dispose();
        let _ = element.set_attribute("style", &format!("height: {}px", size.1));
        let renderer = WasmRenderer::new(size.0, size.1);
        let Ok(instance) = renderer.render(CANVAS_ID, &chart) else {
            return;
        };
        listen(
            &instance,
            &mut held.listeners,
            window,
            cursor,
            extent_cell.clone(),
        );
        held.chart = Some(instance);
        held.signature = signature;
        held.size = size;
    } else {
        if held.size != size {
            let _ = element.set_attribute("style", &format!("height: {}px", size.1));
            if let Some(instance) = held.chart.as_ref() {
                WasmRenderer::resize_chart(
                    instance,
                    charming::ChartResize::new(size.0, size.1, true, None),
                );
            }
            held.size = size;
        }
        if let Some(instance) = held.chart.as_ref() {
            WasmRenderer::update(instance, &chart);
        }
    }
    held.drawn = Some(state);
    drop(held);

    // A cursor left pointing outside the window would read out a sample the
    // operator can no longer see.
    let parked = cursor.peek().is_some_and(|time| time < start || time > end);
    if parked {
        cursor.set(None);
    }
}

fn echarts_available() -> bool {
    web_sys::window().is_some_and(|window| {
        js_sys::Reflect::get(&window, &JsValue::from_str("echarts"))
            .map(|found| !found.is_undefined() && !found.is_null())
            .unwrap_or(false)
    })
}

/// What the chart is made of, as opposed to what it currently shows.
fn signature(store: &SignalStore, size: (u32, u32)) -> String {
    let mut signature = String::new();
    for arm in store.arms() {
        signature.push_str(&arm.id);
        for joint in &arm.joints {
            signature.push('/');
            signature.push_str(&joint.name);
        }
        signature.push(';');
    }
    // Height changes move the grids, which is a layout change rather than a
    // data change; width alone is handled by resizing in place.
    signature.push_str(&size.1.to_string());
    signature
}

#[allow(clippy::too_many_arguments)]
fn build(
    store: &SignalStore,
    hidden: &[String],
    pointed: Option<&str>,
    lo: usize,
    hi: usize,
    extent: (f32, f32),
    start: f32,
    end: f32,
    plot: f64,
    size: (u32, u32),
) -> Chart {
    let tone = Tones::read();
    let arms = store.arms();
    let count = arms.len().max(1);
    // One scale for every arm: the same angle has to sit at the same height in
    // the leader's plot and the follower's, or the comparison the panel exists
    // for is one the operator has to do by arithmetic.
    let (low, high) = shared_bounds(store, lo, hi, hidden).unwrap_or((-1.0, 1.0));
    let (axis_low, axis_high, interval) = nice_range(low, high, divisions(plot));
    // One label per ~64px of plot width, which is what a 10px monospace time
    // reads at without its neighbours touching.
    let ticks = time_ticks(
        start,
        end,
        (f64::from(size.0) / 64.0).round().max(2.0) as u32,
    );

    let mut chart = Chart::new()
        // A scope does not ease into position. Every transition here would be
        // the renderer inventing motion between two real measurements.
        .animation(false)
        .background_color(Color::from("transparent"))
        .tooltip(Tooltip::new().trigger(Trigger::None))
        .axis_pointer(
            AxisPointer::new()
                .show(true)
                .type_(AxisPointerType::Line)
                .snap(true)
                .animation(false)
                .line_style(
                    LineStyle::new()
                        .color(Color::from(tone.accent.clone()))
                        .width(1)
                        .opacity(0.5),
                )
                .link(vec![AxisPointerLink::new().x_axis_index("all")]),
        );

    for (index, arm) in arms.iter().enumerate() {
        let top = (TITLE_BAND + plot) * index as f64;
        let last = index + 1 == count;

        chart = chart
            .title(
                Title::new()
                    .text(arm.id.clone())
                    .left(CompositeValue::from(GUTTER_LEFT))
                    .top(CompositeValue::from(top))
                    .text_style(
                        TextStyle::new()
                            .color(Color::from(tone.ink_soft.clone()))
                            .font_size(10)
                            .font_family(tone.sans.clone()),
                    ),
            )
            .grid(
                Grid::new()
                    .left(CompositeValue::from(GUTTER_LEFT))
                    .right(CompositeValue::from(GUTTER_RIGHT))
                    .top(CompositeValue::from(top + TITLE_BAND))
                    .height(CompositeValue::from(plot)),
            )
            .x_axis(
                Axis::new()
                    .type_(AxisType::Value)
                    .grid_index(index as f64)
                    // The axis spans the whole take and the zoom carves the
                    // window out of it. Setting the axis to the window instead
                    // leaves the slider showing a full-width selection of
                    // itself: a control that cannot say where in the take you
                    // are, which is the one thing it is there for.
                    .min(CompositeValue::from(extent.0))
                    .max(CompositeValue::from(extent.1.max(extent.0 + 0.001)))
                    .axis_line(
                        AxisLine::new()
                            .show(true)
                            .line_style((1.0, tone.line.as_str())),
                    )
                    .axis_tick(AxisTick::new().show(false))
                    .split_line(SplitLine::new().show(false))
                    .axis_label(
                        ticks.iter().fold(
                            AxisLabel::new()
                                .show(last)
                                .color(Color::from(tone.ink_faint.clone()))
                                .font_size(10)
                                .font_family(tone.mono.clone())
                                .formatter("{value}s"),
                            |label, tick| label.custom_values(*tick),
                        ),
                    ),
            )
            .y_axis(
                Axis::new()
                    .type_(AxisType::Value)
                    .grid_index(index as f64)
                    .min(CompositeValue::from(axis_low))
                    .max(CompositeValue::from(axis_high))
                    .interval(f64::from(interval))
                    .axis_line(AxisLine::new().show(false))
                    .axis_tick(AxisTick::new().show(false))
                    .split_line(
                        SplitLine::new().show(true).line_style(
                            LineStyle::new()
                                .color(Color::from(tone.line.clone()))
                                .width(1),
                        ),
                    )
                    .axis_label(
                        AxisLabel::new()
                            .color(Color::from(tone.ink_faint.clone()))
                            .font_size(10)
                            .font_family(tone.mono.clone())
                            .formatter("{value}°"),
                    ),
            );

        for joint in &arm.joints {
            let concealed = hidden.iter().any(|name| name == &joint.name);
            // A hidden joint keeps its series and loses its samples, so the
            // chart's shape never changes under a toggle — only its contents.
            let data = if concealed {
                Vec::new()
            } else {
                decimate(store.times(), &joint.values, lo, hi, POINT_BUDGET)
                    .into_iter()
                    .map(|(time, value)| {
                        CompositeValue::from(vec![
                            CompositeValue::from(time),
                            CompositeValue::from(value),
                        ])
                    })
                    .collect()
            };

            // Pointing at a joint in the readout answers on every plot at once:
            // its trace thickens and the rest step back. Nothing is hidden and
            // nothing moves, so the answer costs no layout and no legibility —
            // which is what earns the readout the word "legend".
            let lifted = pointed == Some(joint.name.as_str());
            let (weight, opacity) = match pointed {
                Some(_) if lifted => (2.4, 1.0),
                Some(_) => (1.5, 0.28),
                None => (1.5, 1.0),
            };

            chart = chart.series(
                Line::new()
                    .name(format!("{}·{}", arm.id, joint.name))
                    .x_axis_index(index as f64)
                    .y_axis_index(index as f64)
                    .show_symbol(false)
                    .connect_nulls(false)
                    // Drawn last so a lifted trace is not crossed by the ones
                    // it was lifted above.
                    .z(if lifted { 4 } else { 2 })
                    .line_style(
                        LineStyle::new()
                            .color(Color::from(signal_color(&joint.name)))
                            .width(weight)
                            .opacity(opacity),
                    )
                    .data(data),
            );
        }
    }

    let band_top = f64::from(size.1) - SLIDER_BAND + 6.0;
    chart
        .data_zoom(
            DataZoom::new()
                .type_(DataZoomType::Inside)
                .x_axis_index(CompositeValue::from("all"))
                .filter_mode(FilterMode::None)
                .start_value(CompositeValue::from(start))
                .end_value(CompositeValue::from(end))
                .throttle(60),
        )
        .data_zoom(
            DataZoom::new()
                .type_(DataZoomType::Slider)
                .x_axis_index(CompositeValue::from("all"))
                .filter_mode(FilterMode::None)
                .start_value(CompositeValue::from(start))
                .end_value(CompositeValue::from(end))
                .realtime(true)
                .throttle(60)
                .top(CompositeValue::from(band_top))
                .bottom(CompositeValue::from(2.0))
                .left(CompositeValue::from(GUTTER_LEFT))
                .right(CompositeValue::from(GUTTER_RIGHT))
                .background_color(Color::from(tone.sunken.clone()))
                .border_color(Color::from(tone.line.clone()))
                .filler_color(Color::from("rgba(8, 104, 184, 0.16)"))
                // ECharts draws its overview shadow from the first series
                // alone, which here would be one joint of one arm standing in
                // for a whole take — a claim rather than an overview. The
                // track says where in the take the window sits; the plots say
                // what is in it.
                .data_background(blank_shadow())
                .selected_data_background(blank_shadow())
                .text_style(
                    TextStyle::new()
                        .color(Color::from(tone.ink_faint.clone()))
                        .font_size(10)
                        .font_family(tone.mono.clone()),
                ),
        )
}

fn blank_shadow() -> charming::element::DataBackground {
    charming::element::DataBackground::new()
        .line_style(LineStyle::new().opacity(0))
        .area_style(charming::element::AreaStyle::new().opacity(0))
}

/// Wire the operator's gestures back into the studio's own state.
fn listen(
    instance: &charming::Echarts,
    listeners: &mut Vec<Closure<dyn FnMut(JsValue)>>,
    mut window: Signal<ViewWindow>,
    mut cursor: Signal<Option<f32>>,
    extent: Rc<Cell<(f32, f32)>>,
) {
    let handle = instance.unchecked_ref::<EchartsInstance>();

    // Zooming or dragging the window is the operator saying "stop moving". The
    // studio holds it there until they pick a preset again, because a scope
    // that scrolled out from under a reading would be unusable.
    let zoom = Closure::<dyn FnMut(JsValue)>::new(move |payload: JsValue| {
        if let Some((start, end)) = zoom_range(&payload, extent.get()) {
            window.set(ViewWindow::Held { start, end });
        }
    });
    handle.on("datazoom", zoom.as_ref().unchecked_ref());
    listeners.push(zoom);

    // The crosshair finds the time; the readout below prints every arm at it.
    let pointer = Closure::<dyn FnMut(JsValue)>::new(move |payload: JsValue| {
        cursor.set(pointer_time(&payload));
    });
    handle.on("updateAxisPointer", pointer.as_ref().unchecked_ref());
    listeners.push(pointer);

    let out = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| cursor.set(None));
    handle.on("globalout", out.as_ref().unchecked_ref());
    listeners.push(out);
}

/// The window an ECharts zoom event describes, in seconds.
///
/// The event reports either the values themselves or the fraction of the axis
/// they cover, depending on which control produced it, so both are accepted and
/// the fraction is resolved against what has actually been captured.
fn zoom_range(payload: &JsValue, extent: (f32, f32)) -> Option<(f32, f32)> {
    let batch = js_sys::Reflect::get(payload, &JsValue::from_str("batch")).ok();
    let source = match batch {
        Some(batch) if js_sys::Array::is_array(&batch) => js_sys::Array::from(&batch).get(0),
        _ => payload.clone(),
    };
    if source.is_undefined() || source.is_null() {
        return None;
    }

    let number = |key: &str| {
        js_sys::Reflect::get(&source, &JsValue::from_str(key))
            .ok()
            .and_then(|value| value.as_f64())
    };

    if let (Some(start), Some(end)) = (number("startValue"), number("endValue")) {
        return finite_range(start as f32, end as f32);
    }
    let (first, last) = extent;
    let span = f64::from(last - first);
    let (start, end) = (number("start")?, number("end")?);
    finite_range(
        f64::from(first) as f32 + (start / 100.0 * span) as f32,
        f64::from(first) as f32 + (end / 100.0 * span) as f32,
    )
}

fn finite_range(start: f32, end: f32) -> Option<(f32, f32)> {
    (start.is_finite() && end.is_finite() && end > start).then_some((start, end))
}

/// The time the crosshair is sitting on.
fn pointer_time(payload: &JsValue) -> Option<f32> {
    let axes = js_sys::Reflect::get(payload, &JsValue::from_str("axesInfo")).ok()?;
    if !js_sys::Array::is_array(&axes) {
        return None;
    }
    let axes = js_sys::Array::from(&axes);
    for index in 0..axes.length() {
        let axis = axes.get(index);
        let dimension = js_sys::Reflect::get(&axis, &JsValue::from_str("axisDim"))
            .ok()
            .and_then(|value| value.as_string());
        if dimension.as_deref() != Some("x") {
            continue;
        }
        let value = js_sys::Reflect::get(&axis, &JsValue::from_str("value"))
            .ok()
            .and_then(|value| value.as_f64())?;
        return value.is_finite().then_some(value as f32);
    }
    None
}
