//! The authored linear pipeline and its measured, sequential episode flow.
use crate::components::icons::{FitViewIcon, ZoomInIcon, ZoomOutIcon};
use dioxus::prelude::*;
use dioxus_flow::{
    Edge, EdgeKind, Flow, Handle, HandleKind, MarkerKind, Node, NodeViewCtx, Point, Rect, Side,
    Viewport, use_flow,
};
use spiral_robotics_common::curation::run::{CurationOutcome, CurationVerdict};
use spiral_robotics_common::curation::{CurationSpec, CurationStep};

use super::state::CurationController;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PipelineSelection {
    Source,
    Step(String),
    Output,
}

impl PipelineSelection {
    pub(super) fn admits(&self, verdict: &CurationVerdict, spec: &CurationSpec) -> bool {
        let Self::Step(id) = self else {
            return *self != Self::Output || verdict.kept();
        };
        if !spec.reaches_step(verdict, id) {
            return false;
        }
        spec.steps()
            .iter()
            .find(|step| step.id() == id)
            .is_some_and(|step| match step {
                CurationStep::Filter(_) => verdict.excluded_by().iter().any(|reason| reason == id),
                CurationStep::Transform(_) => verdict
                    .eligible_transforms()
                    .iter()
                    .any(|transform| transform == id),
            })
    }

    pub(super) fn evidence_label(&self, spec: &CurationSpec) -> String {
        match self {
            Self::Source => "Source episodes".into(),
            Self::Output => "Curated episodes".into(),
            Self::Step(id) => spec
                .steps()
                .iter()
                .find(|step| step.id() == id)
                .map_or_else(
                    || "Stage no longer in this recipe".into(),
                    |step| match step {
                        CurationStep::Filter(_) => "Episodes excluded at this filter".into(),
                        CurationStep::Transform(_) => "Episodes reached by this transform".into(),
                    },
                ),
        }
    }
}

#[derive(Clone, PartialEq)]
struct StageFlow {
    incoming: u64,
    retained: u64,
    excluded: u64,
    duration: Option<(u64, u64)>,
    affected: Option<u64>,
    scored: Option<u64>,
}

impl StageFlow {
    fn unchanged(episodes: u64) -> Self {
        Self {
            incoming: episodes,
            retained: episodes,
            excluded: 0,
            duration: None,
            affected: None,
            scored: None,
        }
    }
    fn from_outcome(outcome: &CurationOutcome, id: &str) -> Option<Self> {
        outcome
            .stage_impacts()
            .iter()
            .find(|impact| impact.id() == id)
            .map(|impact| Self {
                incoming: impact.incoming(),
                retained: impact.retained(),
                excluded: impact.excluded(),
                duration: impact.retained_ns().zip(impact.excluded_ns()),
                affected: impact.affected_episodes(),
                scored: impact.scored_episodes(),
            })
    }
}

#[derive(Clone, PartialEq)]
struct PipelineStage {
    selection: PipelineSelection,
    title: String,
    detail: String,
    kind: &'static str,
    flow: Option<StageFlow>,
}

impl PipelineStage {
    fn source(spec: &CurationSpec, outcome: Option<&CurationOutcome>) -> Self {
        Self {
            selection: PipelineSelection::Source,
            title: "Dataset".into(),
            detail: spec
                .source()
                .map_or_else(|| "Choose a dataset".into(), |source| source.label()),
            kind: "source",
            flow: outcome.map(|outcome| StageFlow::unchanged(outcome.totals().episodes())),
        }
    }
    fn step(
        step: CurationStep<'_>,
        context: (&CurationSpec, Option<&CurationOutcome>),
        index: usize,
    ) -> Self {
        let (spec, outcome) = context;
        let (kind, detail) = match &step {
            CurationStep::Filter(rule) => ("filter", rule.condition().sentence(&spec.catalog())),
            CurationStep::Transform(transform) => (
                "transform",
                if matches!(transform.operation(), spiral_robotics_common::curation::rules::TransformOperation::SelectProgressChunks { .. }) {
                    "Remove unproductive moments".to_owned()
                } else { transform.title() },
            ),
        };
        Self {
            selection: PipelineSelection::Step(step.id().to_owned()),
            title: format!(
                "{} · {}",
                index + 1,
                if kind == "filter" {
                    "Filter"
                } else {
                    "Transform"
                }
            ),
            detail,
            kind,
            flow: outcome.and_then(|outcome| StageFlow::from_outcome(outcome, step.id())),
        }
    }
    fn output(outcome: Option<&CurationOutcome>) -> Self {
        Self {
            selection: PipelineSelection::Output,
            title: "Curated output".into(),
            detail: "Episodes retained by this recipe".into(),
            kind: "output",
            flow: outcome.map(|outcome| StageFlow::unchanged(outcome.totals().kept())),
        }
    }
}

