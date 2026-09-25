//! Which outputs are published as catalog views, and whether each view is
//! still what the saved recipe compiles to.
//!
//! A view cannot be turned back into a graph, so the recipe stays in the
//! document and each publication records the view's name and the saved
//! revision it was compiled from. Saving an edit moves the saved revision past
//! it, which is how a node says its view is out of date. Recording a
//! publication is itself a save that edits no recipe, so every view current
//! before it stays current after it.
use super::CurationGraph;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::curation::ViewName;
use std::collections::BTreeSet;

/// One output published as a catalog view.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishedOutput {
    node: String,
    view: String,
    /// The saved revision whose recipe the view was compiled from.
    revision: u64,
}

impl PublishedOutput {
    pub(crate) fn node(&self) -> &str {
        &self.node
    }
    pub(crate) fn view(&self) -> &str {
        &self.view
    }
    /// Whether the view is still what the recipe saved at `saved` compiles to.
    pub(crate) fn is_current(&self, saved: u64) -> bool {
        self.revision >= saved
    }
}

impl CurationGraph {
    /// The view `node` is published as, if it is.
    pub(crate) fn publication(&self, node: &str) -> Option<&PublishedOutput> {
        self.published
            .iter()
            .find(|published| published.node == node)
    }

    /// Record that `node` is published as `view`, compiled from the recipe
    /// saved at `saved`. The save that records it comes next, at `saved + 1`.
    /// A refused record changes nothing.
    pub(crate) fn record_publication(
        &mut self,
        node: &str,
        view: &str,
        saved: u64,
    ) -> Result<(), String> {
        self.check_publication(node, view)?;
        if let Some(other) = self
            .published
            .iter()
            .find(|published| published.view == view && published.node != node)
        {
            return Err(format!(
                "Another output is already published as “{}”.",
                other.view
            ));
        }
        self.advance_current(saved);
        self.published.retain(|published| published.node != node);
        self.published.push(PublishedOutput {
            node: node.to_owned(),
            view: view.to_owned(),
            revision: saved + 1,
        });
        Ok(())
    }

    /// Forget that `node` is published, once its view is dropped.
    pub(crate) fn remove_publication(&mut self, node: &str, saved: u64) -> Result<(), String> {
        if self.publication(node).is_none() {
            return Err("That output is not published.".into());
        }
        self.advance_current(saved);
        self.published.retain(|published| published.node != node);
        Ok(())
    }

    /// Carry every view current at `saved` to the next save, which records a
    /// publication and edits no recipe.
    fn advance_current(&mut self, saved: u64) {
        for published in &mut self.published {
            if published.is_current(saved) {
                published.revision = saved + 1;
            }
        }
    }

    /// Whether `node` may be published as `view`: it is an output, and the
    /// name is a view name.
    fn check_publication(&self, node: &str, view: &str) -> Result<(), String> {
        ViewName::try_from(view)?;
        if !self.outputs().iter().any(|output| output.id() == node) {
            return Err(format!(
                "“{view}” is published from a Select that must stay an output; unpublish it before adding a step after it."
            ));
        }
        Ok(())
    }

    /// Every publication names one output by a valid view name, and no two
    /// share a view.
    pub(super) fn validate_publications(&self) -> Result<(), String> {
        let mut views = BTreeSet::new();
        for published in &self.published {
            self.check_publication(&published.node, &published.view)?;
            if !views.insert(published.view.as_str()) {
                return Err(format!(
                    "Two outputs are published as “{}”.",
                    published.view
                ));
            }
        }
        Ok(())
    }
}
