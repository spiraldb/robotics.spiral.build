//! One panel per recorded topic, native tab layout, and one episode clock.
use super::{
    episode_browser::EpisodeContext,
    episode_topic::{EpisodeTopic, TopicSource},
    episode_transport::{EpisodeTimeRange, EpisodeTimeline, EpisodeTransport},
};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeTopicRequest, MessageRead};
use std::collections::BTreeSet;

#[derive(Clone, PartialEq)]
struct EpisodePanelCatalog {
    topics: Vec<DatasetTopic>,
    groups: Vec<Vec<usize>>,
}
impl EpisodePanelCatalog {
    fn new(topics: Vec<DatasetTopic>) -> Self {
        let mut ranked = (0..topics.len()).collect::<Vec<_>>();
        ranked.sort_by_key(|index| std::cmp::Reverse(topics[*index].samples().unwrap_or(0)));
        let mut groups = ranked
            .iter()
            .take(4)
            .map(|index| vec![*index])
            .collect::<Vec<_>>();
        for index in ranked.into_iter().skip(4) {
            let schema = topics[index].schemas();
            let group = groups
                .iter_mut()
                .min_by_key(|group| {
                    let related = !schema.is_empty()
                        && group
                            .iter()
                            .any(|member| topics[*member].schemas() == schema);
                    (!related, group.len())
                })
                .expect("overflow topics have four initial groups");
            group.push(index);
        }
        Self { topics, groups }
    }
    fn home(&self, index: usize) -> String {
        let group = self
            .groups
            .iter()
            .position(|group| group.contains(&index))
            .expect("every topic has a group");
        format!("topic-group-{group}")
    }

    fn panel_id(&self, index: usize) -> PanelId {
        PanelId::new(format!("topic-{index}"))
    }
    fn label(&self, index: usize) -> String {
        let topic = &self.topics[index];
        if self
            .topics
            .iter()
            .filter(|other| other.name() == topic.name())
            .count()
            > 1
        {
            format!(
                "{} · {}",
                topic.name(),
                topic
                    .revision()
                    .map_or("unknown", |revision| revision.variant())
            )
        } else {
            topic.name().into()
        }
    }
    fn source(&self, index: usize, episode: &EpisodeContext) -> Option<TopicSource> {
        let topic = &self.topics[index];
        let selection = episode.selection();
        Some(TopicSource::new(
            MessageRead::schema(
                EpisodeTopicRequest::new(
                    selection.dataset().clone(),
                    topic.name().into(),
                    selection.episode_id().into(),
                ),
                topic.revision()?.clone(),
            ),
            topic,
        ))
    }
    fn initial_layout(&self) -> PanelLayout {
        let mut tiles = self.groups.iter().map(|group| {
            LayoutNode::tile(
                self.home(group[0]),
                group.iter().map(|index| self.panel_id(*index)),
            )
        });
        let first = tiles
            .next()
            .unwrap_or_else(|| LayoutNode::tile("topics", Vec::<PanelId>::new()));
        let Some(second) = tiles.next() else {
            return PanelLayout::new(first);
        };
        let top = LayoutNode::split("topic-top", SplitAxis::Horizontal, 0.5, first, second);
        let Some(third) = tiles.next() else {
            return PanelLayout::new(top);
        };
        let bottom = match tiles.next() {
            Some(fourth) => {
                LayoutNode::split("topic-bottom", SplitAxis::Horizontal, 0.5, third, fourth)
            }
            None => third,
        };
        PanelLayout::new(LayoutNode::split(
            "topic-rows",
            SplitAxis::Vertical,
            0.5,
            top,
            bottom,
        ))
    }
}

