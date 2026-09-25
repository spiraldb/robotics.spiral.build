//! One camera of the episodes a statement reads, each assembled out of its
//! recorded packets into a Spiral packet video.
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
        let dataset = SqlLiteral::text(self.dataset);
        format!(
            "{packets} AS (SELECT r.id, r.value.data AS payload, \
             CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index, \
             spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(CAST(e.message_start_time AS BIGINT), 'ns') AS presentation_time \
             FROM spql.mcap.read_topic({dataset}, {}) r \
             JOIN spql.mcap.episodes({dataset}) e ON e.id = r.id WHERE {members}), \
             {videos} AS (SELECT id, spql.video.from_packets(\
             spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source \
             FROM {packets} GROUP BY id)",
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
}
