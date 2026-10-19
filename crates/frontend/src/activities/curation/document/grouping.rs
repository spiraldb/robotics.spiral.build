//! A Group similar subtasks node's categories: what it puts every subtask
//! in, each with a stable id that names its column, so renaming a category
//! leaves every Select choosing it intact.
//!
//! A model proposes the categories in the browser and the operator may
//! rename or merge them; either way they are the node's configuration, and
//! its call names them. A proposal numbers its categories afresh and points
//! every Select choosing among them at all of them; a renamed category keeps
//! its id; and a merged one's Selects choose the category it joined.
use super::{CurationGraph, CurationNode, CurationNodeKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

/// How many categories one grouping may name: what the decision model's
/// Choice takes.
const MAX_CATEGORIES: usize = 255;

/// Which category of its grouping a column or a Select means.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub(crate) struct CategoryId(u32);

impl CategoryId {
    /// Before a grouping takes a category, it has no id of its own.
    const UNNUMBERED: Self = Self(0);
}

impl Display for CategoryId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One category: its id within its grouping, a short name, and the
/// definition the classifier reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskCategory {
    id: CategoryId,
    name: String,
    definition: String,
}
impl SubtaskCategory {
    /// A category in its own words, numbered when a grouping takes it.
    pub(crate) fn new(
        name: impl Into<String>,
        definition: impl Into<String>,
    ) -> Result<Self, String> {
        let category = Self {
            id: CategoryId::UNNUMBERED,
            name: name.into().trim().to_owned(),
            definition: definition.into().trim().to_owned(),
        };
        if category.name.is_empty() || category.definition.is_empty() {
            return Err("A category needs a name and a definition.".into());
        }
        Ok(category)
    }
    pub(crate) fn id(&self) -> CategoryId {
        self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn definition(&self) -> &str {
        &self.definition
    }
    fn numbered(self, id: CategoryId) -> Self {
        Self { id, ..self }
    }
}

/// The categories one Group similar subtasks node puts every subtask of the
/// Break down above it in.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskGrouping {
    #[serde(default)]
    categories: Vec<SubtaskCategory>,
}
impl SubtaskGrouping {
    pub(crate) fn categories(&self) -> &[SubtaskCategory] {
        &self.categories
    }
    pub(crate) fn category(&self, id: CategoryId) -> Option<&SubtaskCategory> {
        self.categories.iter().find(|category| category.id == id)
    }
    /// A proposed list, numbered from one in its order.
    fn proposed(categories: Vec<SubtaskCategory>) -> Result<Self, String> {
        let grouping = Self {
            categories: (1..)
                .zip(categories)
                .map(|(id, category)| category.numbered(CategoryId(id)))
                .collect(),
        };
        grouping.validate()?;
        Ok(grouping)
    }
    /// The grouping with the category called `original` replaced by
    /// `edited`, under the same id, refusing a name another category holds.
    fn renamed(&self, original: &str, edited: SubtaskCategory) -> Result<Self, String> {
        if self
            .categories
            .iter()
            .any(|category| category.name() != original && category.name() == edited.name())
        {
            return Err(format!("A category is already called “{}”.", edited.name()));
        }
        let mut categories = self.categories.clone();
        let place = categories
            .iter()
            .position(|category| category.name() == original)
            .ok_or("That category is no longer part of this grouping.")?;
        categories[place] = edited.numbered(categories[place].id);
        Ok(Self { categories })
    }
    /// The grouping with `merged` gone into `into`, which keeps its name and
    /// id and gains the other definition: the union of two groups is not
    /// described by either sentence alone, and the operator can trim it
    /// afterwards. Also the ids of the two.
    fn merged(&self, merged: &str, into: &str) -> Result<(Self, CategoryId, CategoryId), String> {
        if merged == into {
            return Err("Merge a category into a different one.".into());
        }
        let gone = "That category is no longer part of this grouping.";
        let named = |name: &str| {
            self.categories
                .iter()
                .find(|category| category.name() == name)
                .ok_or(gone)
        };
        let (absorbed, joined) = (named(merged)?, named(into)?);
        let categories = self
            .categories
            .iter()
            .filter(|category| category.name() != merged)
            .map(|category| {
                if category.name() == into {
                    SubtaskCategory::new(
                        category.name(),
                        format!("{} {}", category.definition(), absorbed.definition()),
                    )
                    .map(|category| category.numbered(joined.id))
                } else {
                    Ok(category.clone())
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((Self { categories }, absorbed.id, joined.id))
    }
    /// No categories yet, or up to 255 distinct names, each with its
    /// definition and an id no other holds.
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.categories.len() > MAX_CATEGORIES {
            return Err(format!(
                "A grouping names at most {MAX_CATEGORIES} categories."
            ));
        }
        let (mut names, mut ids) = (BTreeSet::new(), BTreeSet::new());
        for category in &self.categories {
            if category.name.trim().is_empty() || category.definition.trim().is_empty() {
                return Err("A category needs a name and a definition.".into());
            }
            if !names.insert(category.name.as_str()) {
                return Err(format!("Category “{}” appears twice.", category.name));
            }
            if category.id == CategoryId::UNNUMBERED || !ids.insert(category.id) {
                return Err(format!(
                    "Category “{}” has no id of its own.",
                    category.name
                ));
            }
        }
        Ok(())
    }
}

impl CurationGraph {
    /// Group `node` by a list of categories proposed afresh. Every Select
    /// choosing among its categories chooses all of the new ones.
    pub(crate) fn propose_categories(
        &mut self,
        node: &str,
        categories: Vec<SubtaskCategory>,
    ) -> Result<(), String> {
        let grouping = SubtaskGrouping::proposed(categories)?;
        let every = grouping
            .categories()
            .iter()
            .map(SubtaskCategory::id)
            .collect::<Vec<_>>();
        self.regroup(node, grouping, |_| Some(every.clone()))
    }
    /// Rename or redefine the category called `original`; it keeps its id,
    /// so whatever chose it still does.
    pub(crate) fn edit_category(
        &mut self,
        node: &str,
        original: &str,
        edited: SubtaskCategory,
    ) -> Result<(), String> {
        let grouping = self.grouping(node)?.renamed(original, edited)?;
        self.regroup(node, grouping, |_| None)
    }
    /// Fold the category called `merged` into `into`: its subtasks join
    /// `into`, and a Select that chose it chooses `into`.
    pub(crate) fn merge_category(
        &mut self,
        node: &str,
        merged: &str,
        into: &str,
    ) -> Result<(), String> {
        let (grouping, gone, joined) = self.grouping(node)?.merged(merged, into)?;
        self.regroup(node, grouping, |chosen| {
            chosen.contains(&gone).then(|| {
                let mut kept = chosen
                    .iter()
                    .copied()
                    .filter(|id| *id != gone)
                    .collect::<Vec<_>>();
                if !kept.contains(&joined) {
                    kept.push(joined);
                }
                kept
            })
        })
    }
    fn grouping(&self, node: &str) -> Result<&SubtaskGrouping, String> {
        match self.node(node).map(CurationNode::kind) {
            Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => Ok(grouping),
            _ => Err("This node no longer groups subtasks.".into()),
        }
    }
    /// Give `node` its `grouping`, and every Select's choice among its
    /// categories what `rechoose` makes of it, when it makes anything: so
    /// the graph never names a category its grouping does not have.
    fn regroup(
        &mut self,
        node: &str,
        grouping: SubtaskGrouping,
        rechoose: impl Fn(&[CategoryId]) -> Option<Vec<CategoryId>>,
    ) -> Result<(), String> {
        let mut candidate = self.clone();
        match candidate.node_mut(node).map(CurationNode::kind_mut) {
            Some(CurationNodeKind::GroupSimilarSubtasks(current)) => *current = grouping,
            _ => return Err("This node no longer groups subtasks.".into()),
        }
        for other in &mut candidate.nodes {
            if let CurationNodeKind::Select(selection) = other.kind_mut() {
                selection.rechoose_categories(node, &rechoose);
            }
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
impl SubtaskGrouping {
    /// A grouping already holding `categories`, numbered in their order.
    pub(crate) fn of(categories: Vec<SubtaskCategory>) -> Self {
        Self::proposed(categories).expect("categories the classifier takes")
    }
}
