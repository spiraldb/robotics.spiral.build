//! The authored linear pipeline and its measured, sequential episode flow.
//!
//! Stages wear the Studio's stage-card anatomy (kind mark, title, one summary
//! line, a footer that names what executes), so a curation recipe and a
//! workflow graph read as one vocabulary. A filter's identity tint is the
//! same wash its reason chips wear on the tiles it struck.
use crate::components::canvas_controls::CanvasControls;
use crate::components::icons::{
    DataIcon, MediaStageIcon, ModelStageIcon, QueryNodeIcon, ResultNodeIcon,
};
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Edge, EdgeKind, Flow, Handle, HandleKind, MarkerKind, MiniMap,
    Node, NodeViewCtx, Point, Rect, Side, Viewport, use_flow,
};
use spiral_robotics_common::curation::rules::TransformOperation;
use spiral_robotics_common::curation::run::{CurationOutcome, CurationVerdict};
use spiral_robotics_common::curation::{CurationSpec, CurationStep};

use super::sheet::RuleTone;
use super::state::CurationController;

/// Narrower and taller than a Workflow stage, so a recipe of several steps
/// fits the pane; the summary gets a third line instead.
const CARD_WIDTH: f64 = 180.0;
const CARD_HEIGHT: f64 = 108.0;
const CARD_STEP: f64 = 204.0;
const BRANCH_WIDTH: f64 = 112.0;
const BRANCH_HEIGHT: f64 = 24.0;
const CANVAS_TOP: f64 = 16.0;
const CANVAS_LEFT: f64 = 24.0;
/// Below this zoom a card's 12px title drops under 9px; the strip pans instead.
const MIN_READABLE_ZOOM: f64 = 0.7;

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
    pub(super) fn step_id(&self) -> Option<&str> {
        match self {
            Self::Step(id) => Some(id),
            _ => None,
        }
    }
}

/// What a stage does to the episodes; drives its glyph and identity tint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StageKind {
    Source,
    Filter,
    Model,
    Media,
    Trim,
    Output,
}

impl StageKind {
    /// The Workflow stage tint class; a filter carries its rule's tone
    /// inline instead, so its class only names the shape.
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Source => "workflow-stage-source",
            Self::Filter => "curation-stage-filter",
            Self::Model => "workflow-stage-model",
            Self::Media => "workflow-stage-media",
            Self::Trim => "workflow-stage-shape",
            Self::Output => "workflow-stage-output",
        }
    }
    const fn attribute(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Filter => "filter",
            Self::Model | Self::Media | Self::Trim => "transform",
            Self::Output => "output",
        }
    }
    pub(super) fn of_transform(operation: &TransformOperation) -> Self {
        match operation {
            TransformOperation::SelectProgressChunks { .. } => Self::Model,
            TransformOperation::ResizeCamera { .. }
            | TransformOperation::CameraColorProfile { .. } => Self::Media,
            TransformOperation::TrimTime { .. } => Self::Trim,
        }
    }
}

#[component]
pub(super) fn StageGlyph(kind: StageKind) -> Element {
    match kind {
        StageKind::Source => rsx! { DataIcon {} },
        StageKind::Filter | StageKind::Trim => rsx! { QueryNodeIcon {} },
        StageKind::Model => rsx! { ModelStageIcon {} },
        StageKind::Media => rsx! { MediaStageIcon {} },
        StageKind::Output => rsx! { ResultNodeIcon {} },
    }
}

