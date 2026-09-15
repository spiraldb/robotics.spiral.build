use super::*;
#[component]
pub(super) fn ComparisonEpisodes(
    result: ComparisonResult,
    selection: ComparisonEvidence,
    on_select: EventHandler<ComparisonEvidence>,
    on_inspect: EventHandler<serde_json::Map<String, serde_json::Value>>,
) -> Element {
    let request = selection.clone();
    let data = use_resource(use_reactive(
        (&result, &selection),
        move |(result, selection)| async move { result.evidence(&selection).await },
    ));
    let pending = *data.state().read() == UseResourceState::Pending;
    let response = data.read();
    let offset = selection.offset();
    let bins = selection.bin().and_then(|(metric, bin)| {
        result
            .plan()
            .metrics()
            .get(metric)
            .zip(result.report().metrics().get(metric))
            .map(|(metric, distribution)| {
                format!(
                    "{} · {} {}",
                    metric.label(),
                    distribution.bin_label(bin),
                    metric.unit(),
                )
            })
    });
    rsx! {
        section { class: "comparison-episodes", "aria-label": "Comparison episodes",
            header {
                h2 { id: "comparison-evidence-heading", tabindex: "-1", "Inspect the episodes" }
                div {
                    class: "comparison-cohort-tabs",
                    role: "group",
                    "aria-label": "Evidence population",
                    for (cohort, label) in [(WorkflowSide::Left, "Workflow A"), (WorkflowSide::Right, "Workflow B")] {
                        button {
                            "aria-pressed": selection.cohort() == cohort,
                            disabled: pending,
                            onclick: {
                                let request = request.clone();
                                move |_| on_select.call(ComparisonEvidence::new(cohort).with_bin(request.bin()))
                            },
                            "{label}"
                        }
                    }
                }
            }
            if let Some(label) = bins {
                div { class: "comparison-bin-filter",
                    span { "{label}" }
                    button {
                        class: "comparison-text-button",
                        disabled: pending,
                        onclick: {
                            let request = request.clone();
                            move |_| on_select.call(request.clone().with_bin(None))
                        },
                        "Clear bin filter"
                    }
                }
            }
            if pending {
                div { class: "comparison-evidence-loading", role: "status",
                    "Loading matching episodes from Spiral…"
                }
            } else {
                match &*response {
                    Some(Err(message)) => rsx! {
                        p { class: "comparison-error", role: "alert", "{message}" }
                    },
                    Some(Ok(snapshot)) if snapshot.rows().is_empty() => rsx! {
                        p { class: "comparison-evidence-loading",
                            "No episodes in this selection. Choose another population or clear the bin filter."
                        }
                    },
                    Some(Ok(snapshot)) => rsx! {
                        div { class: "comparison-evidence-table",
                            table {
                                thead {
                                    tr {
                                        th { "Episode" }
                                        th { "Metric value" }
                                        th { "Duration" }
                                        th { "Messages" }
                                        th {}
                                    }
                                }
                                tbody {
                                    for (index, row) in snapshot.rows().iter().take(20).enumerate() {
                                        {
                                            let entry = EpisodeEvidence(row);
                                            let selected_row = row.clone();
                                            let button_row = row.clone();
                                            rsx! {
                                                tr { key: "{entry.id()}",
                                                    td {
                                                        button {
                                                            class: "comparison-episode-link",
                                                            onclick: move | _ | on_inspect
                                                                    .call(selected_row.clone()),
                                                            "{entry.label(offset + index + 1)}"
                                                        }
                                                        span { class: "comparison-episode-id", "{entry.id()}" }
                                                    }
                                                    td { class: "comparison-numeric", {entry.number("criterion_value")} }
                                                    td { class: "comparison-numeric", {format!("{} s", entry.number("duration_seconds"))} }
                                                    td { class: "comparison-numeric", {entry.number("message_count")} }
                                                    td {
                                                        button {
                                                            class: "comparison-text-button",
                                                            onclick: move | _ | on_inspect.call(button_row
                                                                    .clone()),
                                                            "Inspect recording"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        footer { class: "comparison-evidence-pagination",
                            span { "{offset + 1}–{offset + snapshot.rows().len().min(20)} episodes · current dataset version" }
                            div {
                                button {
                                    class: "comparison-text-button",
                                    disabled: offset == 0,
                                    onclick: {
                                        let request = request.clone();
                                        move |_| on_select.call(request.clone().with_offset(offset.saturating_sub(20)))
                                    },
                                    "Previous"
                                }
                                button {
                                    class: "comparison-text-button",
                                    disabled: snapshot.rows().len() <= 20,
                                    onclick: {
                                        let request = request.clone();
                                        move |_| on_select.call(request.clone().with_offset(offset + 20))
                                    },
                                    "Next"
                                }
                            }
                        }
                    },
                    None => rsx! {},
                }
            }
        }
    }
}
struct EpisodeEvidence<'a>(&'a serde_json::Map<String, serde_json::Value>);
impl EpisodeEvidence<'_> {
    fn id(&self) -> &str {
        self.0
            .get("episode_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown episode")
    }
    fn label(&self, ordinal: usize) -> String {
        self.0
            .get("source_uri")
            .and_then(serde_json::Value::as_str)
            .and_then(|uri| uri.rsplit('/').find(|part| !part.is_empty()))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Episode {ordinal}"))
    }
    fn number(&self, column: &str) -> String {
        MetricNumber(self.0.get(column).and_then(serde_json::Value::as_f64)).display()
    }
}
