//! One run as its panel shows it — where it is, its loss, what it was
//! given, and how its job performs — and how Training draws a run anywhere
//! it appears.

use charming::Chart;
use charming::component::{Axis, DataZoom, DataZoomType, FilterMode, Grid};
use charming::datatype::{CompositeValue, DataFrame, DataPoint};
use charming::element::{
    AreaStyle, AxisLabel, AxisLine, AxisTick, AxisType, Emphasis, LineStyle, Sampling, SplitLine,
    Symbol,
};
use charming::series::Line;
use dioxus::prelude::*;
use spiral_robotics_common::training::{
    PublishedModel, TrainingCheckpoint, TrainingMetricPoint, TrainingModel, TrainingRun,
    TrainingRunPhase,
};
use spiral_robotics_ui::{Disclosure, Fact, FactLayout, FactList};

use super::checkpoint_models::CheckpointModels;
use super::observation::ImageSizePresentation;
use super::state::TrainingController;
use crate::components::charts::{CharmingChart, ChartColor, ChartPoints, MONO_FONT, StudioTooltip};
use crate::format::{
    ByteCountPresentation, LongDurationPresentation, LossPresentation, MillisecondPresentation,
};

const TRAINING_CHART_POINT_BUDGET: usize = 1_200;

#[component]
pub(super) fn TrainingRunDetail(
    controller: TrainingController,
    run: TrainingRun,
    checkpoints: Vec<TrainingCheckpoint>,
    /// The progress models the project published, which a progress run's
    /// checkpoints show they were published as.
    published: Vec<PublishedModel>,
    details_loaded: bool,
) -> Element {
    if !details_loaded {
        return rsx! { article { class: "training-run-detail", role: "status", "Loading run details…" } };
    }
    let progress = run.progress_percent();
    let message = run.message().to_owned();
    let logs = run.logs().to_owned();
    let progress_model = matches!(run.config().model(), TrainingModel::Progress(_));
    let message_class = if run.phase() == TrainingRunPhase::Failed {
        "training-run-message training-run-message-danger"
    } else {
        "training-run-message"
    };
    rsx! {
        article { class: "training-run-detail",
            header { class: "training-run-overview",
                div { class: "training-run-identity",
                    div {
                        h2 { "{run.config().run_name()}" }
                        span { class: run.phase_class(), "{run.phase().label()}" }
                    }
                    p { "{run.config().model().kind().label()}" }
                }
                FactList { layout: FactLayout::Strip, mono: true, class: "training-run-facts",
                    Fact { label: "Step", "{run.current_step()} / {run.config().steps()}" }
                    Fact { label: "Progress", "{progress:.1}%" }
                    Fact { label: "Checkpoints", "{checkpoints.len()}" }
                }
            }
            if let Some(message) = message {
                div { class: "{message_class}", "{message}" }
            }
            div { class: "training-run-evidence",
                section { class: "training-run-region training-run-learning-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Loss" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingLearning { run: run.clone(), checkpoints: checkpoints.clone() }
                    }
                }
                section { class: "training-run-region training-run-data-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Inputs" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingInputs { run: run.clone() }
                    }
                    // A progress model is published from its checkpoints;
                    // a policy's go to Rollout.
                    if progress_model {
                        header { class: "training-subpanel-heading training-checkpoints-heading",
                            h3 { "Checkpoints" }
                        }
                        div { class: "training-subpanel-body",
                            CheckpointModels { controller, checkpoints, published }
                        }
                    }
                }
                section { class: "training-run-region training-run-throughput-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Performance" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingPerformance { run: run.clone() }
                    }
                }
            }
            if !logs.is_empty() {
                Disclosure { class: "training-run-logs", summary: rsx! { "Log · {logs.len()} lines" },
                    pre {
                        for line in logs.iter().rev().take(20).rev() {
                            code { "{line}\n" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TrainingLearning(run: TrainingRun, checkpoints: Vec<TrainingCheckpoint>) -> Element {
    let last = run.metrics().last();
    let latest_train = last.map(|point| point.train_loss().loss_label());
    let latest_validation = run
        .metrics()
        .iter()
        .rev()
        .find_map(|point| point.validation_loss())
        .map(LossPresentation::loss_label);
    let lr = last.map(|point| format!("{:.2e}", point.learning_rate()));
    let latest_train = latest_train.unwrap_or_else(|| "—".to_owned());
    let latest_validation = latest_validation.unwrap_or_else(|| "—".to_owned());
    let lr = lr.unwrap_or_else(|| "—".to_owned());
    let chart_id = run.chart_dom_id("training-loss");
    let chart_group = run.chart_group();
    let loss_chart = run.loss_chart();
    let ticks = run.event_ticks(&checkpoints);

    rsx! {
        section { class: "training-learning",
            div { class: "training-loss-summary",
                div {
                    span { class: "training-series-dot training-series-train" }
                    small { "Train" }
                    strong { "{latest_train}" }
                }
                div {
                    span { class: "training-series-dot training-series-validation" }
                    small { "Validation" }
                    strong { "{latest_validation}" }
                }
                div {
                    small { "Learning rate" }
                    strong { "{lr}" }
                }
            }
            div { class: "training-chart-wrap",
                if run.metrics().is_empty() {
                    div { class: "training-chart-empty",
                        if run.phase().before_first_step() {
                            "{run.phase().label()}…"
                        } else if run.phase() == TrainingRunPhase::Running {
                            "Waiting for step 1…"
                        } else {
                            "No metrics."
                        }
                    }
                }
                CharmingChart {
                    id: chart_id,
                    chart: loss_chart,
                    presentation: crate::components::charts::ChartPresentation::new(
                        "training-loss-chart",
                        "Training and validation loss by optimizer step",
                    )
                    .with_group(chart_group),
                }
            }
            div { class: "training-event-strip",
                div { class: "training-event-track",
                    i { class: "training-event-axis" }
                    for tick in ticks {
                        span {
                            class: "training-event-tick {tick.class}",
                            style: "left: {tick.percent:.3}%",
                            title: "{tick.label}",
                            "aria-label": "{tick.label}",
                        }
                    }
                }
                div { class: "training-event-legend",
                    small { "Val" }
                    small { "Checkpoint" }
                    small { "Stall" }
                }
            }
        }
    }
}

/// What a run was given: the view and how its model sees it, how much data
/// that came to, and how the optimizer runs.
#[component]
fn TrainingInputs(run: TrainingRun) -> Element {
    let config = run.config();
    let input = run.input();
    let counted =
        |count: usize, noun: &str| format!("{count} {noun}{}", if count == 1 { "" } else { "s" });
    let data = format!(
        "{} · {} · {}",
        counted(input.episodes(), "episode"),
        counted(input.intervals(), "interval"),
        input.seconds().long_duration_label(),
    );
    let validation = if config.validation_split() == 0.0 {
        "Off".to_owned()
    } else {
        format!("{:.0}% · episode", config.validation_split() * 100.0)
    };
    let joints = format!(
        "State: {}\nAction: {}",
        input.state().join(", "),
        input.action().join(", ")
    );
    rsx! {
        section { class: "training-inputs",
            FactList { layout: FactLayout::Tiles, mono: true, class: "training-input-facts",
                Fact { label: "View", "{config.source().view()}" }
                Fact { label: "Dataset", "{config.source().dataset().display_name()}" }
                Fact { label: "Data", class: "training-input-wide", "{data}" }
                match config.model() {
                    TrainingModel::Act(act) => rsx! {
                        Fact { label: "Cameras", class: "training-input-wide", {act.observation().cameras().join(", ")} }
                        Fact { label: "Image size", "{act.observation().image().size_label()}" }
                        Fact { label: "Rate", "{act.observation().rate_hz()} Hz" }
                        Fact { label: "State · action", title: "{joints}",
                            "{input.state().len()} · {input.action().len()}"
                        }
                    },
                    TrainingModel::Progress(progress) => rsx! {
                        Fact { label: "Camera", "{progress.camera()}" }
                    },
                }
                Fact { label: "Batch", "{config.batch_size()} samples" }
                Fact { label: "Validation", "{validation}" }
                if let TrainingModel::Act(act) = config.model() {
                    Fact { label: "Action horizon", "{act.chunk_size()} steps" }
                }
                Fact { label: "Checkpoint interval", "{config.checkpoint_every()} steps" }
            }
        }
    }
}

#[component]
fn TrainingPerformance(run: TrainingRun) -> Element {
    let last = run.metrics().last();
    let chart_group = run.chart_group();
    let throughput = last
        .map(|point| format!("{:.1} samples/s", point.examples_per_second()))
        .unwrap_or_else(|| "—".to_owned());
    let cpu = last
        .and_then(|point| point.cpu_percent())
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let gpu = last
        .and_then(|point| point.gpu_percent())
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let memory = last
        .and_then(|point| point.memory_bytes())
        .map(ByteCountPresentation::byte_label)
        .unwrap_or_else(|| "—".to_owned());
    let loader_ms = last.and_then(|point| point.loader_ms()).millis_label();
    let device_copy_ms = last.and_then(|point| point.device_copy_ms()).millis_label();
    let model_ms = last.and_then(|point| point.model_ms()).millis_label();
    let throughput_chart = ResourceSeries::Throughput.chart(&run);
    let cpu_chart = ResourceSeries::Cpu.chart(&run);
    let gpu_chart = ResourceSeries::Gpu.chart(&run);
    let memory_chart = ResourceSeries::Memory.chart(&run);

    rsx! {
        section { class: "training-resources",
            FactList { layout: FactLayout::Strip, mono: true, class: "training-phase-timings", aria_label: "Latest optimizer step timing",
                Fact { label: "Load", title: "Batch assembly from the prepared samples.", "{loader_ms}" }
                Fact { label: "Device copy", title: "Host-to-device transfer.", "{device_copy_ms}" }
                Fact { label: "Model", title: "Forward pass, backward pass, gradient clipping, and optimizer step.", "{model_ms}" }
            }
            div { class: "training-resource-grid",
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("training-rate"), "Training rate", throughput).with_chart(throughput_chart, chart_group.clone()).with_presentation("resource-throughput", "Samples completed per second.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("cpu-utilization"), "CPU", cpu).with_chart(cpu_chart, chart_group.clone()).with_presentation("resource-cpu", "Job process CPU utilization. 100% is one fully occupied core.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("gpu-utilization"), "GPU", gpu).with_chart(gpu_chart, chart_group.clone()).with_presentation("resource-gpu", "Whole-device GPU utilization.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("memory-usage"), "Memory", memory).with_chart(memory_chart, chart_group).with_presentation("resource-memory", "Current resident memory used by the job process, including training allocations.") }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct ResourceLaneModel {
    id: String,
    label: &'static str,
    value: String,
    chart: Chart,
    group: String,
    class: &'static str,
    description: &'static str,
}

impl ResourceLaneModel {
    fn new(id: String, label: &'static str, value: String) -> Self {
        Self {
            id,
            label,
            value,
            chart: Chart::new(),
            group: String::new(),
            class: "",
            description: "",
        }
    }

    fn with_chart(mut self, chart: Chart, group: String) -> Self {
        self.chart = chart;
        self.group = group;
        self
    }

    fn with_presentation(mut self, class: &'static str, description: &'static str) -> Self {
        self.class = class;
        self.description = description;
        self
    }
}

#[component]
fn ResourceLane(lane: ResourceLaneModel) -> Element {
    let ResourceLaneModel {
        id,
        label,
        value,
        chart,
        group,
        class,
        description,
    } = lane;
    rsx! {
        div { class: "training-resource-lane {class}", title: "{description}",
            div { span { "{label}" } strong { "{value}" } }
            CharmingChart {
                id,
                chart,
                presentation: crate::components::charts::ChartPresentation::new(
                    "training-resource-chart",
                    format!("{label} by optimizer step. {description}"),
                )
                .with_group(group),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceSeries {
    Throughput,
    Cpu,
    Gpu,
    Memory,
}

/// One run, as Training draws it: what its panel and charts are called, what
/// its phase and progress look like, and the series and marks read off its
/// metrics. `TrainingRun` lives in `crates/common`, so this is an extension
/// trait rather than an inherent impl; it is one trait for one subject, not
/// one per method.
pub(super) trait TrainingPresentation {
    fn panel_id(&self) -> String;
    /// The share of its steps the run has taken, in percent.
    fn progress_percent(&self) -> f64;
    /// How far the run has got, in words.
    fn progress_readout(&self) -> String;
    fn chart_dom_id(&self, prefix: &str) -> String;
    /// Every chart of one run shares a zoom group, so dragging one step window
    /// moves them all.
    fn chart_group(&self) -> String;
    fn phase_class(&self) -> &'static str;
    /// The last step any evidence mentions, which is what the step axis spans.
    fn observed_steps(&self) -> u64;
    fn metric_series(&self, value: impl Fn(&TrainingMetricPoint) -> Option<f64>) -> DataFrame;
    fn loss_chart(&self) -> Chart;
    /// This run's checkpoints and metric events on one axis, already placed
    /// and labelled.
    fn event_ticks(&self, checkpoints: &[TrainingCheckpoint]) -> Vec<TrainingEventTick>;
}

/// One mark on the run's event strip.
pub(super) struct TrainingEventTick {
    class: &'static str,
    percent: f64,
    label: String,
}

impl TrainingPresentation for TrainingRun {
    fn panel_id(&self) -> String {
        format!("training-run-{}", self.id())
    }

    fn progress_percent(&self) -> f64 {
        let steps = self.config().steps();
        if steps == 0 {
            0.0
        } else {
            (self.current_step() as f64 / steps as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    fn progress_readout(&self) -> String {
        if self.phase().before_first_step() {
            self.message()
                .map_or_else(|| format!("{}…", self.phase().label()), str::to_owned)
        } else {
            format!("Step {} of {}", self.current_step(), self.config().steps())
        }
    }

    fn chart_dom_id(&self, prefix: &str) -> String {
        let safe = self
            .id()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        format!("{prefix}-{safe}")
    }

    fn chart_group(&self) -> String {
        self.chart_dom_id("training-step-window")
    }

    fn phase_class(&self) -> &'static str {
        match self.phase() {
            TrainingRunPhase::Uploading
            | TrainingRunPhase::Starting
            | TrainingRunPhase::Preparing
            | TrainingRunPhase::Running
            | TrainingRunPhase::Stopping => "training-phase training-phase-active",
            // A finished run is the resting state; only work in progress and
            // failure are marked.
            TrainingRunPhase::Completed
            | TrainingRunPhase::Stopped
            | TrainingRunPhase::Unavailable => "training-phase",
            TrainingRunPhase::Failed => "training-phase training-phase-failed",
        }
    }

    fn observed_steps(&self) -> u64 {
        self.metrics()
            .last()
            .map_or(self.current_step(), |point| {
                point.step().max(self.current_step())
            })
            .max(1)
    }

    fn metric_series(&self, value: impl Fn(&TrainingMetricPoint) -> Option<f64>) -> DataFrame {
        let points = self
            .metrics()
            .iter()
            .filter_map(|point| {
                let value = value(point)?;
                (value.is_finite() && value >= 0.0).then_some((point.step() as f64, value))
            })
            .collect::<Vec<_>>();
        points
            .downsample_extrema(TRAINING_CHART_POINT_BUDGET)
            .into_iter()
            .map(|(step, value)| TrainingChartStyle::point(step, Some(value)))
            .collect()
    }

    fn loss_chart(&self) -> Chart {
        let train = self.metric_series(|point| Some(point.train_loss()).filter(|loss| *loss > 0.0));
        let validation =
            self.metric_series(|point| point.validation_loss().filter(|loss| *loss > 0.0));
        let (train_color, validation_color) =
            (ChartColor::Train.css(), ChartColor::Validation.css());

        Chart::new()
            .animation(false)
            .color(vec![train_color.clone(), validation_color.clone()])
            .tooltip(StudioTooltip::numeric(4))
            .grid(
                Grid::new()
                    .left(48)
                    .right(12)
                    .top(12)
                    .bottom(50)
                    .contain_label(false),
            )
            .x_axis(
                TrainingChartStyle::axis(true)
                    .min(0)
                    .max(self.observed_steps() as f64)
                    .split_number(4),
            )
            .y_axis(TrainingChartStyle::axis(true).split_number(5))
            .data_zoom(TrainingChartStyle::slider_zoom(48.0, 12.0))
            .series(
                MetricLineSpec::new("train-loss", "Train", train_color)
                    .with_width(2.0)
                    .render(train),
            )
            .series(
                MetricLineSpec::new("validation-loss", "Validation", validation_color)
                    .with_width(1.8)
                    .render(validation),
            )
    }

    fn event_ticks(&self, checkpoints: &[TrainingCheckpoint]) -> Vec<TrainingEventTick> {
        let place = |step: u64| {
            (step as f64 / self.config().steps().max(1) as f64 * 100.0).clamp(0.0, 100.0)
        };
        checkpoints
            .iter()
            .map(|checkpoint| TrainingEventTick {
                class: "training-event-checkpoint",
                percent: place(checkpoint.step()),
                label: format!("Checkpoint saved at step {}", checkpoint.step()),
            })
            .chain(self.metrics().iter().filter_map(|point| {
                let event = point.event()?;
                Some(TrainingEventTick {
                    class: if event.contains("loader stall") {
                        "training-event-stall"
                    } else {
                        "training-event-validation"
                    },
                    percent: place(point.step()),
                    label: format!("{event} at step {}", point.step()),
                })
            }))
            .collect()
    }
}

impl ResourceSeries {
    fn chart(self, run: &TrainingRun) -> Chart {
        let (id, name, color) = match self {
            Self::Throughput => ("training-rate", "Training rate", ChartColor::Throughput),
            Self::Cpu => ("cpu-utilization", "CPU", ChartColor::Cpu),
            Self::Gpu => ("gpu-utilization", "GPU", ChartColor::Gpu),
            Self::Memory => ("memory-usage", "Memory", ChartColor::Memory),
        };
        let color = color.css();
        let data = run.metric_series(|point| match self {
            Self::Throughput => Some(point.examples_per_second()),
            Self::Cpu => point.cpu_percent(),
            Self::Gpu => point.gpu_percent(),
            Self::Memory => point.memory_bytes().map(|value| value as f64),
        });
        let tooltip = match self {
            Self::Throughput => StudioTooltip::samples_per_second(),
            Self::Cpu | Self::Gpu => StudioTooltip::percent(),
            Self::Memory => StudioTooltip::bytes(),
        };

        Chart::new()
            .animation(false)
            .color(vec![color.clone()])
            .tooltip(tooltip)
            .grid(Grid::new().left(0).right(0).top(6).bottom(40))
            .x_axis(
                TrainingChartStyle::axis(false)
                    .min(0)
                    .max(run.observed_steps() as f64),
            )
            .y_axis(match self {
                Self::Gpu => TrainingChartStyle::axis(false).min(0).max(100),
                _ => TrainingChartStyle::axis(false).min(0).scale(true),
            })
            .data_zoom(TrainingChartStyle::slider_zoom(8.0, 8.0))
            .series(
                MetricLineSpec::new(id, name, color.clone())
                    .with_width(1.5)
                    .render(data)
                    .area_style(AreaStyle::new().color(color).opacity(0.09)),
            )
    }
}

struct MetricLineSpec {
    id: &'static str,
    name: &'static str,
    color: String,
    width: f64,
}

impl MetricLineSpec {
    fn new(id: &'static str, name: &'static str, color: String) -> Self {
        Self {
            id,
            name,
            color,
            width: 1.0,
        }
    }

    const fn with_width(mut self, width: f64) -> Self {
        self.width = width;
        self
    }

    fn render(self, data: DataFrame) -> Line {
        Line::new()
            .id(self.id)
            .name(self.name)
            .show_symbol(false)
            .symbol(Symbol::None)
            .sampling(Sampling::Minmax)
            .connect_nulls(false)
            .line_style(LineStyle::new().color(self.color).width(self.width))
            .emphasis(Emphasis::new().disabled(true))
            .data(data)
    }
}

struct TrainingChartStyle;

impl TrainingChartStyle {
    fn slider_zoom(left: f64, right: f64) -> DataZoom {
        DataZoom::new()
            .type_(DataZoomType::Slider)
            .show(true)
            .x_axis_index(0)
            .filter_mode(FilterMode::Filter)
            .realtime(true)
            .throttle(50)
            .brush_select(false)
            .left(left)
            .right(right)
            .bottom(4)
            .background_color(ChartColor::Surface.css())
            .border_color(ChartColor::Line.css())
            .filler_color(ChartColor::Selected.css())
    }

    fn axis(visible: bool) -> Axis {
        Axis::new()
            .type_(AxisType::Value)
            .show(true)
            .axis_line(AxisLine::new().show(false))
            .axis_tick(AxisTick::new().show(false))
            .axis_label(
                AxisLabel::new()
                    .show(visible)
                    .font_family(MONO_FONT)
                    .font_size(10)
                    .color(ChartColor::InkFaint.css()),
            )
            .split_line(
                SplitLine::new()
                    .show(visible)
                    .line_style(LineStyle::new().color(ChartColor::Line.css()).width(1)),
            )
    }

    fn point(x: f64, y: Option<f64>) -> DataPoint {
        DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
    }
}

/// The checkpoints one run saved, read off every run's checkpoints.
pub(super) trait RunCheckpoints {
    fn of_run(&self, run_id: &str) -> Vec<TrainingCheckpoint>;
}

impl RunCheckpoints for [TrainingCheckpoint] {
    fn of_run(&self, run_id: &str) -> Vec<TrainingCheckpoint> {
        self.iter()
            .filter(|checkpoint| checkpoint.run_id() == run_id)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::training::{
        CheckpointModel, TrainingConfig, TrainingInputSummary, TrainingRunHistory,
        TrainingRunIdentity, TrainingRunStatus,
    };

    fn run_with(status: TrainingRunStatus, metrics: Vec<TrainingMetricPoint>) -> TrainingRun {
        TrainingRun::new(
            TrainingRunIdentity::new(
                "run",
                TrainingConfig::default(),
                TrainingInputSummary::default(),
                1,
            ),
            status,
        )
        .with_history(TrainingRunHistory::new(metrics, Vec::new()))
    }

    /// Before its first step a run says how far it has got in its own words,
    /// and its bar stays empty; afterwards the step does.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn progress_reads_the_message_until_the_first_step() {
        let uploading = run_with(
            TrainingRunStatus::new(
                TrainingRunPhase::Uploading,
                0,
                Some("Uploading 3 of 12 streams".to_owned()),
            ),
            Vec::new(),
        );
        assert!(uploading.phase().before_first_step());
        assert_eq!(uploading.progress_readout(), "Uploading 3 of 12 streams");
        assert_eq!(uploading.progress_percent(), 0.0);
        let preparing = run_with(
            TrainingRunStatus::new(TrainingRunPhase::Preparing, 0, None),
            Vec::new(),
        );
        assert_eq!(preparing.progress_readout(), "Preparing…");
        let running = run_with(
            TrainingRunStatus::new(TrainingRunPhase::Running, 25_000, None),
            Vec::new(),
        );
        assert!(!running.phase().before_first_step());
        assert_eq!(running.progress_readout(), "Step 25000 of 100000");
        assert_eq!(running.progress_percent(), 25.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn throughput_uses_the_optimizer_step_axis_and_slider_zoom() {
        let run = run_with(
            TrainingRunStatus::new(TrainingRunPhase::Running, 100, None),
            vec![TrainingMetricPoint::new(50, 0.0).with_examples_per_second(25.0)],
        );
        let option = serde_json::to_value(ResourceSeries::Throughput.chart(&run))
            .expect("chart should serialize");
        assert_eq!(option["xAxis"]["max"], 100.0);
        assert_eq!(option["series"][0]["data"][0][0], 50.0);
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(1));
        assert_eq!(option["dataZoom"][0]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["show"], true);
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
    }

    /// Checkpoints and metric events land on one axis, as a fraction of the
    /// steps the run was configured for.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn event_ticks_place_checkpoints_and_stalls_on_the_configured_step_axis() {
        let run = run_with(
            TrainingRunStatus::default(),
            vec![TrainingMetricPoint::new(25, 0.5).with_event("loader stall")],
        );
        let ticks = run.event_ticks(&[TrainingCheckpoint::new(
            "run",
            50,
            CheckpointModel::Progress,
        )]);
        assert_eq!(ticks[0].class, "training-event-checkpoint");
        assert_eq!(ticks[0].label, "Checkpoint saved at step 50");
        assert_eq!(ticks[1].class, "training-event-stall");
        assert_eq!(ticks[1].label, "loader stall at step 25");
        assert!(ticks[0].percent > ticks[1].percent);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn loss_chart_uses_a_linear_axis_and_slider_zoom_with_stable_series() {
        let run = run_with(
            TrainingRunStatus::default(),
            vec![TrainingMetricPoint::new(1, 0.25).with_validation_loss(Some(0.3))],
        );
        let option = serde_json::to_value(run.loss_chart()).expect("chart should serialize");

        assert_eq!(option["yAxis"]["type"], "value");
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(1));
        assert_eq!(option["dataZoom"][0]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
        assert_eq!(option["series"].as_array().map(Vec::len), Some(2));
        assert_eq!(option["series"][0]["id"], "train-loss");
        assert_eq!(option["series"][1]["id"], "validation-loss");
    }
}
