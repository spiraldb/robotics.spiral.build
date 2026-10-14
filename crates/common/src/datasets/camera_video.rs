//! One camera of the episodes a statement reads: a span of its recorded
//! packets assembled into a Spiral packet video, or its packets as they are.
use super::{DatasetSelection, TopicRevision};

/// A camera stream of one dataset, read as each episode's packet video.
///
/// `presentation_time` is what gives the assembled stream a media timeline:
/// each packet is timed from the episode's first message, so a frame's time is
/// the second the timeline shows for it.
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

    /// The packet video, as one row's `source`, of the episode bound as `$1`
    /// around the instant `at_ns` after its first message, which was logged
    /// at `start_ns`: its packets from the last keyframe logged at or before
    /// the instant, or its first packet, up to `span_ns` after the instant.
    /// A packet video starts at a keyframe, and starting at the one before
    /// the instant keeps a frame late in an episode as cheap as its first.
    pub fn video_at(self, start_ns: u64, (at_ns, span_ns): (u64, u64)) -> String {
        let instant = start_ns.saturating_add(at_ns);
        let until = instant.saturating_add(span_ns);
        let messages = self.dataset.messages(self.camera);
        format!(
            "SELECT spql.video.from_packets(spql.list.collect(STRUCT_PACK(\
             payload := payload, decode_index := decode_index, presentation_time := presentation_time)), \
             'h264', 'annex_b') AS source FROM \
             (SELECT r.value.data AS payload, CAST(r.ordinal AS BIGINT) AS decode_index, \
             spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch({start_ns}, 'ns') AS presentation_time \
             FROM {messages} r WHERE r.id = $1 AND r.log_time < {until} AND r.ordinal >= \
             COALESCE((SELECT MAX(k.ordinal) FROM {messages} k \
             WHERE k.id = $1 AND k.keyframe AND k.log_time <= {instant}), 0)) packets"
        )
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
    const RECORDED: &str = "(SELECT r.id, r.value.data AS payload, CAST(r.ordinal AS BIGINT) AS decode_index, CAST(r.log_time AS BIGINT) AS log_ns FROM \"datasets\".\"d\".\"messages\".\"c_00000000\" r WHERE r.id = $1) recorded CROSS JOIN (SELECT CAST(message_start_time AS BIGINT) AS start_ns FROM \"datasets\".\"d\".\"episodes\" WHERE id = $1) e";

    fn camera() -> (DatasetSelection, TopicRevision) {
        (
            DatasetSelection::new("d").unwrap(),
            TopicRevision::new("c_00000000").unwrap(),
        )
    }

    #[test]
    fn a_frame_late_in_an_episode_reads_from_the_keyframe_before_it() {
        let (dataset, stream) = camera();
        assert_eq!(
            CameraVideo::new(&dataset, &stream).video_at(100, (20, 5)),
            "SELECT spql.video.from_packets(spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source FROM (SELECT r.value.data AS payload, CAST(r.ordinal AS BIGINT) AS decode_index, spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(100, 'ns') AS presentation_time FROM \"datasets\".\"d\".\"messages\".\"c_00000000\" r WHERE r.id = $1 AND r.log_time < 125 AND r.ordinal >= COALESCE((SELECT MAX(k.ordinal) FROM \"datasets\".\"d\".\"messages\".\"c_00000000\" k WHERE k.id = $1 AND k.keyframe AND k.log_time <= 120), 0)) packets"
        );
    }

    #[test]
    fn training_reads_the_bound_episodes_packets_as_recorded() {
        let (dataset, stream) = camera();
        assert_eq!(
            CameraVideo::new(&dataset, &stream).recorded_packets(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM \"datasets\".\"d\".\"messages\".\"c_00000000\" r WHERE r.id = $1 ORDER BY r.ordinal"
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
