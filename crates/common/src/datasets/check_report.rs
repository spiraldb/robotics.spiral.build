//! A dataset's check report, kept once per dataset version and per version
//! of the checks. The browser composes the checks and reads the report; the
//! document is the browser's own, so the backend only keeps it.
use super::DatasetSelection;
use serde::{Deserialize, Serialize};

/// Which report to read: the one for the dataset's current version, computed
/// by checks at `definition`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetCheckLookup {
    dataset: DatasetSelection,
    definition: u32,
}
impl DatasetCheckLookup {
    pub fn new(dataset: DatasetSelection, definition: u32) -> Self {
        Self {
            dataset,
            definition,
        }
    }
    pub fn into_parts(self) -> (DatasetSelection, u32) {
        (self.dataset, self.definition)
    }
}

/// A report the browser computed with checks at `definition`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetCheckSave {
    lookup: DatasetCheckLookup,
    report: serde_json::Value,
}
impl DatasetCheckSave {
    pub fn new(lookup: DatasetCheckLookup, report: serde_json::Value) -> Self {
        Self { lookup, report }
    }
    pub fn into_parts(self) -> (DatasetCheckLookup, serde_json::Value) {
        (self.lookup, self.report)
    }
}
