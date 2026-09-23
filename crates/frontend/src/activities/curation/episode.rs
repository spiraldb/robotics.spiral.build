//! One episode's evidence, docked beside the sheet: its statistics, every
//! measured fact, which rules fired and which stayed clear, the transforms
//! it would receive, and the sampled camera frames each camera measurement
//! judged. ↑ and ↓ step through the tiles on view; Escape returns to the tile.

use dioxus::prelude::*;
use spiral_robotics_common::curation::facts::CameraSample;
use spiral_robotics_common::curation::{CurationSpec, rules::TransformOperation};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::{EpisodeQueryRequest, QueryColumn};

use super::sheet::{EPISODE_HEADING_ID, FactValueText, RuleTone, SheetTile};
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, EpisodeFactList, EpisodeInspection};
use crate::components::icons::SpinnerIcon;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Fact, FactList, SheetHeading, SheetPosition, SideSheet,
};

#[component]
pub(super) fn EpisodeSheet(
    tile: SheetTile,
    judged: CurationSpec,
    #[props(default)] selected_stage: Option<String>,
    #[props(default)] inline: bool,
    position: SheetPosition,
    on_step: EventHandler<isize>,
    on_close: EventHandler<()>,
) -> Element {
    let catalog = judged.catalog();
    let verdict = tile.verdict();
    let episode = verdict.episode();
    let dataset = judged.source().cloned();
    let heading = SheetHeading::new(
        EPISODE_HEADING_ID,
        Episode::name_of(episode.id()),
        "episode",
    )
    .with_detail(format!(
        "· {} of {}",
        position.index() + 1,
        position.count()
    ));
    let body = rsx! {
            article { class: "curation-episode", "aria-label": "Episode evidence",
                if !inline || selected_stage.is_none() {
                header { class: "curation-episode-head",
                    span { class: "curation-episode-status", "data-kept": verdict.kept(),
                        span { class: "curation-episode-dot", "aria-hidden": "true" }
                        if verdict.kept() { "Kept" } else { "Excluded" }
                    }
                    h4 { id: inline.then_some(EPISODE_HEADING_ID), tabindex: "-1", class: "curation-episode-title", "{tile.label()}" }
                    if let Some(uri) = episode.source_uri() {
                        p { class: "curation-episode-path", "{uri}" }
                    }
                }
                EpisodeFactList { facts: episode.clone() }
                if !judged.facts().is_empty() {
                    section { class: "curation-evidence",
                        h5 { class: "curation-evidence-title", "Measurements" }
                        FactList { mono: true, class: "curation-kv",
                            for fact in judged.facts() {
                                Fact { label: "{fact.label()}", title: "{FactValueText::new(verdict.fact(fact.key()), fact.kind()).full()}",
                                    "{FactValueText::new(verdict.fact(fact.key()), fact.kind())}" }
                            }
                        }
                    }
                }
                section { class: "curation-evidence",
                    h5 { class: "curation-evidence-title", "Rules" }
                    if judged.rules().is_empty() {
                        p { class: "curation-section-note", "This recipe has no filters; every episode was kept." }
                    }
                    ul { class: "curation-verdict-list",
                        for rule in judged.rules().iter() {
                            li { class: "curation-verdict-row",
                                "data-fired": verdict.excluded_by().iter().any(|fired| fired == rule.id()),
                                style: "--rule-tone: {RuleTone::of(&judged, rule.id()).css()}",
                                span { class: "curation-rule-dot", "aria-hidden": "true" }
                                span { class: "curation-verdict-text", "{rule.condition().sentence(&catalog)}" }
                                span { class: "curation-verdict-word",
                                    if verdict.excluded_by().iter().any(|fired| fired == rule.id()) { "excluded" } else { "clear" }
                                }
                            }
                        }
                    }
                }
                if !judged.transforms().is_empty() {
                    section { class: "curation-evidence",
                        h5 { class: "curation-evidence-title", "Transforms" }
                        ul { class: "curation-verdict-list",
                            for transform in judged.transforms().iter() {
                                li { class: "curation-verdict-row",
                                    "data-fired": verdict.transforms().iter().any(|applied| applied == transform.id()),
                                    span { class: "curation-verdict-text", "{transform.title()}" }
                                    span { class: "curation-verdict-word",
                                        if !verdict.kept() { "not kept" }
                                        else if verdict.transforms().iter().any(|applied| applied == transform.id()) { "applies" }
                                        else { "skipped" }
                                    }
                                }
                            }
                        }
                    }
                }
                }
                if let Some(dataset) = dataset {
                    for transform in judged.transforms().iter().filter(|transform| selected_stage.as_deref() == Some(transform.id())) {
                        match transform.operation() {
                            TransformOperation::SelectProgressChunks { selection } => rsx! {
                                super::progress::ProgressEvidence {
                                    key: "{transform.id()}:{episode.id()}:{selection.scorer().model()}:{selection.sources_relation()}",
                                    episode: tile.episode(dataset.clone()), selection: selection.clone(),
                                }
                            },
                            TransformOperation::ResizeCamera { .. } => rsx! {},
                            _ => rsx! { EpisodeInspection { key: "{transform.id()}:{episode.id()}", episode: tile.episode(dataset.clone()) } },
                        }
                    }
                    for transform in judged.transforms().iter().filter(|transform| selected_stage.as_deref().is_none_or(|id| id == transform.id()) && judged.reaches_step(verdict, transform.id()) && verdict.eligible_transforms().iter().any(|id| id == transform.id())) {
                        if let Some(topic) = transform.operation().topic() {
                            FrameEvidence {
                                key: "transform-{transform.id()}",
                                title: transform.title(),
                                show_heading: selected_stage.is_none(),
                                camera: CameraSample::new(topic.clone()),
                                dataset: dataset.clone(),
                                episode_id: episode.id().clone(),
                                operations: judged.transforms().iter().take_while(|prior| prior.id() != transform.id()).filter(|prior| prior.operation().topic() == Some(topic) && verdict.eligible_transforms().iter().any(|id| id == prior.id())).map(|prior| prior.operation().clone()).chain(std::iter::once(transform.operation().clone())).collect::<Vec<_>>(),
                            }
                        }
                    }
                    for fact in judged.facts().iter().filter(|fact| fact.measure().camera().is_some()) {
                        if let Some(camera) = fact.measure().camera() {
                            FrameEvidence {
                                key: "{fact.key()}",
                                title: fact.label().to_owned(),
                                camera: camera.clone(),
                                dataset: dataset.clone(),
                                episode_id: episode.id().clone(),
                            }
                        }
                    }
                }
            }
    };
    if inline {
        body
    } else {
        rsx! { SideSheet { heading, position, on_step, on_close, {body} } }
    }
}

