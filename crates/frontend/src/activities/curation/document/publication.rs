//! Which outputs are published as catalog views, and whether each view is
//! still what the saved recipe compiles to.
//!
//! A view cannot be turned back into a graph, so the recipe stays in the
//! document and each publication records the view's name and the saved
//! revision it was compiled from. Saving an edit moves the saved revision past
//! it, which is how a node says its view is out of date. Recording a
//! publication is itself a save that edits no recipe, so every view current
//! before it stays current after it.
use super::{CurationGraph, CurationNode};
use crate::activities::curation::population::PUBLISHED_EPISODES;
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
    /// The most whole episodes the view reads, the first its dataset's
    /// Selects keep.
    episodes: usize,
}

impl PublishedOutput {
    /// The views a stored payload publishes, read without decoding its
    /// graph, so a curation saved in an encoding this Studio no longer opens
    /// is still checked before it is deleted. A payload naming none, or not
    /// JSON at all, publishes nothing.
    pub(crate) fn views_in(payload: &str) -> Vec<String> {
        #[derive(Deserialize)]
        struct Publications {
            #[serde(default)]
            published: Vec<PublishedOutput>,
        }
        serde_json::from_str::<Publications>(payload)
            .map(|publications| {
                publications
                    .published
                    .into_iter()
                    .map(|published| published.view)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub(crate) fn node(&self) -> &str {
        &self.node
    }
    pub(crate) fn view(&self) -> &str {
        &self.view
    }
    #[cfg(test)]
    pub(crate) fn episodes(&self) -> usize {
        self.episodes
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

    /// Every output published as a view, beside its node.
    pub(crate) fn publications(&self) -> impl Iterator<Item = (&PublishedOutput, &CurationNode)> {
        self.published
            .iter()
            .filter_map(|published| Some((published, self.node(&published.node)?)))
    }

    /// Record that `node` is published as `view`, compiled from the recipe
    /// saved at `saved` and bounded to [`PUBLISHED_EPISODES`] episodes. The
    /// save that records it comes next, at `saved + 1`. A refused record
    /// changes nothing.
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
            episodes: PUBLISHED_EPISODES,
        });
        Ok(())
    }

    /// Whether `other` differs from this graph by what it publishes alone.
    pub(crate) fn same_recipe(&self, other: &Self) -> bool {
        self.dataset == other.dataset && self.nodes == other.nodes && self.edges == other.edges
    }

    /// Publish what `other`, a copy of this curation stored elsewhere,
    /// publishes.
    pub(crate) fn take_publications(&mut self, other: &Self) {
        self.published = other.published.clone();
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
                "“{view}” names a node that is not an output; only Score progress, Find similar, Group similar, and Select publish."
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
