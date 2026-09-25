//! What the preview has read this session: for each compiled measurement,
//! each episode it ran on and the moments that came back, or why that episode
//! failed. Nothing here is saved; a reload reads again.
use super::document::CurationGraph;
use super::moments::Moment;
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;

/// One episode's preview read.
#[derive(Clone, Debug, PartialEq)]
enum EpisodeRead {
    Moments(Vec<Moment>),
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PreviewReads(BTreeMap<String, BTreeMap<EpisodeId, EpisodeRead>>);

impl PreviewReads {
    /// Hold what `node`'s preview of `episode` answered, replacing any earlier
    /// read of it.
    pub(super) fn record(
        &mut self,
        node: &str,
        episode: &EpisodeId,
        read: Result<Vec<Moment>, String>,
    ) {
        let read = match read {
            Ok(moments) => EpisodeRead::Moments(moments),
            Err(reason) => EpisodeRead::Failed(reason),
        };
        self.0
            .entry(node.to_owned())
            .or_default()
            .insert(episode.clone(), read);
    }
    fn read(&self, node: &str, episode: &EpisodeId) -> Option<&EpisodeRead> {
        self.0.get(node)?.get(episode)
    }
    /// The moments `node` read for `episode`; none until it has.
    pub(super) fn moments(&self, node: &str, episode: &EpisodeId) -> &[Moment] {
        match self.read(node, episode) {
            Some(EpisodeRead::Moments(moments)) => moments,
            Some(EpisodeRead::Failed(_)) | None => &[],
        }
    }
    /// Whether `node` has read `episode` without failing.
    pub(super) fn has_read(&self, node: &str, episode: &EpisodeId) -> bool {
        matches!(self.read(node, episode), Some(EpisodeRead::Moments(_)))
    }
    /// Why `node`'s last read of `episode` failed, when it did.
    pub(super) fn failure(&self, node: &str, episode: &EpisodeId) -> Option<&str> {
        match self.read(node, episode) {
            Some(EpisodeRead::Failed(reason)) => Some(reason),
            Some(EpisodeRead::Moments(_)) | None => None,
        }
    }
    /// Let go of what nodes the graph no longer holds read.
    pub(super) fn forget_removed(&mut self, graph: &CurationGraph) {
        self.0.retain(|node, _| graph.node(node).is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_read_replaces_an_earlier_one_and_a_failure_holds_no_moments() {
        let episode = EpisodeId::fixture(1);
        let moment = Moment::new(episode.clone(), (0.0, 0.5), Some(0.3)).unwrap();
        let mut reads = PreviewReads::default();
        reads.record("detect", &episode, Err("timed out".into()));
        assert_eq!(reads.failure("detect", &episode), Some("timed out"));
        assert!(!reads.has_read("detect", &episode));
        assert!(reads.moments("detect", &episode).is_empty());
        reads.record("detect", &episode, Ok(vec![moment.clone()]));
        assert_eq!(reads.moments("detect", &episode), [moment]);
        assert_eq!(reads.failure("detect", &episode), None);
        reads.forget_removed(&CurationGraph::new(None));
        assert!(!reads.has_read("detect", &episode));
    }
}
