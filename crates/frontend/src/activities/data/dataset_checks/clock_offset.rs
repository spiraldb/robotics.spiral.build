//! Camera clock offset: in episodes where a stream logs exactly one message
//! per frame of the camera that shows the workspace best, frame n and
//! message n record the same instant, so their timestamps should agree. The
//! offset between them is measured per episode, over the whole episode and
//! over its first five seconds, which tells a constant offset from a drift.
//! Each episode's cell keeps, for every stream logged more than once, its
//! offset in that episode when it logs one message per frame. The report
//! judges them in Spiral: which streams are logged at the same instants —
//! every episode reads alike in them — and are shown once, and the offset
//! over every episode.
use super::camera_frames::CameraFrames;
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::recording_rate::RecordingRate;
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use crate::activities::episode::StreamCatalog;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ClockOffset {
    /// The camera every stream is paired with; `None` without a camera.
    camera: Option<String>,
    period_ms: f64,
    pairings: Vec<ClockPairing>,
}

/// The streams logged at one set of instants, paired with the camera, as
/// the judgment answers them.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ClockPairing {
    streams: Vec<String>,
    #[serde(flatten)]
    offset: PooledOffset,
    /// How many episodes log as many of these streams' messages as camera
    /// frames.
    episodes: u64,
    /// Whether the camera and these streams disagree by more than a frame,
    /// episodes disagree with each other by more than one, or an episode
    /// opens more than half a frame away from where it runs.
    flagged: bool,
    /// The episodes whose frames sit over a frame interval from their
    /// messages, or drift over half of one in their first five seconds, when
    /// the pairing is flagged; none when no episode does.
    apart: Option<Vec<EpisodeOffset>>,
    /// The three episodes whose offset is furthest from the others'.
    furthest: Vec<EpisodeOffset>,
}

/// Frame time minus message time over every episode: the median of the
/// episodes' medians, each episode weighing alike, and the lowest and the
/// highest of any pair.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
struct PooledOffset {
    median_ms: f64,
    low_ms: f64,
    high_ms: f64,
}

/// Frame time minus message time in one episode: the median over its pairs.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeOffset {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    median_ms: f64,
}

impl ClockOffset {
    /// The stretch at the start of an episode compared with the whole of it.
    const OPENING_NS: u64 = 5_000_000_000;

    /// The column keeping each episode's offset from the camera of every
    /// stream logged more than once, in the episodes logging one of its
    /// messages per frame; `None` without a camera or such a stream. Which
    /// streams share a clock is only known once every episode is read, so
    /// each stream is paired on its own.
    pub(super) fn column(streams: CheckedStreams<'_>) -> Result<Option<CheckColumn>, String> {
        let Some(camera) = Self::camera(streams) else {
            return Ok(None);
        };
        let parts = streams
            .topics()
            .iter()
            .filter(|topic| !topic.is_video() && RecordingRate::repeats(topic))
            .fold(
                PartsStatement::new(&["median_ms", "opening_ms", "low_ms", "high_ms"]),
                |parts, stream| parts.part(stream.name(), Self::sql(streams, (camera, stream))),
            );
        CheckColumn::of(CheckKind::Clock, parts)
    }

