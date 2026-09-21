//! The Build canvas: a Dioxus Flow graph of dataset → measurement →
//! exploration nodes beside the selected node's workspace, plus the document
//! bar wiring that turns the Build view on. Owned by the Canvas stream.
//!
//! Follow `plan_display.rs` for `compute_layout`, fit-to-view, and popovers,
//! and `curation/pipeline.rs` for typed handles on branching nodes.