impl SheetTile {
    /// The episode this tile stands for.
    fn episode(&self, dataset: DatasetSelection) -> Episode {
        Episode::new(dataset, self.verdict().episode().clone())
    }
}

/// The frames a camera measurement decoded for this episode. They load as
/// soon as the episode opens: one bounded query for one episode, so the eye
/// can check the judgement against the picture without another click.
#[component]
fn FrameEvidence(
    title: String,
    camera: CameraSample,
    dataset: DatasetSelection,
    episode_id: EpisodeId,
    #[props(default)] operations: Vec<TransformOperation>,
    #[props(default = true)] show_heading: bool,
) -> Element {
    let original_sql = camera.sample_sql(&dataset.sql_name());
    let mut input = "image".to_owned();
    for operation in operations.iter().take(operations.len().saturating_sub(1)) {
        if let Ok(expression) = operation.image_sql(&input) {
            input = expression;
        }
    }
    let sql = match operations
        .last()
        .and_then(|operation| operation.image_sql(&input).ok())
    {
        Some(derived) => format!(
            "SELECT *, spql.image.width(image) AS image_width, spql.image.height(image) AS image_height, spql.image.width(adjusted_image) AS adjusted_image_width, spql.image.height(adjusted_image) AS adjusted_image_height FROM (SELECT id, {input} AS image, {derived} AS adjusted_image FROM ({original_sql}) samples) compared"
        ),
        None => format!(
            "SELECT *, spql.image.width(image) AS image_width, spql.image.height(image) AS image_height FROM ({original_sql}) samples"
        ),
    };
    let query_episode = EpisodeSelection::new(dataset, episode_id);
    let mut frames = use_resource(use_reactive(
        (&query_episode, &sql),
        move |(episode, sql)| async move {
            let request = EpisodeQueryRequest::new(episode, sql, 16);
            DatasetClient::current()
                .run_episode_query(&request)
                .await
                .map_err(|error| error.to_string())
                .and_then(|snapshot| {
                    let column = snapshot
                        .columns()
                        .iter()
                        .find(|column| column.name() == "image")
                        .cloned()
                        .ok_or("The frame query returned no image column.")?;
                    let adjusted = snapshot
                        .columns()
                        .iter()
                        .find(|column| column.name() == "adjusted_image");
                    let images = snapshot
                        .rows()
                        .iter()
                        .filter_map(|row| {
                            EvidenceImage::from_row(&column, row).map(|original| {
                                (
                                    original,
                                    adjusted
                                        .and_then(|column| EvidenceImage::from_row(column, row)),
                                )
                            })
                        })
                        .collect::<Vec<_>>();
                    if images.is_empty() {
                        return Err(
                            "The camera recorded no frame at the sampled positions.".to_owned()
                        );
                    }
                    Ok(images)
                })
        },
    ));
    let frame_count = camera.frames().len();
    rsx! {
        section { class: "curation-evidence", "aria-label": "{title} · {camera.topic().topic()}",
            if show_heading { h5 { class: "curation-evidence-title", "{title} · {camera.topic().topic()}" } }
            match frames.read().as_ref() {
                Some(Ok(images)) => rsx! {
                    div { class: "curation-frames",
                        for (index, (image, adjusted)) in images.iter().enumerate() {
                            figure { class: "curation-frame",
                                img { src: "{image.url}", alt: "Original {title}, sampled frame {index + 1}" }
                                figcaption { "Before · {image.dimensions()} · frame {camera.frames().get(index).copied().unwrap_or(index as u32)}" }
                            }
                            if let Some(adjusted) = adjusted {
                                figure { class: "curation-frame",
                                    img { src: "{adjusted.url}", alt: "Adjusted {title}, sampled frame {index + 1}" }
                                    figcaption { "After · {adjusted.dimensions()} · same frame" }
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    p { class: "curation-section-note is-alert", role: "alert", "{error}" }
                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, onclick: move |_| frames.restart(), "Try again" }
                },
                None => rsx! {
                    p { class: "curation-section-note curation-frames-loading", role: "status",
                        SpinnerIcon {}
                        if frame_count == 1 { "Decoding the sampled frame…" } else { "Decoding {frame_count} sampled frames…" }
                    }
                },
            }
        }
    }
}

/// Dimensions come from the native image value, independent of CSS display size.
struct EvidenceImage {
    url: String,
    size: Option<(u64, u64)>,
}

impl EvidenceImage {
    fn from_row(
        column: &QueryColumn,
        row: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        let url = column.image_data_url(row.get(column.name()))?.to_owned();
        let size = row
            .get(&format!("{}_width", column.name()))
            .and_then(serde_json::Value::as_u64)
            .zip(
                row.get(&format!("{}_height", column.name()))
                    .and_then(serde_json::Value::as_u64),
            );
        Some(Self { url, size })
    }
    fn dimensions(&self) -> String {
        self.size
            .map(|(width, height)| format!("{width}×{height} px"))
            .unwrap_or_else(|| "Dimensions unavailable".into())
    }
}
