//! Read-only SQL over the project, with its results directly beneath.
use super::{state::QueryController, workbench::QueryPanel};
use crate::activities::data::query_result::QueryResultView;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;

#[component]
pub(super) fn SqlPanel(studio: StudioProjectState, controller: QueryController) -> Element {
    rsx! {
        div { class: "data-sql",
            div { class: "data-sql-editor", QueryPanel { studio, controller } }
            div { class: "data-sql-results", QueryResultView { controller } }
        }
    }
}
