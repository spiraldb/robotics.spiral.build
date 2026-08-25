//! One browser lifecycle for every Charming / ECharts visualization.
//!
//! Charming owns the typed option model and initial render. This component
//! keeps that renderer out of domain panels, coalesces later updates, follows
//! docked-panel resizes, and disposes the canvas when a run tab closes.

use charming::Chart;
use charming::element::{
    AxisPointer, AxisPointerType, JsFunction, LineStyle, Padding, Tooltip, Trigger,
};
use dioxus::prelude::*;

pub(crate) const COLOR_LINE: &str = "#e8e8e6";
#[cfg(feature = "web")]
pub(crate) const COLOR_INK_SOFT: &str = "#65645f";
pub(crate) const COLOR_INK_FAINT: &str = "#71706c";
pub(crate) const COLOR_SURFACE: &str = "#ffffff";
pub(crate) const COLOR_SELECTED: &str = "rgba(8, 104, 184, 0.10)";
pub(crate) const COLOR_TRAIN: &str = "#2a78d6";
pub(crate) const COLOR_VALIDATION: &str = "#d9572b";
pub(crate) const COLOR_THROUGHPUT: &str = "#3a7654";
pub(crate) const COLOR_TRANSFER: &str = "#7560b8";
pub(crate) const COLOR_CPU: &str = "#d9572b";
pub(crate) const COLOR_GPU: &str = "#2a78d6";
pub(crate) const COLOR_MEMORY: &str = "#7a58a5";

pub(crate) const MONO_FONT: &str = "ui-monospace, SF Mono, JetBrains Mono, Menlo, monospace";

#[cfg(feature = "web")]
const STUDIO_TOOLTIP_CSS: &str =
    "border-radius: 4px; box-shadow: 0 1px 3px rgba(55, 53, 47, 0.10);";

#[derive(Clone, Copy)]
pub(crate) enum TooltipRateUnit {
    SamplesPerSecond,
    BytesPerSecond,
}

pub(crate) struct StudioTooltip;

impl StudioTooltip {
    pub(crate) fn numeric(maximum_fraction_digits: u8) -> Tooltip {
        let maximum_fraction_digits = maximum_fraction_digits.min(6);
        let formatter = JsFunction::new_with_args(
            "value",
            &format!(
                r#"
const raw = Array.isArray(value) ? value[value.length - 1] : value;
if (raw === null || raw === undefined || raw === '') return '—';
const numeric = Number(raw);
if (!Number.isFinite(numeric)) return '—';
const magnitude = Math.abs(numeric);
const options = magnitude > 0 && magnitude < 0.001
  ? {{ maximumSignificantDigits: 3 }}
  : {{ maximumFractionDigits: {maximum_fraction_digits} }};
return numeric.toLocaleString(undefined, options);
"#
            ),
        );
        Self::axis(formatter)
    }

    pub(crate) fn rate(unit: TooltipRateUnit) -> Tooltip {
        let body = match unit {
            TooltipRateUnit::SamplesPerSecond => {
                r#"
const raw = Array.isArray(value) ? value[value.length - 1] : value;
const numeric = Number(raw);
if (!Number.isFinite(numeric)) return '—';
return numeric.toLocaleString(undefined, { maximumFractionDigits: 2 }) + ' samples/s';
"#
            }
            TooltipRateUnit::BytesPerSecond => {
                r#"
const raw = Array.isArray(value) ? value[value.length - 1] : value;
const numeric = Number(raw);
if (!Number.isFinite(numeric)) return '—';
const gib = 1073741824;
const mib = 1048576;
const large = Math.abs(numeric) >= gib;
const scaled = numeric / (large ? gib : mib);
return scaled.toLocaleString(undefined, { maximumFractionDigits: 2 }) + (large ? ' GiB/s' : ' MiB/s');
"#
            }
        };
        Self::single_series(body)
    }