/// The kind mark: the glyph on its tint, shared by cards and stage headings.
#[component]
pub(super) fn StageMark(kind: StageKind, tone: Option<RuleTone>) -> Element {
    rsx! {
        span { class: "workflow-stage-mark {kind.class_name()}", "aria-hidden": "true",
            style: tone.map(|tone| format!("--stage-tone: {}", tone.css())),
            StageGlyph { kind }
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
    /// The footer's measured note: what this stage did to its incoming episodes.
    fn note(&self, kind: StageKind) -> Option<String> {
        match kind {
            StageKind::Source | StageKind::Output => None,
            StageKind::Filter => Some(format!("{} in", self.incoming)),
            StageKind::Model | StageKind::Media | StageKind::Trim => {
                if let Some((kept, removed)) = self.duration {
                    Some(format!(
                        "{:.1} s kept · {:.1} s removed",
                        kept as f64 / 1e9,
                        removed as f64 / 1e9
                    ))
                } else if let Some(scored) = self.scored {
                    Some(format!("{scored} scored"))
                } else {
                    self.affected.map(|affected| format!("{affected} eligible"))
                }
            }
        }
    }
}

/// One stage as the strip and the stage heading describe it.
#[derive(Clone, PartialEq)]
pub(super) struct PipelineStage {
    pub(super) selection: PipelineSelection,
    pub(super) kind: StageKind,
    pub(super) tone: Option<RuleTone>,
    pub(super) title: String,
    pub(super) summary: String,
    order: usize,
    flow: Option<StageFlow>,
    /// Whether the step can run as written; an incomplete one is never measured.
    complete: bool,
}

impl PipelineStage {
    /// Every stage in recipe order, measured where the outcome knows it.
    pub(super) fn all(spec: &CurationSpec, outcome: Option<&CurationOutcome>) -> Vec<Self> {
        let mut stages = vec![Self::source(spec, outcome)];
        stages.extend(
            spec.steps()
                .into_iter()
                .enumerate()
                .map(|(index, step)| Self::step(step, (spec, outcome), index)),
        );
        stages.push(Self::output(outcome, stages.len()));
        stages
    }
    fn source(spec: &CurationSpec, outcome: Option<&CurationOutcome>) -> Self {
        Self {
            selection: PipelineSelection::Source,
            kind: StageKind::Source,
            tone: None,
            title: "Dataset".into(),
            summary: spec
                .source()
                .map_or_else(|| "Choose a dataset".into(), |source| source.label()),
            order: 0,
            flow: outcome.map(|outcome| StageFlow::unchanged(outcome.totals().episodes())),
            complete: true,
        }
    }
    fn step(
        step: CurationStep<'_>,
        context: (&CurationSpec, Option<&CurationOutcome>),
        index: usize,
    ) -> Self {
        let (spec, outcome) = context;
        let catalog = spec.catalog();
        let (kind, tone, summary, complete) = match &step {
            CurationStep::Filter(rule) => (
                StageKind::Filter,
                Some(RuleTone::of(spec, rule.id())),
                rule.condition().sentence(&catalog),
                rule.condition().validate(&catalog).is_ok(),
            ),
            CurationStep::Transform(transform) => (
                StageKind::of_transform(transform.operation()),
                None,
                match transform.operation() {
                    TransformOperation::SelectProgressChunks { .. } => {
                        transform.operation().kind_label().to_owned()
                    }
                    _ => transform.title(),
                },
                transform.validate(&catalog).is_ok(),
            ),
        };
        Self {
            selection: PipelineSelection::Step(step.id().to_owned()),
            kind,
            tone,
            title: format!(
                "{} · {}",
                index + 1,
                if kind == StageKind::Filter {
                    "Filter"
                } else {
                    "Transform"
                }
            ),
            summary,
            order: index + 1,
            flow: outcome.and_then(|outcome| StageFlow::from_outcome(outcome, step.id())),
            complete,
        }
    }
    fn output(outcome: Option<&CurationOutcome>, order: usize) -> Self {
        Self {
            selection: PipelineSelection::Output,
            kind: StageKind::Output,
            tone: None,
            title: "Curated output".into(),
            summary: "Episodes kept by this recipe".into(),
            order,
            flow: outcome.map(|outcome| StageFlow::unchanged(outcome.totals().kept())),
            complete: true,
        }
    }
    fn node_id(&self) -> String {
        match &self.selection {
            PipelineSelection::Source => "source".into(),
            PipelineSelection::Step(id) => format!("step:{id}"),
            PipelineSelection::Output => "output".into(),
        }
    }
}

/// What the count slot says before the outcome knows the stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PipelinePending {
    Measuring,
    Running,
    Stale,
    Unmeasured,
}

impl PipelinePending {
    const fn label(self) -> &'static str {
        match self {
            Self::Measuring => "Measuring…",
            Self::Running => "Running…",
            Self::Stale => "Run again",
            Self::Unmeasured => "Not measured",
        }
    }
}

