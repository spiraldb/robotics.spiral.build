//! Query editor and relation summaries for the Data route.
use crate::activities::data::query_result::DEFAULT_QUERY_ROWS;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
const QUERY_ROW_OPTIONS: [usize; 5] = [25, 100, 250, 500, 1_000];

#[component]
pub(super) fn QueryPanel(
    studio: StudioProjectState,
    controller: super::state::QueryController,
) -> Element {
    let query_available = crate::shell::project::ProjectSessionSignals::current()
        .mounted_session()
        .is_some_and(|session| !session.project_id().trim().is_empty());
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
