//! One camera of the episodes a statement reads: each assembled out of its
//! recorded packets into a Spiral packet video, or its packets as they are.
use crate::sql::SqlLiteral;

/// A camera topic of one dataset, read as each episode's packet video.
///
/// `presentation_time` is what gives the assembled stream a media timeline:
/// each packet is timed from the episode's first message, so a frame's time is
/// the second the timeline shows for it.
#[derive(Clone, Copy)]
pub struct CameraVideo<'a> {
    dataset: &'a str,
    camera: &'a str,
}

impl<'a> CameraVideo<'a> {
    /// `dataset` is the dataset's SQL name and `camera` one of its video topics.
    pub fn new(dataset: &'a str, camera: &'a str) -> Self {
        Self { dataset, camera }
    }

    /// Two `WITH` entries: `packets` reads this camera's packets of the
    /// episode bound as `$1`, and `videos` assembles them into one packet
    /// video, `(id, source)`.
    pub fn videos(self, (packets, videos): (&str, &str)) -> String {
        let timed = "spql.dt.from_epoch(recorded.log_ns, 'ns') - spql.dt.from_epoch(e.start_ns, 'ns') AS presentation_time";
        format!(
            "{packets} AS ({}), \
             {videos} AS (SELECT id, spql.video.from_packets(\
             spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source \
             FROM {packets} GROUP BY id)",
            self.read(timed)
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
    /// `log_time_ns`, the absolute MCAP log time, and `payload`, the access
    /// unit.
    pub fn recorded_packets(self) -> String {
        format!(
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload \
             FROM spql.mcap.read_topic({}, {}) r WHERE r.id = $1 \
             ORDER BY r.chunk_start_offset, r.sample_ordinal",
            SqlLiteral::text(self.dataset),
            SqlLiteral::text(self.camera)
        )
    }

    /// Every packet of the episode bound as `$1`, numbered in decode order and
    /// timed by `timed`, an expression of the packet's `recorded.log_ns` and
    /// the episode's `e.start_ns`. The camera and the episode's start are read
    /// apart, each filtered to the episode: joined on the episode instead,
    /// Spiral read the camera of every episode in the dataset for each one.
    fn read(self, timed: &str) -> String {
        let dataset = SqlLiteral::text(self.dataset);
        format!(
            "SELECT recorded.id, recorded.payload, recorded.decode_index, {timed} FROM \
             (SELECT r.id, r.value.data AS payload, \
             CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, \
             CAST(r.log_time AS BIGINT) AS log_ns \
             FROM spql.mcap.read_topic({dataset}, {}) r WHERE r.id = $1) recorded \
             CROSS JOIN (SELECT CAST(message_start_time AS BIGINT) AS start_ns \
             FROM spql.mcap.episodes({dataset}) WHERE id = $1) e",
            SqlLiteral::text(self.camera)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CameraVideo;

    /// The camera's packets of the bound episode, and that episode's start,
    /// each read filtered to the episode on its own.
    const RECORDED: &str = "(SELECT r.id, r.value.data AS payload, CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, CAST(r.log_time AS BIGINT) AS log_ns FROM spql.mcap.read_topic('d', '/c') r WHERE r.id = $1) recorded CROSS JOIN (SELECT CAST(message_start_time AS BIGINT) AS start_ns FROM spql.mcap.episodes('d') WHERE id = $1) e";

    #[test]
    fn the_bound_episodes_packets_assemble_into_its_video() {
        assert_eq!(
            CameraVideo::new("d", "/c").videos(("packets", "streams")),
            format!(
                "packets AS (SELECT recorded.id, recorded.payload, recorded.decode_index, spql.dt.from_epoch(recorded.log_ns, 'ns') - spql.dt.from_epoch(e.start_ns, 'ns') AS presentation_time FROM {RECORDED}), streams AS (SELECT id, spql.video.from_packets(spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source FROM packets GROUP BY id)"
            )
        );
    }

    #[test]
    fn training_reads_the_bound_episodes_packets_as_recorded() {
        assert_eq!(
            CameraVideo::new("d", "/c").recorded_packets(),
            "SELECT CAST(r.log_time AS BIGINT) AS log_time_ns, r.value.data AS payload FROM spql.mcap.read_topic('d', '/c') r WHERE r.id = $1 ORDER BY r.chunk_start_offset, r.sample_ordinal"
        );
    }

    #[test]
    fn the_progress_service_reads_the_bound_episodes_packets_in_decode_order() {
        assert_eq!(
            CameraVideo::new("d", "/c").packets(),
            format!(
                "SELECT payload, decode_index, pts_ns FROM (SELECT recorded.id, recorded.payload, recorded.decode_index, recorded.log_ns - e.start_ns AS pts_ns FROM {RECORDED}) packets ORDER BY decode_index"
            )
        );
    }
}
