//! The Curation canvas: a Dioxus Flow graph of the dataset and every node it
//! feeds, beside the selected node's workspace, plus the document bar.
//!
//! `flow_canvas` ranks the cards and parks the viewport; `node_card` is
//! which card a node draws and what its + offers, and the bar's Publish
//! turns chosen outputs into catalog views.
mod canvas;
mod canvas_controls;
mod dataset;
mod document_bar;
mod flow_canvas;
mod node_card;
mod node_workspace;
mod publish;

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

pub(super) use canvas::{AddGlyph, CurationGraphCanvas, NodeGlyph};
pub(super) use document_bar::CurationDocumentBar;
pub(super) use node_card::NodeFace;
pub(super) use node_workspace::NodeWorkspace;
pub(super) use publish::PublishedViews;