    /// Each set of streams logged at the same instants, once, and its offset
    /// from the camera, judged in Spiral over the column among `columns`
    /// that `table` holds against the camera's usual frame interval in
    /// `frames`.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        streams: CheckedStreams<'_>,
        frames: &CameraFrames,
    ) -> Result<Self, String> {
        let period_ms = Self::camera(streams)
            .and_then(|camera| frames.period_ns(camera.name()))
            .unwrap_or_default()
            / 1e6;
        let judged = match table.parts(columns, CheckKind::Clock) {
            Some(parts) => table.rows(Self::judgment(&parts, period_ms)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(streams, period_ms, judged))
    }

    /// The judgment over `parts`, every episode's offsets, against a frame
    /// interval of `period_ms`. Streams whose every episode reads alike, to
    /// a hundredth of a millisecond, are logged at the same instants: they
    /// pair as one, read from the first of them by name. Per pairing, the
    /// median of its episodes' medians, the lowest and highest of any pair,
    /// whether it is flagged and the episodes that flag it, and the three
    /// episodes furthest from its median.
    fn judgment(parts: &str, period_ms: f64) -> String {
        format!(
            "WITH o AS (SELECT episode_id, part, median_ms, opening_ms, low_ms, high_ms, CAST(round(median_ms * 100) AS BIGINT) AS centi_ms FROM {parts} p), \
             n AS (SELECT part, count(*) AS episodes FROM o GROUP BY part), \
             a AS (SELECT mine.part AS part, theirs.part AS peer, count(*) AS episodes FROM o mine \
               JOIN o theirs ON theirs.episode_id = mine.episode_id AND theirs.centi_ms = mine.centi_ms GROUP BY mine.part, theirs.part), \
             k AS (SELECT a.part AS part, min(a.peer) AS lead FROM a JOIN n mine ON mine.part = a.part JOIN n theirs ON theirs.part = a.peer \
               WHERE a.episodes = mine.episodes AND a.episodes = theirs.episodes GROUP BY a.part), \
             g AS (SELECT lead, spql.list.collect(part) AS streams FROM k GROUP BY lead), \
             e AS (SELECT o.episode_id AS episode_id, o.part AS part, o.median_ms AS median_ms, o.opening_ms AS opening_ms, o.low_ms AS low_ms, o.high_ms AS high_ms \
               FROM o JOIN g ON g.lead = o.part), \
             t AS (SELECT part, spql.agg.approx_quantile(median_ms, 0.5) AS median_ms, min(low_ms) AS low_ms, max(high_ms) AS high_ms, count(*) AS episodes, \
               max(median_ms) - min(median_ms) AS spread, max(abs(opening_ms - median_ms)) AS drift FROM e GROUP BY part), \
             j AS (SELECT part, median_ms, low_ms, high_ms, episodes, \
               CASE WHEN abs(median_ms) > {frame} OR spread > {frame} OR drift > {half} THEN true ELSE false END AS flagged FROM t), \
             x AS (SELECT e.part AS part, spql.list.collect(STRUCT_PACK(episode_id := e.episode_id, median_ms := e.median_ms)) AS apart \
               FROM e JOIN j ON j.part = e.part WHERE j.flagged AND (abs(e.median_ms) > {frame} OR abs(e.opening_ms - e.median_ms) > {half}) GROUP BY e.part), \
             r AS (SELECT e.part AS part, e.episode_id AS episode_id, e.median_ms AS median_ms, \
               ROW_NUMBER() OVER (PARTITION BY e.part ORDER BY abs(e.median_ms - t.median_ms) DESC) AS distance_rank FROM e JOIN t ON t.part = e.part), \
             f AS (SELECT part, spql.list.collect(STRUCT_PACK(episode_id := episode_id, median_ms := median_ms)) AS furthest FROM r WHERE distance_rank <= 3 GROUP BY part) \
             SELECT g.streams AS streams, j.median_ms AS median_ms, j.low_ms AS low_ms, j.high_ms AS high_ms, j.episodes AS episodes, j.flagged AS flagged, \
               x.apart AS apart, f.furthest AS furthest \
             FROM g JOIN j ON j.part = g.lead JOIN f ON f.part = g.lead LEFT JOIN x ON x.part = g.lead",
            frame = period_ms,
            half = period_ms / 2.0,
        )
    }

    /// The pairings `judged`, against a frame interval of `period_ms`, in the
    /// order `streams` declares their streams: the first leads the finding.
    fn of(streams: CheckedStreams<'_>, period_ms: f64, judged: Vec<ClockPairing>) -> Self {
        let mut pairings = judged
            .into_iter()
            .map(|pairing| pairing.ordered(streams))
            .collect::<Vec<_>>();
        pairings.sort_by_cached_key(|pairing| {
            pairing
                .streams
                .first()
                .and_then(|stream| ClockPairing::declared(streams, stream))
        });
        Self {
            camera: Self::camera(streams).map(|camera| camera.name().to_owned()),
            period_ms,
            pairings,
        }
    }

    /// The camera every stream is paired with: the one that shows the
    /// workspace best.
    fn camera<'a>(streams: CheckedStreams<'a>) -> Option<&'a DatasetTopic> {
        let cameras = streams
            .topics()
            .iter()
            .filter(|topic| topic.is_video())
            .collect::<Vec<_>>();
        let names = cameras
            .iter()
            .map(|topic| topic.name().to_owned())
            .collect::<Vec<_>>();
        let overview = StreamCatalog::overview_camera(&names)?;
        cameras.into_iter().find(|topic| topic.name() == overview)
    }

    /// Each episode's offset between `camera` and `stream`: frame n paired
    /// with message n, in episodes logging as many of each, over the whole
    /// episode and over its opening, and the lowest and highest of any pair.
    fn sql(
        streams: CheckedStreams<'_>,
        (camera, stream): (&DatasetTopic, &DatasetTopic),
    ) -> String {
        let numbered = "log_time, ROW_NUMBER() OVER (PARTITION BY id ORDER BY log_time) AS i, count(*) OVER (PARTITION BY id) AS n";
        format!(
            "WITH \
             c AS (SELECT id, {numbered} FROM {camera}), \
             s AS (SELECT id, {numbered} FROM {stream}), \
             p AS (SELECT c.id AS id, (CAST(c.log_time AS BIGINT) - CAST(s.log_time AS BIGINT)) / 1e6 AS offset_ms, \
               CAST(c.log_time AS BIGINT) - min(CAST(c.log_time AS BIGINT)) OVER (PARTITION BY c.id) AS since_ns \
               FROM c JOIN s ON s.id = c.id AND s.i = c.i WHERE c.n = s.n) \
             SELECT id, spql.agg.approx_quantile(offset_ms, 0.5) AS median_ms, \
               spql.agg.approx_quantile(CASE WHEN since_ns < {opening} THEN offset_ms END, 0.5) AS opening_ms, \
               min(offset_ms) AS low_ms, max(offset_ms) AS high_ms FROM p GROUP BY id",
            camera = streams.read(camera),
            stream = streams.read(stream),
            opening = Self::OPENING_NS,
        )
    }

    pub(super) fn row(&self, episodes: u64) -> CheckRow {
        const NAME: &str = "Clock offset";
        let Some(camera) = &self.camera else {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why("No camera to pair"),
            );
        };
        let Some(first) = self.pairings.first() else {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why(format!("No stream logs one message per {camera} frame")),
            );
        };
        let flagged = self.pairings.iter().any(|pairing| pairing.flagged);
        let others = self
            .pairings
            .iter()
            .map(|pairing| pairing.streams.len())
            .sum::<usize>();
        // How far apart the camera's frames and the streams logged with them
        // are stamped, and which comes first when they are apart at all.
        let (offset, order) = match first.offset.median_ms {
            ms if ms.abs() < 0.5 => ("0 ms".to_owned(), ""),
            ms if ms < 0.0 => (Milliseconds::magnitude(ms), ", frames first"),
            ms => (Milliseconds::magnitude(ms), ", frames last"),
        };
        let finding = Finding::of(offset, "between")
            .and(camera.clone(), "and")
            .and_count(others, "stream", &format!("logged with it{order}"));
        let apart = self
            .pairings
            .iter()
            .flat_map(ClockPairing::apart)
            .map(EpisodeOffset::evidence)
            .collect::<Vec<_>>();
        let pairings = self.pairings.clone();
        let period = self.period_ms;
        let camera = camera.clone();
        CheckRow::new(NAME, if flagged { Verdict::Flagged } else { Verdict::Clean }, finding).flagging(apart).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Clock offsets",
                thead { tr {
                    th { "Streams" } th { class: "is-number", "Offset" } th { class: "is-number", "Range" }
                    th { class: "is-number", "Episodes" } th { "Furthest from the rest" }
                } }
                tbody {
                    for pairing in pairings {
                        tr { key: "{pairing.streams[0]}",
                            td { class: "data-checks-streams",
                                for stream in pairing.streams.clone() {
                                    StreamName { key: "{stream}", topic: stream, camera: false }
                                }
                            }
                            td { class: "is-number", "{Milliseconds::signed(pairing.offset.median_ms)}" }
                            td { class: "is-number", "{Milliseconds::signed(pairing.offset.low_ms)} to {Milliseconds::signed(pairing.offset.high_ms)}" }
                            td { class: "is-number", "{pairing.episodes} of {episodes}" }
                            td { EvidenceChips { evidence: pairing.furthest.iter().map(EpisodeOffset::evidence).collect::<Vec<_>>() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "Frame n of {camera} against message n of each stream, in the episodes that log as many of both; frame time minus message time. More than one frame interval ({period:.1} ms) apart, between episodes, or over an episode's first five seconds, is flagged."
            }
        })
    }
}