    pub(crate) fn percent() -> Tooltip {
        Self::single_series(
            r#"
const raw = Array.isArray(value) ? value[value.length - 1] : value;
const numeric = Number(raw);
if (!Number.isFinite(numeric)) return '—';
return numeric.toLocaleString(undefined, { maximumFractionDigits: 1 }) + '%';
"#,
        )
    }

    pub(crate) fn bytes() -> Tooltip {
        Self::single_series(
            r#"
const raw = Array.isArray(value) ? value[value.length - 1] : value;
const numeric = Number(raw);
if (!Number.isFinite(numeric)) return '—';
const gib = 1073741824;
const mib = 1048576;
const large = Math.abs(numeric) >= gib;
const scaled = numeric / (large ? gib : mib);
return scaled.toLocaleString(undefined, { maximumFractionDigits: 2 }) + (large ? ' GiB' : ' MiB');
"#,
        )
    }

    fn single_series(value_formatter_body: &str) -> Tooltip {
        let popup_formatter_body = format!(
            r#"
const point = Array.isArray(params)
  ? params.find((candidate) => candidate && candidate.value !== null && candidate.value !== undefined)
  : params;
const value = point ? point.value : null;
{value_formatter_body}
"#
        );
        Self::axis(JsFunction::new_with_args("value", value_formatter_body))
            .formatter(JsFunction::new_with_args("params", &popup_formatter_body))
    }

    fn axis(value_formatter: JsFunction) -> Tooltip {
        Tooltip::new()
            .trigger(Trigger::Axis)
            .axis_pointer(
                AxisPointer::new()
                    .type_(AxisPointerType::Line)
                    .snap(false)
                    .animation(false)
                    .line_style(
                        LineStyle::new()
                            .color(COLOR_INK_FAINT)
                            .width(1)
                            .opacity(0.3),
                    ),
            )
            .background_color(COLOR_SURFACE)
            .border_color(COLOR_LINE)
            .border_width(1)
            .padding(Padding::Double(4.0, 6.0))
            .value_formatter(value_formatter)
    }
}

pub(crate) trait ChartPoints {
    /// Preserve short-lived peaks while bounding the Rust-to-JavaScript payload.
    /// The first and last samples stay fixed; each interior bucket contributes its
    /// extrema in source order so the reduced line never invents a reversal.
    fn downsample_extrema(&self, budget: usize) -> Vec<(f64, f64)>;
}

impl ChartPoints for [(f64, f64)] {
    fn downsample_extrema(&self, budget: usize) -> Vec<(f64, f64)> {
        if self.len() <= budget || budget < 4 {
            return self.to_vec();
        }

        let interior = &self[1..self.len() - 1];
        let bucket_count = ((budget - 2) / 2).max(1);
        let bucket_size = interior.len().div_ceil(bucket_count);
        let mut reduced = Vec::with_capacity(budget + 2);
        reduced.push(self[0]);

        for bucket in interior.chunks(bucket_size) {
            let min = bucket
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| left.1.total_cmp(&right.1));
            let max = bucket
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| left.1.total_cmp(&right.1));
            match (min, max) {
                (Some((min_index, min)), Some((max_index, max))) if min_index <= max_index => {
                    reduced.push(*min);
                    if min_index != max_index {
                        reduced.push(*max);
                    }
                }
                (Some((_, min)), Some((_, max))) => {
                    reduced.push(*max);
                    reduced.push(*min);
                }
                _ => {}
            }
        }

        reduced.push(self[self.len() - 1]);
        reduced
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChartPresentation {
    class: String,
    aria_label: String,
    group: Option<String>,
}

impl ChartPresentation {
    pub(crate) fn new(class: impl Into<String>, aria_label: impl Into<String>) -> Self {
        Self {
            class: class.into(),
            aria_label: aria_label.into(),
            group: None,
        }
    }

    pub(crate) fn with_group(mut self, group: String) -> Self {
        self.group = Some(group);
        self
    }
}

