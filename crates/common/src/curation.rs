//! Curation: rules that keep, exclude, and shape a dataset's episodes for
//! training, compiled to one read-only Spiral query. A curation is a
//! specialization of a workflow whose input and output are episodes.
pub mod compile;
pub mod facts;
pub mod rules;
pub mod run;

use crate::datasets::DatasetSelection;
use facts::{CurationFact, FactCatalog};
use rules::{CurationRule, CurationTransform};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CURATION_NAME: &str = "Untitled curation";
const MAX_FACTS: usize = 16;
const MAX_RULES: usize = 32;

/// The complete authored recipe. Runs execute the compiled SQL; Save persists
/// this document with optimistic revisions.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationSpec {
    id: String,
    name: String,
    revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<DatasetSelection>,
    #[serde(default)]
    facts: Vec<CurationFact>,
    #[serde(default)]
    rules: Vec<CurationRule>,
    #[serde(default)]
    transforms: Vec<CurationTransform>,
}

impl CurationSpec {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision: 0,
            source: None,
            facts: Vec::new(),
            rules: Vec::new(),
            transforms: Vec::new(),
        }
    }
    pub fn with_source(mut self, source: DatasetSelection) -> Self {
        self.source = Some(source);
        self
    }
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
    pub fn with_fact(mut self, fact: CurationFact) -> Self {
        self.facts.push(fact);
        self
    }
    pub fn with_rule(mut self, rule: CurationRule) -> Self {
        self.rules.push(rule);
        self
    }
    pub fn with_transform(mut self, transform: CurationTransform) -> Self {
        self.transforms.push(transform);
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub const fn source(&self) -> Option<&DatasetSelection> {
        self.source.as_ref()
    }
    pub fn facts(&self) -> &[CurationFact] {
        &self.facts
    }
    pub fn rules(&self) -> &[CurationRule] {
        &self.rules
    }
    pub fn transforms(&self) -> &[CurationTransform] {
        &self.transforms
    }
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
    pub fn set_source(&mut self, source: Option<DatasetSelection>) {
        self.source = source;
    }
    pub fn add_fact(&mut self, fact: CurationFact) {
        self.facts.push(fact);
    }
    pub fn replace_fact(&mut self, key: &str, fact: CurationFact) {
        if let Some(existing) = self.facts.iter_mut().find(|fact| fact.key() == key) {
            *existing = fact;
        }
    }
    pub fn remove_fact(&mut self, key: &str) {
        self.facts.retain(|fact| fact.key() != key);
    }
    pub fn add_rule(&mut self, rule: CurationRule) {
        self.rules.push(rule);
    }
    pub fn rule_mut(&mut self, id: &str) -> Option<&mut CurationRule> {
        self.rules.iter_mut().find(|rule| rule.id() == id)
    }
    pub fn remove_rule(&mut self, id: &str) {
        self.rules.retain(|rule| rule.id() != id);
    }
    pub fn add_transform(&mut self, transform: CurationTransform) {
        self.transforms.push(transform);
    }
    pub fn transform_mut(&mut self, id: &str) -> Option<&mut CurationTransform> {
        self.transforms
            .iter_mut()
            .find(|transform| transform.id() == id)
    }
    pub fn remove_transform(&mut self, id: &str) {
        self.transforms.retain(|transform| transform.id() != id);
    }
    /// Every fact a rule may name: the builtin episode statistics followed by
    /// the declared measurements, in authoring order.
    pub fn catalog(&self) -> FactCatalog {
        FactCatalog::for_spec(self)
    }
    /// A run's verdicts stay current while the judged definition is unchanged.
    /// Renaming the curation, a measurement, a rule's reason, or a transform
    /// does not invalidate them, and neither does saving; any edit that could
    /// change a verdict does.
    pub fn accepts_output_from(&self, executed: &Self) -> bool {
        fn same<T>(mine: &[T], theirs: &[T], alike: impl Fn(&T, &T) -> bool) -> bool {
            mine.len() == theirs.len() && mine.iter().zip(theirs).all(|(a, b)| alike(a, b))
        }
        self.id == executed.id
            && self.source == executed.source
            && same(&self.facts, &executed.facts, CurationFact::measures_like)
            && same(&self.rules, &executed.rules, CurationRule::judges_like)
            && same(
                &self.transforms,
                &executed.transforms,
                CurationTransform::judges_like,
            )
    }
    pub fn validate(&self) -> CurationValidationReport {
        let mut report = CurationValidationReport::default();
        if self.id.is_empty() || self.id.len() > 128 {
            report.push_error(
                "invalid_identity",
                "Curation identity must contain 1–128 characters.",
                "Create a new curation.",
            );
        }
        if self.name.trim().is_empty() {
            report.push_error(
                "empty_name",
                "The curation has no name.",
                "Name the curation before saving.",
            );
        }
        if self.source.is_none() {
            report.push_error(
                "missing_source",
                "No dataset is selected.",
                "Choose the dataset whose episodes this curation judges.",
            );
        }
        if self.facts.len() > MAX_FACTS {
            report.push_error(
                "too_many_facts",
                &format!("A curation measures at most {MAX_FACTS} facts."),
                "Remove a measurement.",
            );
        }
        if self.rules.len() > MAX_RULES {
            report.push_error(
                "too_many_rules",
                &format!("A curation holds at most {MAX_RULES} rules."),
                "Remove a rule.",
            );
        }
        let catalog = self.catalog();
        for (index, fact) in self.facts.iter().enumerate() {
            if let Err(message) = fact.validate() {
                report.push_error("invalid_fact", &message, "Fix the measurement.");
            }
            if self.facts[..index]
                .iter()
                .any(|other| other.key() == fact.key())
            {
                report.push_error(
                    "duplicate_fact",
                    &format!("Two measurements share the key “{}”.", fact.key()),
                    "Give each measurement its own key.",
                );
            }
            if facts::BuiltinFact::ALL
                .iter()
                .any(|builtin| builtin.key() == fact.key())
            {
                report.push_error(
                    "reserved_fact",
                    &format!("“{}” is already measured for every episode.", fact.key()),
                    "Choose another key.",
                );
            }
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if let Err(message) = rule.condition().validate(&catalog) {
                report.push_error(
                    "invalid_rule",
                    &format!("{} {message}.", Named("Rule", index, rule.label())),
                    "Fix the rule's condition.",
                );
            }
        }
        for (index, transform) in self.transforms.iter().enumerate() {
            if let Err(message) = transform.validate(&catalog) {
                report.push_error(
                    "invalid_transform",
                    &format!(
                        "{} {message}.",
                        Named("Transform", index, transform.label())
                    ),
                    "Fix the transform.",
                );
            }
        }
        report
    }
}

/// How a problem names a rule or transform: by its label when it has one,
/// else by its place in the recipe, so an unnamed rule is never “”.
struct Named<'a>(&'static str, usize, &'a str);

impl std::fmt::Display for Named<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(noun, index, label) = self;
        if label.trim().is_empty() {
            write!(formatter, "{noun} {}", index + 1)
        } else {
            write!(formatter, "{noun} “{label}”")
        }
    }
}

/// Validation reads as a list of problems, each with the way out.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurationValidationReport {
    #[serde(default)]
    errors: Vec<CurationIssue>,
}

impl CurationValidationReport {
    pub fn errors(&self) -> &[CurationIssue] {
        &self.errors
    }
    fn push_error(&mut self, code: &str, message: &str, recovery: &str) {
        self.errors
            .push(CurationIssue::new(code, message, recovery));
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurationIssue {
    code: String,
    message: String,
    recovery: String,
}

impl CurationIssue {
    fn new(code: &str, message: &str, recovery: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            recovery: recovery.to_owned(),
        }
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn recovery(&self) -> &str {
        &self.recovery
    }
}

/// One saved curation in the switcher.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationSummary {
    id: String,
    name: String,
}

impl CurationSummary {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests;
