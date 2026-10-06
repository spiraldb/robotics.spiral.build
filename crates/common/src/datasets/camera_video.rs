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

    /// Two `WITH` entries: `packets` reads this camera's packets of every
    /// episode `members` admits — a predicate on the topic read `r` — and
    /// `videos` assembles each episode's packets into one packet video,
    /// `(id, source)`.
    pub fn videos(self, (packets, videos): (&str, &str), members: &str) -> String {
        let timed = "spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(CAST(e.message_start_time AS BIGINT), 'ns') AS presentation_time";
        format!(
            "{packets} AS ({}), \
             {videos} AS (SELECT id, spql.video.from_packets(\
             spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source \
             FROM {packets} GROUP BY id)",
            self.read(timed, members)
        )
    }

    /// One statement listing this camera's packets of the episode bound as
    /// `$1`, in decode order, as the progress service reads them: `payload`,
    /// the access unit exactly as recorded; `decode_index`, from 0; and
    /// `pts_ns`, its time from the episode's first message.
    pub fn packets(self) -> String {
        let timed = "CAST(r.log_time AS BIGINT) - CAST(e.message_start_time AS BIGINT) AS pts_ns";
        format!(
            "SELECT payload, decode_index, pts_ns FROM ({}) packets ORDER BY decode_index",
            self.read(timed, "r.id = $1")
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

    /// Every packet of the episodes `members` admits, numbered in decode
    /// order within its episode and timed by `timed`.
    fn read(self, timed: &str, members: &str) -> String {
        let dataset = SqlLiteral::text(self.dataset);
        format!(
            "SELECT r.id, r.value.data AS payload, \
             CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, \
             {timed} FROM spql.mcap.read_topic({dataset}, {}) r \
             JOIN spql.mcap.episodes({dataset}) e ON e.id = r.id WHERE {members}",
            SqlLiteral::text(self.camera)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CameraVideo;

    #[test]
    fn one_episode_is_the_bound_member_and_many_are_any_predicate() {
        let video = CameraVideo::new("d", "/c");
        assert_eq!(
            video.videos(("packets", "streams"), "r.id = $1"),
            "packets AS (SELECT r.id, r.value.data AS payload, CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(CAST(e.message_start_time AS BIGINT), 'ns') AS presentation_time FROM spql.mcap.read_topic('d', '/c') r JOIN spql.mcap.episodes('d') e ON e.id = r.id WHERE r.id = $1), streams AS (SELECT id, spql.video.from_packets(spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source FROM packets GROUP BY id)"
        );
        let many = video.videos(("p", "v"), "r.id IN (SELECT episode_id FROM kept)");
        assert!(many.starts_with("p AS (SELECT r.id,"));
        assert!(many.contains("WHERE r.id IN (SELECT episode_id FROM kept)), v AS (SELECT id,"));
        assert!(many.ends_with("FROM p GROUP BY id)"));
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
            "SELECT payload, decode_index, pts_ns FROM (SELECT r.id, r.value.data AS payload, CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, CAST(r.log_time AS BIGINT) - CAST(e.message_start_time AS BIGINT) AS pts_ns FROM spql.mcap.read_topic('d', '/c') r JOIN spql.mcap.episodes('d') e ON e.id = r.id WHERE r.id = $1) packets ORDER BY decode_index"
        );
    }
}