#[component]
pub(super) fn Pipeline(
    controller: CurationController,
    stages: Vec<PipelineStage>,
    pending: PipelinePending,
) -> Element {
    let selected = controller.pipeline_selection();
    rsx! {
        section { class: "curation-pipeline", "aria-label": "Curation pipeline",
            PipelineCanvas { stages, selected, pending, controller }
        }
    }
}

/// A branch node: the episodes a filter sent out of the flow.
#[derive(Clone, PartialEq)]
enum PipelineNodeKind {
    Stage(PipelineStage),
    Branch {
        selection: PipelineSelection,
        tone: RuleTone,
        excluded: u64,
    },
}

#[derive(Clone)]
struct PipelineGraph {
    nodes: Vec<Node<PipelineNodeKind>>,
    edges: Vec<Edge>,
}

impl PipelineGraph {
    fn new(stages: &[PipelineStage], selection: &PipelineSelection) -> Self {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for (index, stage) in stages.iter().enumerate() {
            let id = stage.node_id();
            let x = CANVAS_LEFT + index as f64 * CARD_STEP;
            let mut node = Node::with_data(
                &id,
                &stage.title,
                Point::new(x, CANVAS_TOP),
                PipelineNodeKind::Stage(stage.clone()),
            )
            .size((CARD_WIDTH, CARD_HEIGHT))
            .sides(Side::Left, Side::Right)
            .draggable(false);
            node.selected = &stage.selection == selection;
            nodes.push(node);
            if let Some(next) = stages.get(index + 1) {
                let mut edge = Edge::new(&id, next.node_id())
                    .kind(EdgeKind::SmoothStep)
                    .marker_end(MarkerKind::None)
                    .source_handle("retained")
                    .target_handle("incoming");
                if stage.flow.is_none() {
                    edge = edge.class("curation-flow-pending");
                }
                edges.push(edge);
            }
            if let (StageKind::Filter, Some(tone), Some(flow)) = (
                stage.kind,
                stage.tone,
                stage.flow.as_ref().filter(|flow| flow.excluded > 0),
            ) {
                let branch_id = format!("excluded:{id}");
                nodes.push(
                    Node::with_data(
                        &branch_id,
                        format!("{} excluded", flow.excluded),
                        Point::new(
                            x + (CARD_WIDTH - BRANCH_WIDTH) / 2.0,
                            CANVAS_TOP + CARD_HEIGHT + 24.0,
                        ),
                        PipelineNodeKind::Branch {
                            selection: stage.selection.clone(),
                            tone,
                            excluded: flow.excluded,
                        },
                    )
                    .size((BRANCH_WIDTH, BRANCH_HEIGHT))
                    .sides(Side::Top, Side::Bottom)
                    .draggable(false),
                );
                edges.push(
                    Edge::new(&id, &branch_id)
                        .kind(EdgeKind::Straight)
                        .marker_end(MarkerKind::None)
                        .source_handle("excluded")
                        .target_handle("incoming")
                        .style(format!("stroke: {}", tone.css())),
                );
            }
        }
        Self { nodes, edges }
    }
}

