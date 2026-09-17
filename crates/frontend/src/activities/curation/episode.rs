//! One episode's evidence, docked beside the sheet: its statistics, every
//! measured fact, which rules fired and which stayed clear, the transforms
//! it would receive, and the sampled camera frames each camera measurement
//! judged. ↑ and ↓ step through the tiles on view; Escape returns to the tile.

use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationSpec;
use spiral_robotics_common::curation::facts::CameraSample;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;

use super::sheet::{EPISODE_HEADING_ID, FactValueText, RuleTone, SheetTile};
use crate::activities::data::DatasetClient;
use crate::components::icons::SpinnerIcon;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::components::side_sheet::{SheetHeading, SheetPosition, SideSheet};
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, UtcTimestampPresentation,
};

#[component]
pub(super) fn EpisodeSheet(
    tile: SheetTile,
    judged: CurationSpec,
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
        format!("Episode {}", tile.ordinal()),
        "episode",
    )
    .with_detail(format!("of {}", position.count()));
    rsx! {
        SideSheet { heading, position, on_step, on_close,
            article { class: "curation-episode", "aria-label": "Episode evidence",
                header { class: "curation-episode-head",
                    span { class: "curation-episode-status", "data-kept": verdict.kept(),
                        span { class: "curation-episode-dot", "aria-hidden": "true" }
                        if verdict.kept() { "Kept" } else { "Excluded" }
                    }
                    h4 { class: "curation-episode-title", "{tile.label()}" }
                    if let Some(uri) = episode.source_uri() {
                        p { class: "curation-episode-path", "{uri}" }
                    }
                }
                dl { class: "curation-stat-grid",
                    div { class: "curation-stat", dt { "Recorded" } dd { "{episode.message_start_time().map_or_else(|| \"—\".into(), UtcTimestampPresentation::utc_label)}" } }
                    div { class: "curation-stat", dt { "Duration" } dd { "{episode.duration_seconds().map_or_else(|| \"—\".into(), ClockPresentation::clock_label)}" } }
                    div { class: "curation-stat", dt { "Messages" } dd { "{episode.message_count().map_or_else(|| \"—\".into(), GroupedCountPresentation::grouped_label)}" } }
                    div { class: "curation-stat", dt { "Channels" } dd { "{episode.channel_count().map_or_else(|| \"—\".into(), GroupedCountPresentation::grouped_label)}" } }
                    div { class: "curation-stat", dt { "File" } dd { "{episode.source_bytes().map_or_else(|| \"—\".into(), ByteCountPresentation::byte_label)}" } }
                }
                if !judged.facts().is_empty() {
                    section { class: "curation-evidence",
                        h5 { class: "curation-evidence-title", "Measurements" }
                        dl { class: "curation-kv",
                            for fact in judged.facts() {
                                dt { "{fact.label()}" }
                                dd { title: "{FactValueText::new(verdict.fact(fact.key()), fact.kind()).full()}",
                                    "{FactValueText::new(verdict.fact(fact.key()), fact.kind())}" }
                            }
                        }
                    }
                }
                section { class: "curation-evidence",
                    h5 { class: "curation-evidence-title", "Rules" }
                    if judged.rules().iter().all(|rule| !rule.enabled()) {
                        p { class: "curation-section-note", "No rule was active; every episode was kept." }
                    }
                    ul { class: "curation-verdict-list",
                        for (index, rule) in judged.rules().iter().enumerate().filter(|(_, rule)| rule.enabled()) {
                            li { class: "curation-verdict-row",
                                "data-fired": verdict.excluded_by().iter().any(|fired| fired == rule.id()),
                                style: "--rule-tone: {RuleTone::at(index).css()}",
                                span { class: "curation-rule-dot", "aria-hidden": "true" }
                                span { class: "curation-verdict-text", "{rule.condition().sentence(&catalog)}" }
                                span { class: "curation-verdict-word",
                                    if verdict.excluded_by().iter().any(|fired| fired == rule.id()) { "excluded" } else { "clear" }
                                }
                            }
                        }
                    }
                }
                if judged.transforms().iter().any(|transform| transform.enabled()) {
                    section { class: "curation-evidence",
                        h5 { class: "curation-evidence-title", "Transforms" }
                        ul { class: "curation-verdict-list",
                            for transform in judged.transforms().iter().filter(|transform| transform.enabled()) {
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
                if let Some(dataset) = dataset {
                    for fact in judged.facts().iter().filter(|fact| fact.measure().camera().is_some()) {
                        if let Some(camera) = fact.measure().camera() {
                            FrameEvidence {
                                key: "{fact.key()}",
                                title: fact.label().to_owned(),
                                camera: camera.clone(),
                                dataset: dataset.clone(),
                                episode_id: episode.id().to_owned(),
                            }
                        }
                    }
                }
            }
        }
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
    episode_id: String,
) -> Element {
    let request = EpisodeQueryRequest::new(
        EpisodeSelection::new(dataset.clone(), episode_id),
        camera.sample_sql(&dataset.sql_name()),
        16,
    );
    let mut frames = use_resource(move || {
        let request = request.clone();
        async move {
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
                    let images = snapshot
                        .rows()
                        .iter()
                        .filter_map(|row| {
                            column.image_data_url(row.get("image")).map(str::to_owned)
                        })
                        .collect::<Vec<_>>();
                    if images.is_empty() {
                        return Err(
                            "The camera recorded no frame at the sampled positions.".to_owned()
                        );
                    }
                    Ok(images)
                })
        }
    });
    let frame_count = camera.frames().len();
    rsx! {
        section { class: "curation-evidence",
            h5 { class: "curation-evidence-title", "{title} · {camera.topic().topic()}" }
            match frames.read().as_ref() {
                Some(Ok(images)) => rsx! {
                    div { class: "curation-frames",
                        for (index, image) in images.iter().enumerate() {
                            figure { class: "curation-frame",
                                img { src: "{image}", alt: "{title}, sampled frame {index + 1}" }
                                figcaption { "frame {camera.frames().get(index).copied().unwrap_or(index as u32)}" }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    p { class: "curation-section-note is-alert", role: "alert", "{error}" }
                    Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| frames.restart(), "Try again" }
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