#[component]
pub(super) fn Pipeline(
    controller: CurationController,
    preview: Option<CurationOutcome>,
) -> Element {
    let spec = controller.spec();
    let snapshot = controller.snapshot();
    let outcome = controller.outcome();
    let current = outcome.is_some()
        && !snapshot.result_stale()
        && controller
            .judged_spec()
            .is_some_and(|judged| spec.accepts_output_from(&judged));
    let measured = outcome.as_ref().filter(|_| current).or(preview.as_ref());
    let total = measured.map_or(0, |outcome| outcome.totals().episodes());
    let selected = controller.pipeline_selection();
    let mut stages = vec![PipelineStage::source(&spec, measured)];
    stages.extend(
        spec.steps()
            .into_iter()
            .enumerate()
            .map(|(index, step)| PipelineStage::step(step, (&spec, measured), index)),
    );
    stages.push(PipelineStage::output(measured));
    let pending = if snapshot.phase().active() {
        "Running…"
    } else if outcome.is_some() && !current {
        "Run again"
    } else {
        "Not measured"
    };
    rsx! {
        section { class: "curation-pipeline", "aria-label": "Ordered curation pipeline",
            div { class: "curation-pipeline-heading",
                strong { "Pipeline" }
                span { if measured.is_some() { "Episode flow · select a stage to inspect" } else { "Run the recipe to measure its impact" } }
            }
            PipelineCanvas { stages, selected, total, pending, controller }

        }
    }
}

impl PipelineStage {
    fn node_id(&self) -> String {
        match &self.selection {
            PipelineSelection::Source => "source".into(),
            PipelineSelection::Step(id) => format!("step:{id}"),
            PipelineSelection::Output => "output".into(),
        }
    }
}

#[derive(Clone)]
struct PipelineGraph {
    nodes: Vec<Node<PipelineStage>>,
    edges: Vec<Edge>,
}

impl PipelineGraph {
    fn new(stages: &[PipelineStage], total: u64, selection: &PipelineSelection) -> Self {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for (index, stage) in stages.iter().enumerate() {
            let id = stage.node_id();
            let mut node = Node::with_data(
                &id,
                &stage.title,
                Point::new(index as f64 * 184.0 + 12.0, 12.0),
                stage.clone(),
            )
            .size((160.0, 104.0))
            .sides(Side::Left, Side::Right)
            .draggable(false);
            node.selected = &stage.selection == selection;
            nodes.push(node);
            if let Some(next) = stages.get(index + 1) {
                edges.push(
                    Self::connection(
                        (&id, &next.node_id()),
                        stage.flow.as_ref().map(|flow| flow.retained),
                        total,
                    )
                    .source_handle("retained")
                    .target_handle("incoming"),
                );
            }
            if stage.kind == "filter" {
                if let Some(flow) = stage.flow.as_ref().filter(|flow| flow.excluded > 0) {
                    let branch_id = format!("excluded:{id}");
                    let excluded = PipelineStage {
                        selection: stage.selection.clone(),
                        title: format!("{} excluded", flow.excluded),
                        detail: format!("of {} incoming", flow.incoming),
                        kind: "excluded",
                        flow: None,
                    };
                    nodes.push(
                        Node::with_data(
                            &branch_id,
                            &excluded.title,
                            Point::new(index as f64 * 184.0 + 32.0, 148.0),
                            excluded.clone(),
                        )
                        .size((120.0, 28.0))
                        .sides(Side::Top, Side::Bottom)
                        .draggable(false),
                    );
                    edges.push(
                        Self::connection((&id, &branch_id), Some(flow.excluded), total)
                            .source_handle("excluded")
                            .target_handle("incoming")
                            .class("curation-flow-excluded")
                            .style(format!(
                                "stroke:var(--tone-line-strong);stroke-dasharray:none;stroke-width:{}",
                                14.0 * flow.excluded as f64 / total.max(1) as f64
                            )),
                    );
                }
            }
        }
        Self { nodes, edges }
    }
    fn connection(endpoints: (&str, &str), count: Option<u64>, total: u64) -> Edge {
        let style = count.map_or_else(
            || "stroke:var(--tone-line-strong);stroke-width:1;stroke-dasharray:3 3".into(),
            |count| {
                format!(
                    "stroke:var(--tone-accent);stroke-width:{};stroke-dasharray:none;opacity:0.7",
                    14.0 * count as f64 / total.max(1) as f64
                )
            },
        );
        Edge::new(endpoints.0, endpoints.1)
            .kind(EdgeKind::SmoothStep)
            .marker_end(MarkerKind::None)
            .style(style)
    }
}