#[component]
fn PipelineCanvas(
    stages: Vec<PipelineStage>,
    selected: PipelineSelection,
    pending: PipelinePending,
    controller: CurationController,
) -> Element {
    let initial = use_hook(|| PipelineGraph::new(&stages, &selected));
    let mut nodes = use_signal(|| initial.nodes);
    let mut edges = use_signal(|| initial.edges);
    use_effect(use_reactive(
        (&stages, &selected),
        move |(stages, selected)| {
            let graph = PipelineGraph::new(&stages, &selected);
            nodes.set(graph.nodes);
            edges.set(graph.edges);
        },
    ));
    let selected_id = stages
        .iter()
        .find(|stage| stage.selection == selected)
        .map(PipelineStage::node_id)
        .unwrap_or_default();
    rsx! {
        div { class: "curation-pipeline-canvas",
            Flow { id: "curation-pipeline-flow", class: "curation-flow",
                nodes, edges, fit_view: false, fit_view_padding: 0.08,
                min_zoom: 0.4, max_zoom: 1.25, nodes_draggable: false, delete_key: false,
                // Two fingers pan the strip as they do the Workflow graph; a
                // pinch (or ctrl/⌘ + wheel) and the controls zoom.
                pan_on_scroll: true, zoom_on_scroll: false,
                is_valid_connection: move |_| false,
                on_node_click: move |id| {
                    let selection = nodes.read().iter().find(|node| node.id == id).map(|node| match &node.data {
                        PipelineNodeKind::Stage(stage) => stage.selection.clone(),
                        PipelineNodeKind::Branch { selection, .. } => selection.clone(),
                    });
                    if let Some(selection) = selection {
                        controller.select_pipeline(selection);
                    }
                },
                node_view: move |ctx: NodeViewCtx<PipelineNodeKind>| rsx! { PipelineNode { ctx, pending } },
                Background { variant: BackgroundVariant::Dots, gap: 22.0, size: 0.7 }
                PipelineViewport { selected_id }
                CanvasControls { label: "Pipeline view", fit_label: "Fit the whole pipeline" }
            }
        }
    }
}

/// Match the workflow canvas: fit while readable, otherwise start at the
/// dataset and let a drag expose the rest of a long recipe. Selecting a
/// stage that sits outside the pane slides it into view at the same zoom,
/// so a rule chosen in the recipe always shows its card, and a pane that
/// narrows (the side sheet opening) keeps the selected card in view.
#[component]
fn PipelineViewport(selected_id: ReadSignal<String>) -> Element {
    let flow = use_flow();
    let mut automatic_viewport = use_signal(|| None::<Viewport>);
    let mut revealed = use_signal(String::new);
    let mut overflowing = use_signal(|| false);
    use_effect(move || {
        let container = *flow.container.read();
        let geometry = flow.geoms.read();
        let selected = selected_id();
        if container.width <= 0.0 || container.height <= 0.0 {
            return;
        }
        let Some(bounds) = geometry
            .iter()
            .map(|node| node.rect)
            .reduce(|left, right| left.union(&right))
        else {
            return;
        };
        let current = *flow.viewport.peek();
        let settled = automatic_viewport.peek().is_some();
        let user_controlled = automatic_viewport
            .peek()
            .is_some_and(|last| last != current);
        let selection_changed = *revealed.peek() != selected;
        if selection_changed {
            revealed.set(selected.clone());
        }
        if user_controlled && !selection_changed {
            return;
        }
        let safe = Rect::new(
            16.0,
            12.0,
            (container.width - 32.0).max(1.0),
            (container.height - 24.0).max(1.0),
        );
        let fit_zoom = (safe.width / bounds.width).min(safe.height / bounds.height);
        let fits = fit_zoom >= MIN_READABLE_ZOOM;
        if *overflowing.peek() != !fits {
            overflowing.set(!fits);
        }
        let base = if user_controlled {
            current
        } else if fits {
            Viewport::fit(Some(bounds), safe, MIN_READABLE_ZOOM, 1.0)
        } else {
            let zoom = (safe.height / bounds.height).clamp(MIN_READABLE_ZOOM, 1.0);
            Viewport::new(
                safe.x - bounds.x * zoom,
                safe.y + (safe.height - bounds.height * zoom) / 2.0 - bounds.y * zoom,
                zoom,
            )
        };
        let target = geometry
            .iter()
            .find(|node| node.id == selected)
            .map_or(base, |node| {
                reveal(base, node.rect, bounds, container.width)
            });
        automatic_viewport.set(Some(target));
        if target != current {
            let duration = if settled && selection_changed { 200 } else { 0 };
            flow.set_viewport(target, duration);
        }
    });
    rsx! {
        if overflowing() { MiniMap { width: 112.0, height: 56.0 } }
    }
}

