//! One camera of the episodes a statement reads: its packets as they are.
use super::{DatasetSelection, TopicRevision};

/// A camera stream of one dataset, read as each episode's packets.
#[derive(Clone, Copy)]
pub struct CameraVideo<'a> {
    dataset: &'a DatasetSelection,
    camera: &'a TopicRevision,
}

impl<'a> CameraVideo<'a> {
    /// `camera` is one of `dataset`'s video streams.
    pub fn new(dataset: &'a DatasetSelection, camera: &'a TopicRevision) -> Self {
        Self { dataset, camera }
    }

    /// One statement listing this camera's packets of the episode bound as
    /// `$1`, in decode order, as the progress service reads them: `payload`,
    /// the access unit exactly as recorded; `decode_index`, from 0; and
    /// `pts_ns`, its time from the episode's first message.
    pub fn packets(self) -> String {
        format!(
            "SELECT payload, decode_index, pts_ns FROM ({}) packets ORDER BY decode_index",
            self.read("recorded.log_ns - e.start_ns AS pts_ns")
        )
    }

    /// One statement listing this camera's packets of the episode bound as
    /// `$1` exactly as recorded, in decode order, as training uploads them:
    /// `log_time_ns`, the absolute log time, and `payload`, the access unit.
    pub fn recorded_packets(self) -> String {
        format!(
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload \
             FROM {} r WHERE r.id = $1 ORDER BY r.ordinal",
            self.dataset.messages(self.camera)
        )
    }

    /// Every packet of the episode bound as `$1`, numbered in decode order and
    /// timed by `timed`, an expression of the packet's `recorded.log_ns` and
    /// the episode's `e.start_ns`. The camera and the episode's start are read
    /// apart, each filtered to the episode, so neither read joins the other's
    /// whole table.
    fn read(self, timed: &str) -> String {
        format!(
            "SELECT recorded.id, recorded.payload, recorded.decode_index, {timed} FROM \
             (SELECT r.id, r.value.data AS payload, CAST(r.ordinal AS BIGINT) AS decode_index, \
             CAST(r.log_time AS BIGINT) AS log_ns \
             FROM {} r WHERE r.id = $1) recorded \
             CROSS JOIN (SELECT CAST(message_start_time AS BIGINT) AS start_ns \
             FROM {} WHERE id = $1) e",
            self.dataset.messages(self.camera),
            self.dataset.episodes()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CameraVideo;
    use crate::datasets::{DatasetSelection, TopicRevision};

    /// The camera's packets of the bound episode, and that episode's start,
    /// each read filtered to the episode on its own.
    const RECORDED: &str = "(SELECT r.id, r.value.data AS payload, CAST(r.ordinal AS BIGINT) AS decode_index, CAST(r.log_time AS BIGINT) AS log_ns FROM \"datasets.d.messages.c_00000000\" r WHERE r.id = $1) recorded CROSS JOIN (SELECT CAST(message_start_time AS BIGINT) AS start_ns FROM \"datasets.d.episodes\" WHERE id = $1) e";

    fn camera() -> (DatasetSelection, TopicRevision) {
        (
            DatasetSelection::new("d").unwrap(),
            TopicRevision::new("c_00000000").unwrap(),
        )
    }

    #[test]
    fn training_reads_the_bound_episodes_packets_as_recorded() {
        let (dataset, stream) = camera();
        assert_eq!(
            CameraVideo::new(&dataset, &stream).recorded_packets(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM \"datasets.d.messages.c_00000000\" r WHERE r.id = $1 ORDER BY r.ordinal"
        );
    }

    #[test]
    fn the_progress_service_reads_the_bound_episodes_packets_in_decode_order() {
        let (dataset, stream) = camera();
        assert_eq!(
            CameraVideo::new(&dataset, &stream).packets(),
            format!(
                "SELECT payload, decode_index, pts_ns FROM (SELECT recorded.id, recorded.payload, recorded.decode_index, recorded.log_ns - e.start_ns AS pts_ns FROM {RECORDED}) packets ORDER BY decode_index"
            )
        );
    }
}