#[component]
pub(super) fn EpisodeWorkspace(
    episode: EpisodeContext,
    topics: Vec<DatasetTopic>,
    timeline: EpisodeTimeRange,
    on_back: EventHandler,
) -> Element {
    let catalog = use_hook(|| EpisodePanelCatalog::new(topics));
    let mut opened = use_signal(|| (0..catalog.topics.len()).collect::<BTreeSet<_>>());
    let mut active = use_signal(|| None::<PanelId>);
    let transport = EpisodeTransport::use_transport(timeline);
    let mut keyboard = transport.clone();
    let panels = opened
        .read()
        .iter()
        .filter_map(|index| {
            let source = catalog.source(*index, &episode)?;
            Some(
                Panel::new(
                    catalog.panel_id(*index),
                    catalog.label(*index),
                    catalog.home(*index),
                    rsx! { EpisodeTopic { source, timeline, transport: transport.clone() } },
                )
                .with_closable(true)
                .with_class("episode-stream-panel"),
            )
        })
        .collect::<Vec<_>>();
    let layout = use_hook(|| catalog.initial_layout());
    rsx! {
        section { class: "episode-workspace", "aria-label": "Recorded episode inspection", tabindex: "0",
            onkeydown: move |event: KeyboardEvent| {
                if event.key() == Key::Character(" ".into()) && !event.is_auto_repeating() && !EpisodeShortcuts::editing() {
                    event.prevent_default(); event.stop_propagation(); keyboard.toggle();
                }
            },
            header { class: "episode-workspace-header",
                nav { "aria-label": "Episode navigation",
                    button { class: "data-text-action", onclick: move |_| on_back.call(()), "Episodes" }
                    span { "/" }
                    strong { title: episode.dataset().label(), "{episode.title()}" }
                }
                details { class: "episode-topics-menu",
                    summary { "Topics ({catalog.topics.len()})" }
                    div { class: "episode-topics-popover",
                        table { class: "data-ledger", "aria-label": "Episode topics",
                            thead { tr { th { "Recorded topic" } th { "Open" } } }
                            tbody { for (index, topic) in catalog.topics.iter().enumerate() {
                                tr {
                                    td { span { class: "episode-topic-name", "{topic.name()}" } small { "{topic.schemas().join(\", \")}" } }
                                    td {
                                        button { class: "data-text-action", onclick: { let id = catalog.panel_id(index); move |_| { opened.write().insert(index); active.set(Some(id.clone())); } }, "Show" }
                                    }
                                }
                            } }
                        }
                        p { "Split topic tabs to compare them on the shared timeline." }
                    }
                }
                details { class: "episode-details-menu",
                    summary { "Details" }
                    div { class: "episode-details-popover", {episode.render_summary()} {episode.render_details()} }
                }
            }
            div { class: "episode-panel-workspace",
                if opened.read().is_empty() { p { class: "data-empty-copy", "Choose a topic from Topics to reopen its panel." } }
                else {
                PanelWorkspace { panels, initial_layout: layout, active_panel: active(),
                    on_panel_close: move |id: PanelId| { if let Some(index) = id.as_str().strip_prefix("topic-").and_then(|index| index.parse::<usize>().ok()) { opened.write().remove(&index); } active.set(None); },
                    on_panel_activate: move |id: PanelId| active.set(Some(id)),
                }
                }
            }
            EpisodeTimeline { transport }
        }
    }
}
struct EpisodeShortcuts;
impl EpisodeShortcuts {
    fn editing() -> bool {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .is_some_and(|element| {
                matches!(
                    element.tag_name().as_str(),
                    "INPUT" | "SELECT" | "TEXTAREA" | "BUTTON" | "SUMMARY"
                ) || element.get_attribute("contenteditable").is_some()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn overflow_uses_recorded_schema_without_topic_name_assumptions() {
        let topics = [
            ("sensor.a", 100),
            ("sensor.b", 90),
            ("sensor.c", 80),
            ("sensor.d", 70),
            ("sensor.a", 5),
            ("other", 3),
            ("other", 2),
            ("", 1),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (schema, samples))| {
            DatasetTopic::new(
                format!("/arbitrary/{index}"),
                if schema.is_empty() {
                    vec![]
                } else {
                    vec![schema.into()]
                },
            )
            .with_coverage(1, Some(samples))
        })
        .collect::<Vec<_>>();
        let catalog = EpisodePanelCatalog::new(topics.clone());
        assert_eq!(
            catalog.groups,
            vec![vec![0, 4], vec![1, 5, 6], vec![2, 7], vec![3]]
        );
        let renamed = topics
            .into_iter()
            .enumerate()
            .map(|(index, topic)| {
                DatasetTopic::new(format!("/camera/renamed-{index}"), topic.schemas().to_vec())
                    .with_coverage(1, topic.samples())
            })
            .collect();
        assert_eq!(EpisodePanelCatalog::new(renamed).groups, catalog.groups);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn initial_panes_prioritize_recorded_samples_and_handle_small_catalogs() {
        let topics = [Some(1), Some(30), None, Some(20), Some(10)]
            .into_iter()
            .enumerate()
            .map(|(index, count)| {
                DatasetTopic::new(format!("/topic/{index}"), vec!["shared".into()])
                    .with_coverage(1, count)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            EpisodePanelCatalog::new(topics.clone()).groups,
            vec![vec![1, 2], vec![3], vec![4], vec![0]]
        );
        for count in 0..=4 {
            let catalog = EpisodePanelCatalog::new(topics[..count].to_vec());
            assert_eq!(catalog.groups.len(), count);
            assert_eq!(
                catalog
                    .groups
                    .iter()
                    .flatten()
                    .copied()
                    .collect::<BTreeSet<_>>(),
                (0..count).collect()
            );
            catalog.initial_layout();
        }
    }
}