#[component]
pub(crate) fn CharmingChart(id: String, chart: Chart, presentation: ChartPresentation) -> Element {
    let ChartPresentation {
        class,
        aria_label,
        group,
    } = presentation;
    #[cfg(not(feature = "web"))]
    let _ = &group;
    let runtime_failed = use_signal(|| false);

    #[cfg(feature = "web")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use charming::Echarts;

        let instance = use_hook(|| Rc::new(RefCell::new(None::<Echarts>)));
        let observer = use_hook(|| Rc::new(RefCell::new(None::<ChartResizeObserver>)));

        let update_instance = Rc::clone(&instance);
        let update_observer = Rc::clone(&observer);
        let mut runtime_status = runtime_failed;
        use_effect(use_reactive(
            (&id, &chart, &group),
            move |(id, chart, group)| {
                let instance = Rc::clone(&update_instance);
                let observer = Rc::clone(&update_observer);
                spawn(async move {
                    for _ in 0..100 {
                        if EchartsRuntime::ready() {
                            match (ChartRenderRequest {
                                id: &id,
                                chart: &chart,
                                group: group.as_deref(),
                                instance: &instance,
                                observer: &observer,
                            })
                            .render()
                            {
                                Ok(()) => runtime_status.set(false),
                                Err(()) => runtime_status.set(true),
                            }
                            return;
                        }
                        gloo_timers::future::TimeoutFuture::new(20).await;
                    }
                    runtime_status.set(true);
                });
            },
        ));

        let cleanup_id = id.clone();
        let cleanup_instance = Rc::clone(&instance);
        let cleanup_observer = Rc::clone(&observer);
        use_drop(move || {
            cleanup_observer.borrow_mut().take();
            if let Some(chart) = cleanup_instance.borrow_mut().take() {
                chart.dispose();
            } else if EchartsRuntime::ready()
                && let Some(chart) = EchartsRuntime::existing(&cleanup_id)
            {
                chart.dispose();
            }
        });
    }

    rsx! {
        div {
            id,
            class: "charming-chart {class}",
            role: "img",
            "aria-label": aria_label,
            if runtime_failed() {
                span { class: "charming-chart-error", role: "alert", "Chart unavailable." }
            }
        }
    }
}

#[cfg(feature = "web")]
struct EchartsRuntime;

#[cfg(feature = "web")]
impl EchartsRuntime {
    fn ready() -> bool {
        use wasm_bindgen::JsValue;

        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("echarts"))
            .ok()
            .is_some_and(|runtime| !runtime.is_null() && !runtime.is_undefined())
    }

    fn existing(id: &str) -> Option<charming::Echarts> {
        use wasm_bindgen::JsCast as _;
        use wasm_bindgen::JsValue;

        let element = web_sys::window()?.document()?.get_element_by_id(id)?;
        let runtime =
            js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("echarts")).ok()?;
        let getter = js_sys::Reflect::get(&runtime, &JsValue::from_str("getInstanceByDom"))
            .ok()?
            .dyn_into::<js_sys::Function>()
            .ok()?;
        getter
            .call1(&runtime, element.as_ref())
            .ok()?
            .dyn_into()
            .ok()
    }
}

#[cfg(feature = "web")]
struct ChartRenderRequest<'a> {
    id: &'a str,
    chart: &'a Chart,
    group: Option<&'a str>,
    instance: &'a std::rc::Rc<std::cell::RefCell<Option<charming::Echarts>>>,
    observer: &'a std::rc::Rc<std::cell::RefCell<Option<ChartResizeObserver>>>,
}

#[cfg(feature = "web")]
impl ChartRenderRequest<'_> {
    fn render(self) -> Result<(), ()> {
        if let Some(existing) = self.instance.borrow().as_ref() {
            existing.update(self.chart);
            existing.connect(self.group)?;
            return Ok(());
        }

        let rendered = charming::WasmRenderer::new_opt(None, None)
            .render(self.id, self.chart)
            .map_err(|_| ())?;
        rendered.apply_studio_tooltip_theme();
        rendered.connect(self.group)?;
        self.observer.borrow_mut().replace(ChartResizeObserver::new(
            self.id,
            std::rc::Rc::clone(self.instance),
        )?);
        self.instance.borrow_mut().replace(rendered);
        Ok(())
    }
}