#[component]
fn PipelineCanvas(
    stages: Vec<PipelineStage>,
    selected: PipelineSelection,
    total: u64,
    pending: &'static str,
    controller: CurationController,
) -> Element {
    let initial = use_hook(|| PipelineGraph::new(&stages, total, &selected));
    let mut nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    use_effect(use_reactive(
        (&stages, &selected, &total),
        move |(stages, selected, total)| {
            let graph = PipelineGraph::new(&stages, total, &selected);
            nodes.set(graph.nodes);
            edges.set(graph.edges);
        },
    ));
    rsx! {
        div { class: "curation-pipeline-canvas",
            Flow { id: "curation-pipeline-flow", class: "curation-flow",
                nodes, edges, fit_view: false, fit_view_padding: 0.08,
                min_zoom: 0.3, max_zoom: 1.5, nodes_draggable: false, delete_key: false,
                is_valid_connection: move |_| false,
                on_node_click: move |id| {
                    if let Some(node) = nodes.read().iter().find(|node| node.id == id) {
                        controller.select_pipeline(node.data.selection.clone());
                    }
                },
                node_view: move |ctx: NodeViewCtx<PipelineStage>| rsx! { PipelineNode { ctx, pending, controller } },
                PipelineInitialViewport {}
                PipelineControls {}
            }
        }
    }
}

/// Match the workflow canvas: fit while readable, otherwise start at the
/// dataset and let native pan/zoom expose the rest of a long recipe.
#[component]
fn PipelineInitialViewport() -> Element {
    let flow = use_flow();
    let mut automatic_viewport = use_signal(|| None::<Viewport>);
    use_effect(move || {
        let container = *flow.container.read();
        let geometry = flow.geoms.read();
        if container.width <= 0.0 || container.height <= 0.0 {
            return;
        }
        if automatic_viewport
            .peek()
            .is_some_and(|last| last != *flow.viewport.peek())
        {
            return;
        }
        let Some(bounds) = geometry
            .iter()
            .map(|node| node.rect)
            .reduce(|left, right| left.union(&right))
        else {
            return;
        };
        let safe = Rect::new(
            16.0,
            16.0,
            (container.width - 32.0).max(1.0),
            (container.height - 56.0).max(1.0),
        );
        let fit_zoom = (safe.width / bounds.width).min(safe.height / bounds.height);
        let viewport = if fit_zoom >= 0.75 {
            Viewport::fit(Some(bounds), safe, 0.75, 1.0)
        } else {
            let zoom = (safe.height / bounds.height).clamp(0.75, 1.0);
            Viewport::new(
                safe.x - bounds.x * zoom,
                safe.y + (safe.height - bounds.height * zoom) / 2.0 - bounds.y * zoom,
                zoom,
            )
        };
        automatic_viewport.set(Some(viewport));
        flow.set_viewport(viewport, 0);
    });
    rsx! {}
}

#[component]
fn PipelineControls() -> Element {
    let flow = use_flow();
    rsx! {
        div { class: "workflow-canvas-controls", role: "toolbar", "aria-label": "Pipeline view",
            onpointerdown: move |event| event.stop_propagation(),
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, aria_label: "Zoom in", onclick: move |_| flow.zoom_in(200), ZoomInIcon {} }
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, aria_label: "Zoom out", onclick: move |_| flow.zoom_out(200), ZoomOutIcon {} }
            Action { purpose: ActionPurpose::Navigate, variant: ActionVariant::Icon, aria_label: "Fit pipeline", onclick: move |_| flow.fit_view(200), FitViewIcon {} }
        }
    }
}

