//! A curation preview run: every query one node's preview takes over the
//! sample, handed to the backend in one request and polled until answered.
//!
//! A preview query can wait minutes on a model, which the browser cannot hold
//! a connection open for without starving every other request of the page.
//! So the backend runs the queries and the browser only asks how far they
//! have got, fetching each answer once it lands.
use crate::relations::{EpisodeQueryRequest, QueryRequest};
use serde::{Deserialize, Serialize};

/// One query of a preview run, and the form its answer comes back in.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "request", rename_all = "snake_case")]
pub enum PreviewQuery {
    /// One episode's statement, `$1` bound to it, answered as rows.
    EpisodeRows(EpisodeQueryRequest),
    /// One episode's statement answered as one Arrow IPC stream, for answers
    /// longer than rows are served, such as every scored frame of a recording.
    EpisodeArrow(EpisodeQueryRequest),
    /// A statement over many episodes, answered as rows.
    Rows(QueryRequest),
}

/// Where one query of a run stands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum PreviewQueryState {
    Queued,
    Running,
    Answered,
    Failed(String),
}

impl PreviewQueryState {
    /// Whether a run whose queries stand at `queries` will answer nothing
    /// more: none is running, and none is left to start, or the run stopped
    /// before starting them.
    pub fn settled(queries: &[Self], stopped: bool) -> bool {
        !queries.contains(&Self::Running) && (stopped || !queries.contains(&Self::Queued))
    }
}

/// Where one preview run stands, as each poll reads it: its queries in the
/// order they were submitted, and whether it was stopped.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRunStatus {
    id: String,
    queries: Vec<PreviewQueryState>,
    stopped: bool,
}

impl PreviewRunStatus {
    pub fn new(id: String, queries: Vec<PreviewQueryState>, stopped: bool) -> Self {
        Self {
            id,
            queries,
            stopped,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn queries(&self) -> &[PreviewQueryState] {
        &self.queries
    }
    pub fn is_settled(&self) -> bool {
        PreviewQueryState::settled(&self.queries, self.stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_settles_once_nothing_runs_and_nothing_is_left_to_start() {
        use PreviewQueryState::{Answered, Failed, Queued, Running};
        let status = |queries: Vec<PreviewQueryState>, stopped| {
            PreviewRunStatus::new("preview-1".into(), queries, stopped).is_settled()
        };
        assert!(!status(vec![Answered, Running], false));
        assert!(!status(vec![Answered, Queued], false));
        assert!(status(vec![Answered, Failed("no camera".into())], false));
        // Stopping starts nothing new, but what is running still lands.
        assert!(!status(vec![Running, Queued], true));
        assert!(status(vec![Answered, Queued], true));
    }

    #[test]
    fn a_failed_query_says_why_on_the_wire() {
        assert_eq!(
            serde_json::to_value(PreviewQueryState::Failed("no camera".into())).unwrap(),
            serde_json::json!({"state": "failed", "reason": "no camera"})
        );
        assert_eq!(
            serde_json::to_value(PreviewQueryState::Queued).unwrap(),
            serde_json::json!({"state": "queued"})
        );
    }
}
