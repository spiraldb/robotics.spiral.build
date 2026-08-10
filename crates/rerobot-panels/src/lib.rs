//! A small, application-agnostic panel system for Dioxus workspaces.
//!
//! Applications register content as [`Panel`] values and describe only their
//! preferred starting arrangement. [`PanelWorkspace`] owns the durable layout,
//! tab activation, tiling, split resizing, drag previews, keyboard equivalents,
//! persistence, and the common panel chrome.

mod model;
mod workspace;

pub use model::{
    DockZone, LayoutNode, PanelId, PanelLayout, PanelPlacement, SplitAxis, SplitId, Tile, TileId,
};
pub use workspace::{Panel, PanelWorkspace};