#[cfg(feature = "web")]
trait StudioEcharts {
    fn connect(&self, group: Option<&str>) -> Result<(), ()>;
    fn update(&self, option: &Chart);
    fn apply_studio_tooltip_theme(&self);
    fn preserve_data_zoom(&self, option: &wasm_bindgen::JsValue);
    fn dispose(&self);
    fn call_method(
        &self,
        method: &str,
        arguments: &[wasm_bindgen::JsValue],
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

#[cfg(feature = "web")]
impl StudioEcharts for charming::Echarts {
    fn connect(&self, group: Option<&str>) -> Result<(), ()> {
        use wasm_bindgen::JsCast as _;
        use wasm_bindgen::JsValue;

        let Some(group) = group else {
            return Ok(());
        };
        let group = JsValue::from_str(group);
        js_sys::Reflect::set(self.as_ref(), &JsValue::from_str("group"), &group).map_err(|_| ())?;
        let runtime = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("echarts"))
            .map_err(|_| ())?;
        let connect = js_sys::Reflect::get(&runtime, &JsValue::from_str("connect"))
            .map_err(|_| ())?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| ())?;
        connect.call1(&runtime, &group).map_err(|_| ())?;
        Ok(())
    }

    fn update(&self, option: &Chart) {
        use wasm_bindgen::JsValue;

        let settings = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&settings, &JsValue::from_str("notMerge"), &JsValue::TRUE);
        let _ = js_sys::Reflect::set(&settings, &JsValue::from_str("lazyUpdate"), &JsValue::TRUE);
        let _ = js_sys::Reflect::set(&settings, &JsValue::from_str("silent"), &JsValue::TRUE);
        if let Ok(option) = serde_wasm_bindgen::to_value(option) {
            self.preserve_data_zoom(&option);
            let _ = self.call_method("setOption", &[option, settings.into()]);
            self.apply_studio_tooltip_theme();
        }
    }

    fn apply_studio_tooltip_theme(&self) {
        use wasm_bindgen::JsValue;

        let option = js_sys::Object::new();
        let tooltip = js_sys::Object::new();
        let text_style = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &text_style,
            &JsValue::from_str("color"),
            &JsValue::from_str(COLOR_INK_SOFT),
        );
        let _ = js_sys::Reflect::set(
            &text_style,
            &JsValue::from_str("fontSize"),
            &JsValue::from_f64(12.0),
        );
        let _ = js_sys::Reflect::set(
            &text_style,
            &JsValue::from_str("fontWeight"),
            &JsValue::from_f64(400.0),
        );
        let _ = js_sys::Reflect::set(
            &text_style,
            &JsValue::from_str("lineHeight"),
            &JsValue::from_f64(16.0),
        );
        let _ = js_sys::Reflect::set(
            &tooltip,
            &JsValue::from_str("textStyle"),
            &text_style.into(),
        );
        let _ = js_sys::Reflect::set(
            &tooltip,
            &JsValue::from_str("extraCssText"),
            &JsValue::from_str(STUDIO_TOOLTIP_CSS),
        );
        let _ = js_sys::Reflect::set(&option, &JsValue::from_str("tooltip"), &tooltip.into());
        let _ = self.call_method("setOption", &[option.into()]);
    }

    fn preserve_data_zoom(&self, option: &wasm_bindgen::JsValue) {
        use wasm_bindgen::JsValue;

        let key = JsValue::from_str("dataZoom");
        let Ok(next_zoom) = js_sys::Reflect::get(option, &key) else {
            return;
        };
        if next_zoom.is_null() || next_zoom.is_undefined() {
            return;
        }
        let Ok(current) = self.call_method("getOption", &[]) else {
            return;
        };
        let Ok(current_zoom) = js_sys::Reflect::get(&current, &key) else {
            return;
        };
        if current_zoom.is_null() || current_zoom.is_undefined() {
            return;
        }
        if js_sys::Array::is_array(&next_zoom) && js_sys::Array::is_array(&current_zoom) {
            let next_zoom = js_sys::Array::from(&next_zoom);
            let current_zoom = js_sys::Array::from(&current_zoom);
            if next_zoom.length() != current_zoom.length() {
                return;
            }
        }
        let _ = js_sys::Reflect::set(option, &key, &current_zoom);
    }

    fn dispose(&self) {
        let _ = self.call_method("dispose", &[]);
    }

    fn call_method(
        &self,
        method: &str,
        arguments: &[wasm_bindgen::JsValue],
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
        use wasm_bindgen::JsCast as _;
        use wasm_bindgen::JsValue;

        let this: &JsValue = self.as_ref();
        let function = js_sys::Reflect::get(this, &JsValue::from_str(method))?
            .dyn_into::<js_sys::Function>()?;
        let arguments = arguments.iter().collect::<js_sys::Array>();
        function.apply(this, &arguments)
    }
}

