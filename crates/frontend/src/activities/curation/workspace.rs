//! Curation contact sheet: every episode as a tile on one sheet, the recipe
//! that judges them in a tall column at the left, and the selected
//! episode's evidence in a side sheet beside the tiles.
//!
//! Direction contract (seed dd03151f, structure 6 of the grounded list),
//! distilled after the finish review.
//! THESIS: curating is marking a contact sheet. The whole dataset is visible
//! at once; kept episodes stay in full ink, excluded ones dim and carry the
//! rule that removed them in that rule's own tint. This refuses the
//! query-builder-over-a-results-table arrangement.
//! OWN-WORLD: the one-light bench unchanged: paper, graphite, hairlines, 4px
//! radii, 12/11/10px type, tabular mono numerals. Added: six rule tints from
//! the result-category washes, dimmed struck tiles, a 2px duration rule under
//! each tile, and one highlight sweep when a rule is touched.
//! STORY: name a dataset, write "if this then exclude" in words, run, watch
//! the sheet settle into kept and struck tiles, touch a rule to see what it
//! removed, open one tile to read its facts beside the sheet and step
//! through its neighbours with the arrow keys.
//! FIRST VIEWPORT: one 36px document bar (glyph · name ▾ · Saved · Discard
//! Save ▶Run); left 26% Recipe (Source, Rules, Add a rule); right: the
//! Sheet, a pipeline strip of Workflow's stage cards over one stage heading
//! and that stage's tiles, with the selected episode's side sheet docked at
//! its right edge while one is open.
//! FORM: pipeline strip over a contact sheet, beside a recipe column.
//! FINISH: unreviewed and undocumented is unfinished; this build ends with the
//! finish review, the verdict, and DESIGN.md.

use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

use super::state::{CurationController, RECIPE_PANEL_ID, SHEET_PANEL_ID};
use crate::components::icons::{CurationIcon, DetailsIcon};

const CURATION_LAYOUT_SCOPE: &str = "curation-sheet-v2";

#[component]
fn CurationPanels(controller: CurationController) -> Element {
    let panels = vec![
        Panel::new(
            RECIPE_PANEL_ID,
            "Recipe",
            "curation-recipe",
            rsx! { super::recipe::RecipePanel { controller } },
        )
        .with_class("curation-recipe-editor")
        .with_tab_icon(rsx! { DetailsIcon {} }),
        Panel::new(
            SHEET_PANEL_ID,
            "Sheet",
            "curation-sheet",
            rsx! { super::sheet::SheetPanel { controller } },
        )
        .with_class("curation-sheet-editor")
        .with_tab_icon(rsx! { CurationIcon {} }),
    ];
    let studio = crate::shell::state::StudioProjectState::current();
    rsx! {
        div { class: "curation-workspace",
            // ⌘/Ctrl+Enter runs from anywhere in the recipe, like Send in a chat.
            onkeydown: move |event: KeyboardEvent| {
                let modifiers = event.modifiers();
                if event.key() == Key::Enter
                    && (modifiers.contains(Modifiers::META) || modifiers.contains(Modifiers::CONTROL))
                {
                    event.prevent_default();
                    controller.run(studio);
                }
            },
            super::document_bar::CurationDocumentBar { controller }
            div { class: "curation-panels",
                PanelWorkspace {
                    panels,
                    initial_layout: CurationWorkspaceLayout::initial(),
                    layout_scope: Some(CURATION_LAYOUT_SCOPE.to_owned()),
                    active_panel: controller.active_panel(),
                    on_panel_activate: move |id: PanelId| controller.activate_panel(id),
                }
            }
        }
    }
}

struct CurationWorkspaceLayout;

impl CurationWorkspaceLayout {
    /// The recipe is a tall column at the left; the sheet owns the rest.
    /// The selected episode's evidence docks inside the sheet, so an empty
    /// shelf never holds a third of the height at rest.
    fn initial() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "curation-columns",
            SplitAxis::Horizontal,
            0.26,
            LayoutNode::tile("curation-recipe", [RECIPE_PANEL_ID]),
            LayoutNode::tile("curation-sheet", [SHEET_PANEL_ID]),
        ))
    }
}

#[component]
pub(crate) fn CurationWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Curation,
        is_active,
    );
    let controller = CurationController::use_controller();
    controller.use_loading();
    let status = use_memo(move || controller.editor_status());
    rsx! {
        crate::shell::frame::WorkspaceFrame {
            workspace,
            status: crate::shell::frame::WorkspaceStatus::Curation(status),
            CurationPanels { controller }
        }
    }
}