/// The viewport that keeps `rect` in the pane: `base` when it already is,
/// otherwise the card centred, but never with the strip's first or last card
/// slid away from the pane's edge.
fn reveal(base: Viewport, rect: Rect, bounds: Rect, width: f64) -> Viewport {
    const MARGIN: f64 = 24.0;
    let left = base.x + rect.x * base.zoom;
    let right = base.x + rect.max_x() * base.zoom;
    if left >= MARGIN && right <= width - MARGIN {
        return base;
    }
    let centred = width / 2.0 - rect.center().x * base.zoom;
    let first = MARGIN - bounds.x * base.zoom;
    let last = width - MARGIN - bounds.max_x() * base.zoom;
    let x = if last > first {
        first
    } else {
        centred.clamp(last, first)
    };
    Viewport::new(x, base.y, base.zoom)
}

#[component]
fn PipelineNode(ctx: NodeViewCtx<PipelineNodeKind>, pending: PipelinePending) -> Element {
    match &ctx.node.data {
        PipelineNodeKind::Branch { tone, excluded, .. } => rsx! {
            div { class: "curation-flow-branch", style: "--stage-tone: {tone.css()}",
                title: "Episodes this filter excluded",
                span { class: "curation-flow-branch-count", "{excluded}" }
                " excluded"
                Handle { id: "incoming", kind: HandleKind::Target, position: Side::Top }
            }
        },
        PipelineNodeKind::Stage(stage) => {
            // The mark and title already name the kind; the footer holds what
            // the stage measured, or why it has not been measured yet, and
            // the episodes it sends on.
            let footer_note = stage.flow.as_ref().and_then(|flow| flow.note(stage.kind));
            rsx! {
                div { class: "workflow-stage-card curation-stage-card {stage.kind.class_name()}",
                    class: if ctx.node.selected { "is-selected" },
                    "data-kind": stage.kind.attribute(),
                    style: format!(
                        "--stage-order: {}{}",
                        stage.order,
                        stage.tone.map(|tone| format!("; --stage-tone: {}", tone.css())).unwrap_or_default()
                    ),
                    div { class: "workflow-stage-heading",
                        span { class: "workflow-stage-mark", "aria-hidden": "true", StageGlyph { kind: stage.kind } }
                        strong { class: "workflow-stage-title", "{stage.title}" }
                    }
                    p { class: "workflow-stage-summary", title: "{stage.summary}", "{stage.summary}" }
                    footer { class: "workflow-stage-footer",
                        if let Some(flow) = &stage.flow {
                            if let Some(note) = footer_note { span { "{note}" } }
                            span { class: "curation-stage-count", strong { "{flow.retained}" } " episodes" }
                        } else if !stage.complete {
                            span { class: "curation-stage-incomplete", "Incomplete" }
                        } else {
                            span { "{pending.label()}" }
                        }
                    }
                    if stage.kind != StageKind::Source {
                        Handle { id: "incoming", kind: HandleKind::Target, position: Side::Left }
                    }
                    if stage.kind != StageKind::Output {
                        Handle { id: "retained", kind: HandleKind::Source, position: Side::Right }
                    }
                    if stage.kind == StageKind::Filter && stage.flow.as_ref().is_some_and(|flow| flow.excluded > 0) {
                        Handle { id: "excluded", kind: HandleKind::Source, position: Side::Bottom }
                    }
                }
            }
        }
    }
}

/// The value a builtin or declared fact took for one episode.
pub(super) fn measured_value(verdict: &CurationVerdict, key: &str) -> Option<f64> {
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
}

