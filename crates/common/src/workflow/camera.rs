//! Camera columns discovered from an MCAP dataset.
use crate::datasets::TopicRevision;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraBinding {
    topic: String,
    revision: TopicRevision,
    field: Vec<String>,
}
impl CameraBinding {
    pub fn new(topic: String, revision: TopicRevision) -> Self {
        Self {
            topic,
            revision,
            field: vec!["value".into(), "data".into()],
        }
    }
}