#[cfg(feature = "web")]
struct ChartResizeObserver {
    observer: web_sys::ResizeObserver,
    _callback: wasm_bindgen::closure::Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
}

#[cfg(feature = "web")]
impl ChartResizeObserver {
    fn new(
        id: &str,
        instance: std::rc::Rc<std::cell::RefCell<Option<charming::Echarts>>>,
    ) -> Result<Self, ()> {
        use wasm_bindgen::JsCast as _;

        let element = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(id))
            .ok_or(())?;
        let resize_element = element.clone();
        let callback = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |_entries: js_sys::Array, _observer: web_sys::ResizeObserver| {
                let rect = resize_element.get_bounding_client_rect();
                let width = rect.width().round().max(0.0) as u32;
                let height = rect.height().round().max(0.0) as u32;
                if width == 0 || height == 0 {
                    return;
                }
                if let Some(chart) = instance.borrow().as_ref() {
                    charming::WasmRenderer::resize_chart(
                        chart,
                        charming::ChartResize::new(width, height, true, None),
                    );
                }
            },
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>);
        let observer =
            web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).map_err(|_| ())?;
        observer.observe(&element);
        Ok(Self {
            observer,
            _callback: callback,
        })
    }
}

#[cfg(feature = "web")]
impl Drop for ChartResizeObserver {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_formatters_bound_precision_and_keep_single_series_popups_to_one_value() {
        let numeric = serde_json::to_value(StudioTooltip::numeric(3))
            .expect("numeric tooltip should serialize");
        let numeric_formatter = numeric["valueFormatter"]
            .as_str()
            .expect("numeric tooltip should contain a formatter");
        assert!(numeric_formatter.contains("maximumFractionDigits: 3"));
        assert_eq!(numeric["padding"], serde_json::json!([4.0, 6.0]));
        assert_eq!(numeric["borderColor"], COLOR_LINE);
        assert_eq!(numeric["axisPointer"]["lineStyle"]["opacity"], 0.3);
        assert!(numeric["formatter"].is_null());

        let byte_rate = serde_json::to_value(StudioTooltip::rate(TooltipRateUnit::BytesPerSecond))
            .expect("rate tooltip should serialize");
        let byte_formatter = byte_rate["valueFormatter"]
            .as_str()
            .expect("rate tooltip should contain a formatter");
        assert!(byte_formatter.contains("GiB/s"));
        assert!(byte_formatter.contains("MiB/s"));
        let byte_popup_formatter = byte_rate["formatter"]
            .as_str()
            .expect("single-series tooltip should contain a popup formatter");
        assert!(byte_popup_formatter.contains("point.value"));
        assert!(!byte_popup_formatter.contains("seriesName"));
        assert!(!byte_popup_formatter.contains("axisValue"));
        assert!(!byte_popup_formatter.contains("<br"));
    }
}