#[component]
fn PipelineNode(
    ctx: NodeViewCtx<PipelineStage>,
    pending: &'static str,
    controller: CurationController,
) -> Element {
    let stage = &ctx.node.data;
    let selection = stage.selection.clone();
    rsx! {
        div { class: "curation-flow-stage", "data-kind": stage.kind,
            Action { purpose: ActionPurpose::Select, variant: ActionVariant::Quiet,
                class: "curation-pipeline-node", aria_pressed: controller.pipeline_selection() == stage.selection && stage.kind != "excluded", title: stage.detail.clone(),
                onclick: move |_| controller.select_pipeline(selection.clone()),
                div { class: "curation-pipeline-node-copy",
                    strong { "{stage.title}" }
                    if stage.kind != "excluded" { span { class: "curation-pipeline-detail", "{stage.detail}" } }
                    if let Some(flow) = &stage.flow {
                        if stage.kind == "filter" { small { "{flow.incoming} in · {flow.excluded} excluded" } }
                        else if let Some(scored) = flow.scored {
                            small { "{scored} scored · {flow.incoming.saturating_sub(scored)} unscored" }
                            if let Some((kept, removed)) = flow.duration { small { "Scored: {kept as f64 / 1e9:.1} s kept · {removed as f64 / 1e9:.1} s removed" } }
                        }
                        else if let Some((kept, removed)) = flow.duration { small { "{kept as f64 / 1e9:.1} s kept · {removed as f64 / 1e9:.1} s removed" } }
                        else if stage.kind == "transform" {
                            if let Some(affected) = flow.affected { small { "{affected} eligible" } }
                            else { small { "Episode count unchanged" } }
                        }
                    }
                }
                if stage.kind != "excluded" {
                    span { class: "curation-pipeline-count",
                        if let Some(flow) = &stage.flow { strong { "{flow.retained}" } span { "episodes" } }
                        else { span { "{pending}" } }
                    }
                }
            }
            if stage.kind != "source" {
                Handle { id: "incoming", kind: HandleKind::Target, position: if stage.kind == "excluded" { Side::Top } else { Side::Left } }
            }
            if stage.kind != "output" && stage.kind != "excluded" {
                Handle { id: "retained", kind: HandleKind::Source, position: Side::Right }
            }
            if stage.kind == "filter" && stage.flow.as_ref().is_some_and(|flow| flow.excluded > 0) {
                Handle { id: "excluded", kind: HandleKind::Source, position: Side::Bottom }
            }
        }
    }
}

#[derive(Clone, Default)]
struct DistributionBin {
    retained: usize,
    excluded: usize,
}

struct FilterDistribution {
    bins: Vec<DistributionBin>,
    range: (f64, f64),
    threshold: Option<f64>,
    missing: usize,
    label: String,
}