impl ClockPairing {
    /// This pairing with its streams in the order `streams` declares them,
    /// and its furthest episodes furthest first.
    fn ordered(mut self, streams: CheckedStreams<'_>) -> Self {
        self.streams
            .sort_by_cached_key(|stream| Self::declared(streams, stream));
        let typical = self.offset.median_ms;
        self.furthest.sort_by(|left, right| {
            (right.median_ms - typical)
                .abs()
                .total_cmp(&(left.median_ms - typical).abs())
        });
        self
    }
    /// Where `streams` declares `stream`.
    fn declared(streams: CheckedStreams<'_>, stream: &str) -> Option<usize> {
        streams
            .topics()
            .iter()
            .position(|topic| topic.name() == stream)
    }
    /// The episodes that flag this pairing.
    fn apart(&self) -> impl Iterator<Item = &EpisodeOffset> {
        self.apart.iter().flatten()
    }
}

impl EpisodeOffset {
    fn evidence(&self) -> Evidence {
        Evidence::whole(self.episode.clone(), Milliseconds::signed(self.median_ms))
    }
}

/// A duration in milliseconds as the report writes it.
pub(super) struct Milliseconds;

impl Milliseconds {
    pub(super) fn signed(ms: f64) -> String {
        if ms.abs() < 0.05 {
            "0 ms".into()
        } else {
            format!("{:+.0} ms", ms).replace('-', "−")
        }
    }
    fn magnitude(ms: f64) -> String {
        format!("{:.0} ms", ms.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::curation::AnswerSource;
    use spiral_robotics_common::datasets::DatasetSelection;

    fn episode(ordinal: u32) -> String {
        format!("{ordinal:032x}")
    }

    #[test]
    fn the_judgment_pairs_streams_whose_episodes_read_alike_and_flags_them_in_spiral() {
        let sql = ClockOffset::judgment("(parts)", 33.4);
        assert!(
            sql.contains("CAST(round(median_ms * 100) AS BIGINT) AS centi_ms FROM (parts) p"),
            "{sql}"
        );
        // Streams read alike when every episode of each reads alike in the other.
        assert!(
            sql.contains("JOIN o theirs ON theirs.episode_id = mine.episode_id AND theirs.centi_ms = mine.centi_ms"),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE a.episodes = mine.episodes AND a.episodes = theirs.episodes"),
            "{sql}"
        );
        assert!(
            sql.contains("spql.agg.approx_quantile(median_ms, 0.5) AS median_ms"),
            "{sql}"
        );
        assert!(
            sql.contains("CASE WHEN abs(median_ms) > 33.4 OR spread > 33.4 OR drift > 16.7 THEN true ELSE false END AS flagged"),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE j.flagged AND (abs(e.median_ms) > 33.4 OR abs(e.opening_ms - e.median_ms) > 16.7)"),
            "{sql}"
        );
        assert!(sql.contains("WHERE distance_rank <= 3"), "{sql}");
    }

    #[test]
    fn each_repeated_stream_pairs_with_the_overview_camera_and_pairings_read_in_declared_order() {
        let dataset = DatasetSelection::new(vec!["dataset_folding".into()]).unwrap();
        let video = || vec!["foxglove.CompressedVideo".to_owned()];
        let topics = [
            DatasetTopic::new("/left-wrist-camera".into(), video()).with_coverage(2, Some(7)),
            DatasetTopic::new("/top-camera".into(), video()).with_coverage(2, Some(7)),
            DatasetTopic::new("/left-arm-state".into(), vec![]).with_coverage(2, Some(7)),
            DatasetTopic::new("/left-arm-action".into(), vec![]).with_coverage(2, Some(7)),
            DatasetTopic::new("/right-arm-state".into(), vec![]).with_coverage(2, Some(7)),
            DatasetTopic::new("/instruction".into(), vec![]).with_coverage(2, Some(2)),
        ];
        let streams = CheckedStreams::new(&dataset, &topics);
        let column = ClockOffset::column(streams).unwrap().unwrap();
        assert_eq!(column.kind(), CheckKind::Clock);
        let fill = column.fill(streams);
        let AnswerSource::DatasetStatement { statement } = fill.source() else {
            panic!("a check fills its column from one statement over every episode");
        };
        // Every stream logged more than once pairs with the overview camera.
        assert!(statement.contains("'/left-arm-state' AS part"));
        assert!(statement.contains("'/left-arm-action' AS part"));
        assert!(!statement.contains("'/instruction'"));
        assert!(statement.contains("'/top-camera'"));
        assert!(!statement.contains("'/left-wrist-camera'"));
        assert!(
            ClockOffset::column(CheckedStreams::new(&dataset, &topics[2..]))
                .unwrap()
                .is_none()
        );

        let judged: Vec<ClockPairing> = serde_json::from_value(json!([
            {
                "streams": ["/right-arm-state"], "median_ms": -10.0, "low_ms": -12.0, "high_ms": -8.0,
                "episodes": 1, "flagged": false, "apart": null,
                "furthest": [{"episode_id": episode(1), "median_ms": -10.0}]
            },
            {
                "streams": ["/left-arm-action", "/left-arm-state"], "median_ms": -105.0, "low_ms": -130.0, "high_ms": -30.0,
                "episodes": 3, "flagged": true,
                "apart": [{"episode_id": episode(1), "median_ms": -100.0}, {"episode_id": episode(2), "median_ms": -110.0}],
                "furthest": [
                    {"episode_id": episode(1), "median_ms": -100.0},
                    {"episode_id": episode(3), "median_ms": -126.0},
                    {"episode_id": episode(2), "median_ms": -110.0}
                ]
            }
        ]))
        .unwrap();
        let clock = ClockOffset::of(streams, 33.3, judged);
        assert_eq!(clock.camera.as_deref(), Some("/top-camera"));
        // The pairing whose stream is declared first leads, its streams as
        // declared, and its furthest episode first.
        let first = &clock.pairings[0];
        assert_eq!(first.streams, ["/left-arm-state", "/left-arm-action"]);
        assert_eq!(first.offset.median_ms, -105.0);
        assert!(first.flagged);
        assert_eq!(first.furthest[0].evidence().measure(), "−126 ms");
        assert_eq!(first.apart().count(), 2);
        // A pairing no episode flags names none.
        assert_eq!(clock.pairings[1].apart().count(), 0);
        assert_eq!(Milliseconds::signed(0.0), "0 ms");
        assert_eq!(Milliseconds::signed(12.4), "+12 ms");
    }
}