impl FilterDistribution {
    const BINS: usize = 24;
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
                measured_value(verdict, condition.fact())
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
        let mut bins = vec![DistributionBin::default(); Self::BINS];
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
        })
    }
    fn tallest(&self) -> f64 {
        self.bins
            .iter()
            .map(|bin| bin.retained + bin.excluded)
            .max()
            .unwrap_or(1)
            .max(1) as f64
    }
    /// Where the threshold falls across the plot, 0–100.
    fn threshold_percent(&self) -> Option<f64> {
        self.threshold
            .map(|threshold| 100.0 * (threshold - self.range.0) / (self.range.1 - self.range.0))
    }
}

/// Numbers on an axis: whole when they are whole, else two decimals.
struct AxisNumber(f64);

impl std::fmt::Display for AxisNumber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.fract() == 0.0 && self.0.abs() < 1e12 {
            write!(formatter, "{}", self.0 as i64)
        } else {
            write!(formatter, "{:.2}", self.0)
        }
    }
}

/// How the incoming episodes measured against a numeric filter: one bar per
/// bin, kept in graphite, excluded in the rule's own tint, the threshold as
/// a rule across the plot.
#[component]
pub(super) fn FilterHistogram(
    spec: CurationSpec,
    outcome: CurationOutcome,
    rule_id: String,
    tone: RuleTone,
) -> Element {
    if outcome.totals().truncated()
        || outcome.verdicts().len() as u64 != outcome.totals().episodes()
    {
        return rsx! { p { class: "curation-section-note", "The episode preview is incomplete; a full distribution is unavailable." } };
    }
    let Some(distribution) = FilterDistribution::new(&spec, &outcome, &rule_id) else {
        return rsx! {};
    };
    let tallest = distribution.tallest();
    let (kept, excluded) = distribution
        .bins
        .iter()
        .fold((0, 0), |(kept, excluded), bin| {
            (kept + bin.retained, excluded + bin.excluded)
        });
    let threshold_text = distribution
        .threshold
        .map(|threshold| format!(", threshold {}", AxisNumber(threshold)))
        .unwrap_or_default();
    let label = spec
        .rules()
        .iter()
        .find(|rule| rule.id() == rule_id)
        .map(|rule| rule.condition().sentence(&spec.catalog()))
        .unwrap_or_default();
    rsx! {
        figure { class: "curation-histogram", style: "--stage-tone: {tone.css()}",
            "aria-label": "How incoming episodes measured against {label}",
            div { class: "curation-histogram-plot", role: "img",
                "aria-label": "{kept + excluded} incoming episodes from {AxisNumber(distribution.range.0)} to {AxisNumber(distribution.range.1)}{threshold_text}: {kept} kept, {excluded} excluded",
                for (index, bin) in distribution.bins.iter().enumerate() {
                    div { key: "{index}", class: "curation-histogram-bin",
                        title: "{bin.retained} kept · {bin.excluded} excluded",
                        i { class: "is-kept", style: "height: {bin.retained as f64 / tallest * 100.0:.2}%" }
                        i { class: "is-excluded", style: "height: {bin.excluded as f64 / tallest * 100.0:.2}%" }
                    }
                }
                if let (Some(percent), Some(threshold)) = (distribution.threshold_percent(), distribution.threshold) {
                    span { class: "curation-histogram-threshold", style: "left: {percent:.2}%",
                        "data-side": if percent > 55.0 { "left" } else { "right" },
                        span { "{AxisNumber(threshold)}" }
                    }
                }
            }
            figcaption { class: "curation-histogram-axis",
                span { "{AxisNumber(distribution.range.0)}" }
                span { class: "curation-histogram-legend",
                    span { i { class: "is-kept" } "Kept" }
                    span { i { class: "is-excluded" } "Excluded" }
                    if distribution.missing > 0 { span { "{distribution.missing} without a value" } }
                }
                span { "{AxisNumber(distribution.range.1)}" }
            }
        }
    }
}