impl FilterDistribution {
    fn new(spec: &CurationSpec, outcome: &CurationOutcome, id: &str) -> Option<Self> {
        use spiral_robotics_common::curation::{facts::FactKind, rules::ConditionValue};
        let rule = spec.rules().iter().find(|rule| rule.id() == id)?;
        let condition = rule.condition();
        let catalog = spec.catalog();
        if catalog
            .entries()
            .iter()
            .find(|entry| entry.key() == condition.fact())?
            .kind()
            != FactKind::Number
        {
            return None;
        }
        let incoming = outcome
            .verdicts()
            .iter()
            .filter(|verdict| spec.reaches_step(verdict, id))
            .collect::<Vec<_>>();
        let values = incoming
            .iter()
            .filter_map(|verdict| {
                Self::value(verdict, condition.fact())
                    .filter(|value| value.is_finite())
                    .map(|value| {
                        (
                            value,
                            verdict.excluded_by().iter().any(|reason| reason == id),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let threshold = match condition.value() {
            ConditionValue::Number(value) if value.is_finite() => Some(*value),
            _ => None,
        };
        let mut minimum = values.iter().map(|(value, _)| *value).reduce(f64::min)?;
        let mut maximum = values.iter().map(|(value, _)| *value).reduce(f64::max)?;
        if let Some(threshold) = threshold {
            minimum = minimum.min(threshold);
            maximum = maximum.max(threshold);
        }
        if minimum == maximum {
            minimum -= 0.5;
            maximum += 0.5;
        }
        let mut bins = vec![DistributionBin::default(); 20];
        for (value, excluded) in &values {
            let index = (((value - minimum) / (maximum - minimum) * bins.len() as f64) as usize)
                .min(bins.len() - 1);
            if *excluded {
                bins[index].excluded += 1;
            } else {
                bins[index].retained += 1;
            }
        }
        Some(Self {
            bins,
            range: (minimum, maximum),
            threshold,
            missing: incoming.len() - values.len(),
            label: condition.sentence(&catalog),
        })
    }
    fn value(verdict: &CurationVerdict, key: &str) -> Option<f64> {
        match key {
            "duration_s" => verdict.episode().duration_seconds(),
            "messages" => verdict.episode().message_count().map(|value| value as f64),
            "channels" => verdict.episode().channel_count().map(|value| value as f64),
            "size_mb" => verdict
                .episode()
                .source_bytes()
                .map(|value| value as f64 / 1_000_000.0),
            _ => verdict.fact(key)?.as_f64(),
        }
    }
    fn scale(&self) -> f64 {
        66.0 / self
            .bins
            .iter()
            .map(|bin| bin.retained + bin.excluded)
            .max()
            .unwrap_or(1)
            .max(1) as f64
    }
    fn threshold_x(&self) -> Option<f64> {
        self.threshold.map(|threshold| {
            12.0 + 576.0 * (threshold - self.range.0) / (self.range.1 - self.range.0)
        })
    }
}

#[component]
pub(super) fn StageDistribution(
    controller: CurationController,
    preview: Option<CurationOutcome>,
) -> Element {
    let PipelineSelection::Step(id) = controller.pipeline_selection() else {
        return rsx! {};
    };
    let spec = controller.spec();
    let run_matches = controller
        .judged_spec()
        .is_some_and(|judged| spec.accepts_output_from(&judged));
    let Some(outcome) = controller.outcome().filter(|_| run_matches).or(preview) else {
        return rsx! {};
    };
    if !spec.rules().iter().any(|rule| rule.id() == id) {
        return rsx! {};
    }
    if outcome.totals().truncated()
        || outcome.verdicts().len() as u64 != outcome.totals().episodes()
    {
        return rsx! { p { class: "curation-section-note", "The episode preview is incomplete; a full distribution is unavailable." } };
    }
    let Some(distribution) = FilterDistribution::new(&spec, &outcome, &id) else {
        return rsx! {};
    };
    let impact = outcome
        .stage_impacts()
        .iter()
        .find(|impact| impact.id() == id);
    let scale = distribution.scale();
    rsx! {
        figure { class: "curation-distribution", "aria-label": "Incoming episode measurement distribution",
            figcaption {
                strong { "Exclude when {distribution.label}" }
                if let Some(impact) = impact { span { "{impact.incoming()} incoming · {impact.excluded()} excluded" } }
            }
            svg { view_box: "0 0 600 96", role: "img", "aria-label": "Histogram of all incoming episodes, with excluded values in gray and the threshold marked",
                for (index, bin) in distribution.bins.iter().enumerate() {
                    rect { class: "curation-distribution-kept", x: "{12.0 + index as f64 * 28.8}", y: "{76.0 - bin.retained as f64 * scale}", width: "27", height: "{bin.retained as f64 * scale}" }
                    rect { class: "curation-distribution-excluded", x: "{12.0 + index as f64 * 28.8}", y: "{76.0 - (bin.retained + bin.excluded) as f64 * scale}", width: "27", height: "{bin.excluded as f64 * scale}" }
                }
                line { class: "curation-distribution-axis", x1: "12", y1: "77", x2: "588", y2: "77" }
                if let Some(x) = distribution.threshold_x() { line { class: "curation-distribution-threshold", x1: "{x}", x2: "{x}", y1: "4", y2: "79" } }
                text { x: "12", y: "92", "{distribution.range.0:.2}" }
                text { x: "588", y: "92", text_anchor: "end", "{distribution.range.1:.2}" }
            }
            div { class: "curation-distribution-legend",
                span { i { class: "curation-distribution-kept" } "Retained" }
                span { i { class: "curation-distribution-excluded" } "Excluded" }
                if let Some(threshold) = distribution.threshold { span { "Threshold {threshold}" } }
                if distribution.missing > 0 { span { "{distribution.missing} missing values" } }
            }
        }
    }
}
