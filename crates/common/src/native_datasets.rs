//! Native Spiral Dataset discovery and bounded topic inspection.
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeDatasetSelection {
    path: Vec<String>,
}
impl NativeDatasetSelection {
    pub fn new(path: Vec<String>) -> Result<Self, String> {
        if path.is_empty()
            || path.len() > 32
            || path.iter().any(|part| {
                part.is_empty() || part.len() > 256 || part.chars().any(char::is_control)
            })
        {
            return Err("Dataset path must contain 1–32 nonempty names".to_owned());
        }
        Ok(Self { path })
    }
    pub fn path(&self) -> &[String] {
        &self.path
    }
    pub fn label(&self) -> String {
        self.path.join(".")
    }
    pub fn sql_name(&self) -> String {
        self.label()
    }
    pub fn can_preview(&self) -> bool {
        self.path.iter().all(|part| !part.contains('.'))
    }
    pub fn episodes_sql(&self) -> String {
        format!(
            "SELECT * FROM spql.mcap.episodes('{}') LIMIT 100",
            self.sql_name().replace('\'', "''")
        )
    }
    pub fn topic_sql(&self, topic: &str) -> String {
        format!(
            "SELECT * FROM spql.mcap.read_topic('{}', '{}') LIMIT 100",
            self.sql_name().replace('\'', "''"),
            topic.replace('\'', "''")
        )
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeDatasetSummary {
    selection: NativeDatasetSelection,
    resource_id: String,
    version_id: String,
}
impl NativeDatasetSummary {
    pub fn new(selection: NativeDatasetSelection, identity: (String, String)) -> Self {
        Self {
            selection,
            resource_id: identity.0,
            version_id: identity.1,
        }
    }
    pub fn selection(&self) -> &NativeDatasetSelection {
        &self.selection
    }
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }
    pub fn version_id(&self) -> &str {
        &self.version_id
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeDatasetTopic {
    name: String,
    schemas: Vec<String>,
    members: u64,
    samples: Option<u64>,
}
impl NativeDatasetTopic {
    pub fn new(name: String, schemas: Vec<String>) -> Self {
        Self {
            name,
            schemas,
            members: 0,
            samples: None,
        }
    }
    pub fn with_coverage(mut self, members: u64, samples: Option<u64>) -> Self {
        self.members = members;
        self.samples = samples;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn schemas(&self) -> &[String] {
        &self.schemas
    }
    pub fn members(&self) -> u64 {
        self.members
    }
    pub fn samples(&self) -> Option<u64> {
        self.samples
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeDatasetInspection {
    dataset: NativeDatasetSummary,
    members: Option<u64>,
    topics: Vec<NativeDatasetTopic>,
}
impl NativeDatasetInspection {
    pub fn new(
        dataset: NativeDatasetSummary,
        members: Option<u64>,
        topics: Vec<NativeDatasetTopic>,
    ) -> Self {
        Self {
            dataset,
            members,
            topics,
        }
    }
    pub fn dataset(&self) -> &NativeDatasetSummary {
        &self.dataset
    }
    pub fn members(&self) -> Option<u64> {
        self.members
    }
    pub fn topics(&self) -> &[NativeDatasetTopic] {
        &self.topics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dataset_paths_and_topics_cannot_escape_preview_sql() {
        let selection =
            NativeDatasetSelection::new(vec!["a.b".into(), "owner's \"data\"".into()]).unwrap();
        assert!(!selection.can_preview());
        assert!(
            selection
                .topic_sql("/camera' --")
                .contains("'/camera'' --'")
        );
        assert!(selection.episodes_sql().contains("owner''s"));
        assert!(NativeDatasetSelection::new(vec![]).is_err());
        assert!(NativeDatasetSelection::new(vec!["\n".into()]).is_err());
    }
}

/// A real resource at its catalog path; parentage comes from Spiral's namespace.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectResource {
    selection: NativeDatasetSelection,
    kind: String,
    identity: (String, String),
}
impl ProjectResource {
    pub fn new(
        selection: NativeDatasetSelection,
        kind: String,
        identity: (String, String),
    ) -> Self {
        Self {
            selection,
            kind,
            identity,
        }
    }
    pub fn selection(&self) -> &NativeDatasetSelection {
        &self.selection
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn resource_id(&self) -> &str {
        &self.identity.0
    }
    pub fn dataset(&self) -> Option<NativeDatasetSummary> {
        (self.kind == "dataset")
            .then(|| NativeDatasetSummary::new(self.selection.clone(), self.identity.clone()))
    }
    pub fn table_sql(&self) -> String {
        let name = self
            .selection
            .path()
            .iter()
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".");
        format!("SELECT * FROM {name} LIMIT 100")
    }
}
