//! The Curation canvas: a Dioxus Flow graph of the dataset and every node it
//! feeds, beside the selected node's workspace, plus the document bar.
//!
//! Ranking the cards and parking the viewport are `components::flow_canvas`;
//! what is Curation's own is which card a node draws and what its + offers,
//! and the bar's Publish, which turns chosen outputs into catalog views.
mod canvas;
mod dataset;
mod document_bar;
mod node_card;
mod node_workspace;
mod publish;

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

pub(super) use canvas::{CurationGraphCanvas, NodeGlyph};
pub(super) use document_bar::CurationDocumentBar;
pub(super) use node_card::NodeFace;
pub(super) use node_workspace::NodeWorkspace;
