//! Which nodes are published as catalog views, and whether each view is
//! still what its node compiles to.
//!
//! A view cannot be turned back into a graph, so the recipe stays in the
//! document and each publication records the view's name and the digest of
//! the statement it was created with. A view is out of date once its node
//! compiles to another statement: an edit on its path, from the dataset down
//! to the node, and only such an edit.
use super::{CurationGraph, CurationNode};
use crate::activities::curation::path::CurationPath;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::curation::{PublishedView, ViewName};
use std::collections::BTreeSet;

/// One node published as a catalog view, over every episode of the
/// dataset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishedOutput {
    node: String,
    view: String,
    /// The statement the view was created with.
    statement: StatementDigest,
}

/// The SHA-256 of a view's statement, which tells whether a node still
/// compiles to its view without keeping the statement in the document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
struct StatementDigest(String);

impl StatementDigest {
    fn of(statement: &str) -> Self {
        Self(format!("{:x}", Sha256::digest(statement.as_bytes())))
    }
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
    /// Whether the view is still what `graph` compiles its node to.
    pub(crate) fn is_current(&self, graph: &CurationGraph) -> bool {
        CurationPath::to(graph, &self.node)
            .and_then(|path| path.statement())
            .is_ok_and(|statement| StatementDigest::of(&statement) == self.statement)
    }
}

/// The view one node is published as, judged against the graph it is in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NodePublication {
    view: String,
    current: bool,
}

impl NodePublication {
    pub(crate) fn view(&self) -> &str {
        &self.view
    }
    /// Whether the node still compiles to its view.
    pub(crate) fn is_current(&self) -> bool {
        self.current
    }
}

impl CurationGraph {
    /// The view `node` is published as, if it is.
    pub(super) fn publication(&self, node: &str) -> Option<&PublishedOutput> {
        self.published
            .iter()
            .find(|published| published.node == node)
    }

    /// The view `node` is published as, if it is, and whether `node` still
    /// compiles to it.
    pub(crate) fn node_publication(&self, node: &str) -> Option<NodePublication> {
        self.publication(node).map(|published| NodePublication {
            view: published.view.clone(),
            current: published.is_current(self),
        })
    }

    /// Every output published as a view, beside its node.
    pub(crate) fn publications(&self) -> impl Iterator<Item = (&PublishedOutput, &CurationNode)> {
        self.published
            .iter()
            .filter_map(|published| Some((published, self.node(&published.node)?)))
    }

    /// Record that `node` is published as `view`. A refused record changes
    /// nothing.
    pub(crate) fn record_publication(
        &mut self,
        node: &str,
        view: &PublishedView,
    ) -> Result<(), String> {
        self.check_publication(node, view.name())?;
        if let Some(other) = self
            .published
            .iter()
            .find(|published| published.view == view.name() && published.node != node)
        {
            return Err(format!(
                "Another output is already published as “{}”.",
                other.view
            ));
        }
        self.published.retain(|published| published.node != node);
        self.published.push(PublishedOutput {
            node: node.to_owned(),
            view: view.name().to_owned(),
            statement: StatementDigest::of(view.statement()),
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
    pub(crate) fn remove_publication(&mut self, node: &str) -> Result<(), String> {
        if self.publication(node).is_none() {
            return Err("That output is not published.".into());
        }
        self.published.retain(|published| published.node != node);
        Ok(())
    }

    /// Whether `node` may be published as `view`: it is a node but the
    /// dataset, and the name is a view name.
    fn check_publication(&self, node: &str, view: &str) -> Result<(), String> {
        ViewName::try_from(view)?;
        if !self.outputs().iter().any(|output| output.id() == node) {
            return Err(format!(
                "“{view}” names no node that publishes; every node but the dataset does."
            ));
        }
        Ok(())
    }

    /// Every publication names one node by a valid view name, and no two
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
