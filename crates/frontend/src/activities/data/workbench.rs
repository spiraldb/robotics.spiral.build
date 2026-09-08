//! Query editor and relation summaries for the Data route.
use crate::activities::data::catalog::ManagedTable as Table;
use crate::activities::data::query_result::DEFAULT_QUERY_ROWS;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::relations::RelationCatalog;
const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];

#[component]
pub(crate) fn QueryPanel(
    studio: StudioProjectState,
    controller: super::state::QueryController,
    catalog: super::relations::RelationCatalogState,
) -> Element {
    let configuration = studio.configuration();
    let namespace = configuration.dataset();
    let dataset_id = "default";
    let query_available = configuration.spiral_query_available();
    let catalog_available =
        crate::shell::project::ProjectSessionSignals::current().catalog_available();
    let relation_result = catalog.relation_catalog_result();
    let relation_error = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .cloned();
    let relation_response = relation_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let schema_relations = SchemaRelationSummary::for_dataset(
        namespace,
        dataset_id,
        query_available.then_some(relation_response).flatten(),
    );
    let can_query = query_available;
    let query_running = controller.query_running();
    let run_query = move || controller.run_query(studio, can_query);

    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface text-ink", "aria-label": "Query",
            if !query_available {
                div { class: "mx-2 mt-2 rounded border border-line bg-sunken px-2 py-1.5 text-[11px] text-ink-soft", role: "status",
                    strong { class: "block font-medium text-ink", "Configure SpiralDB to run Query" }
                    span { "Open Settings and configure a Spiral catalog, project, and namespace." }
                }
            }
            details { class: "query-schema-browser",
                summary {
                    span { "Database relations" }
                    small {
                        if !catalog_available {
                            "Waiting for project"
                        } else if relation_result.is_none() && query_available {
                            "Discovering…"
                        } else {
                            "{schema_relations.len()} relations · {namespace}"
                        }
                    }
                }
                div { class: "query-schema-list",
                    if let Some(problem) = relation_error {
                        div { class: "query-schema-error", role: "alert",
                            span { "Catalog discovery failed" }
                            Action {
                                purpose: ActionPurpose::Retry,
                                variant: ActionVariant::Link,
                                onclick: move |_| super::relations::RelationCatalogState::refresh(),
                                "Retry"
                            }
                            small { "{problem}" }
                        }
                    }
                    for relation in schema_relations.iter().cloned() {
                        Action {
                            key: "{relation.name}",
                            purpose: ActionPurpose::Apply,
                            variant: ActionVariant::Row,
                            class: "query-schema-row",
                            title: "Query {relation.name}",
                            onclick: move |_| controller.set_sql(relation.sql.clone()),
                            span {
                                strong { "{relation.label}" }
                                small { "{relation.description}" }
                            }
                            code { "{relation.columns}" }
                        }
                    }
                }
            }
            label { class: "flex min-h-0 flex-1 flex-col overflow-hidden px-2 pt-2",
                span { class: "label", "Read-only SQL" }
                // `min-h-0`, not `min-h-32`. A 112px floor on a box whose parent
                // is free to shrink to 37px does not make the box taller — it
                // makes it hang 75px out of the bottom of its own panel, with
                // the Max rows select and Run query drawn on top of the SQL.
                textarea {
                    class: "query-input min-h-0 flex-1 resize-none",
                    spellcheck: "false",
                    value: controller.sql(),
                    oninput: move |event| controller.set_sql(event.value()),
                    onkeydown: move |event| {
                        if event.key() == Key::Enter
                            && (event.modifiers().ctrl() || event.modifiers().meta())
                        {
                            event.prevent_default();
                            run_query();
                        }
                    },
                }
            }
            div { class: "flex shrink-0 items-end gap-2 px-2 py-2",
                label { class: "w-24",
                    span { class: "label", "Max rows" }
                    select {
                        class: "field",
                        value: "{controller.max_rows()}",
                        onchange: move |event| {
                            let rows = event.value().parse().unwrap_or(DEFAULT_QUERY_ROWS);
                            controller.set_max_rows(rows);
                        },
                        for option in QUERY_ROW_OPTIONS {
                            option { value: "{option}", selected: controller.max_rows() == option, "{option}" }
                        }
                    }
                }
                p { class: "min-w-[7rem] flex-1 text-[10px] leading-snug text-ink-faint",
                    span { class: "block", "Keeps newest 8" }
                    span { class: "block whitespace-nowrap", "⌘/Ctrl Enter" }
                }
                if query_running {
                    Action {
                        purpose: ActionPurpose::Cancel,
                        variant: ActionVariant::Outline,
                        onclick: move |_| controller.cancel_query(),
                        "Cancel query"
                    }
                } else {
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
                        disabled: !can_query || controller.sql().trim().is_empty(),
                        onclick: move |_| run_query(),
                        "Run query"
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SchemaRelationSummary {
    name: String,
    label: String,
    description: String,
    columns: String,
    sql: String,
}

impl SchemaRelationSummary {
    fn for_dataset(
        namespace: &str,
        dataset_id: &str,
        response: Option<&RelationCatalog>,
    ) -> Vec<Self> {
        let Some(response) = response else {
            return Vec::new();
        };
        response
            .relations()
            .iter()
            .map(|relation| {
                let managed = Table::ALL
                    .into_iter()
                    .find(|table| relation.name() == table.qualified_name(namespace));
                let label = managed
                    .map(|table| table.name().to_owned())
                    .unwrap_or_else(|| {
                        relation
                            .name()
                            .strip_prefix(&format!("{namespace}."))
                            .unwrap_or(relation.name())
                            .to_owned()
                    });
                Self {
                    name: relation.name().to_owned(),
                    label,
                    description: managed
                        .map(ManagedTablePresentation::description)
                        .unwrap_or(relation.relation_type())
                        .to_owned(),
                    columns: relation
                        .columns()
                        .iter()
                        .map(|column| column.name())
                        .collect::<Vec<_>>()
                        .join(", "),
                    sql: managed
                        .map(|table| table.scoped_sql(namespace, dataset_id))
                        .unwrap_or_else(|| format!("SELECT * FROM {} LIMIT 100", relation.name())),
                }
            })
            .collect()
    }
}

/// The workbench narrows a relation's default query to the selected dataset
/// wherever the relation can reach one; the rest keep the shared preset.
pub(crate) trait QueryPresetSql {
    fn scoped_sql(self, namespace: &str, dataset_id: &str) -> String;
}

impl QueryPresetSql for Table {
    fn scoped_sql(self, namespace: &str, dataset_id: &str) -> String {
        let dataset_id = dataset_id.replace('\'', "''");
        let episodes = Table::Episodes.qualified_name(namespace);
        let relation = self.qualified_name(namespace);
        match self {
            Table::Episodes => format!(
                "SELECT * FROM {relation} WHERE dataset_id = '{dataset_id}' ORDER BY started_ms DESC LIMIT 100"
            ),
            Table::EpisodeStreams => format!(
                "SELECT s.* FROM {relation} AS s JOIN {episodes} AS e ON e.episode_id = s.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY s.episode_id DESC, s.stream_id LIMIT 100"
            ),
            Table::JointSamples => format!(
                "SELECT j.* FROM {relation} AS j JOIN {episodes} AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
            ),
            Table::VideoAssets => format!(
                "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM {relation} AS v JOIN {episodes} AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = '{dataset_id}' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
            ),
            Table::Datasets
            | Table::TrainingCheckpoints
            | Table::Robots
            | Table::StudioSettings
            | Table::TrainingRuns
            | Table::TrainingRunMetrics
            | Table::TrainingRunCheckpoints
            | Table::RolloutRuns
            | Table::WorkflowRuns
            | Table::FeatureRegistry => self.sql(namespace),
        }
    }
}

trait ManagedTablePresentation {
    fn description(self) -> &'static str;
}

impl ManagedTablePresentation for Table {
    fn description(self) -> &'static str {
        match self {
            Table::Datasets => "Dataset identity and metadata",
            Table::Episodes => "Completed episodes and their datasets",
            Table::EpisodeStreams => "Available joint and camera streams",
            Table::JointSamples => "Synchronized state and action samples",
            Table::VideoAssets => "Recorded MP4 segments and media metadata",
            Table::TrainingCheckpoints => "Saved checkpoints by training run",
            Table::Robots => "Robot configuration and hardware assignments",
            Table::StudioSettings => "Persistent Studio preferences",
            Table::TrainingRuns => "Training run configuration and lifecycle",
            Table::TrainingRunMetrics => "Per-step metrics for each training run",
            Table::TrainingRunCheckpoints => "Checkpoint history for each training run",
            Table::RolloutRuns => "Policy rollout lifecycle and outcomes",
            Table::WorkflowRuns => "Saved Workflow graph and latest run snapshot",
            Table::FeatureRegistry => "Registered Workflow feature definitions and provenance",
        }
    }
}

pub(crate) trait EpisodeIdPresentation {
    fn short_episode_id(&self) -> String;
}

impl EpisodeIdPresentation for str {
    fn short_episode_id(&self) -> String {
        self.strip_prefix("episode_")
            .and_then(|number| number.parse::<u64>().ok())
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| self.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn presets_are_scoped_to_the_current_dataset() {
        assert_eq!(
            Table::JointSamples.scoped_sql("training", "pick-place"),
            "SELECT j.* FROM training.joint_samples AS j JOIN training.episodes AS e ON e.episode_id = j.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY j.episode_id DESC, j.stream_id, j.sample_index LIMIT 100"
        );
        assert_eq!(
            Table::Episodes.scoped_sql("training", "pick-'place"),
            "SELECT * FROM training.episodes WHERE dataset_id = 'pick-''place' ORDER BY started_ms DESC LIMIT 100"
        );
        assert_eq!(
            Table::VideoAssets.scoped_sql("training", "pick-place"),
            "SELECT v.episode_id, v.stream_id, v.segment_index, v.camera_id, v.byte_size, v.width, v.height, v.fps, v.frames FROM training.video_assets AS v JOIN training.episodes AS e ON e.episode_id = v.episode_id WHERE e.dataset_id = 'pick-place' ORDER BY v.episode_id DESC, v.stream_id, v.segment_index LIMIT 100"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn generated_episode_ids_read_as_take_numbers() {
        assert_eq!("episode_000024".short_episode_id(), "#24");
        assert_eq!("imported-pass".short_episode_id(), "imported-pass");
    }
}
