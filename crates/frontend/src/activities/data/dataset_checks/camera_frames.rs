//! Dropped and late camera frames: how many frames each camera should have
//! logged over an episode at its usual frame interval, how many it did, its
//! longest gap, and the frames logged late and caught up on — a long gap
//! followed by a short one, which drops no frame. Each episode's cell keeps,
//! for every camera, its frames, span and longest gap, and its own usual
//! interval, which its late frames are judged against. The report judges
//! them in Spiral: the camera's usual interval — the median of its episodes'
//! own — the frames missing at it, and the episodes that miss the most.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};
use spiral_robotics_common::sql::SqlStruct;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CameraFrames {
    cameras: Vec<CameraGaps>,
}

/// One camera's frames over every episode, as the judgment answers them.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct CameraGaps {
    #[serde(rename = "part")]
    topic: String,
    /// The camera's usual frame interval: the median of its episodes' own,
    /// each episode weighing alike, however many frames it logged; none
    /// until an episode logs two frames.
    period_ns: Option<f64>,
    #[serde(flatten)]
    counts: FrameCounts,
    /// Frames logged late and caught up on, at each episode's own interval.
    late: u64,
    /// The episode with the camera's longest gap; none when every episode
    /// logged one frame.
    longest: Option<LongestGap>,
    /// Episodes missing more than the flagged share of their frames; none
    /// when no episode does.
    flagged: Option<Vec<MissingFrames>>,
}

/// Whole frames missing, and frames expected, at a camera's usual interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
struct FrameCounts {
    missing: f64,
    expected: f64,
}

/// The longest gap between two of a camera's frames, and the episode and
/// the seconds into it where the gap ends.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct LongestGap {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    gap_ns: u64,
    at: Option<f64>,
}

/// An episode missing more than the flagged share of a camera's frames, and
/// the seconds into it where its longest gap ends.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct MissingFrames {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    share: f64,
    at: Option<f64>,
}

impl CameraFrames {
    /// An episode missing more than this share of its frames is flagged.
    const MISSING: f64 = 0.01;
    /// A frame logged later than this share of an interval, and followed by
    /// a gap shorter than the rest of it, was late rather than dropped.
    const LATE: f64 = 0.4;

    /// The column keeping each episode's frames of every camera; `None`
    /// without a camera.
    pub(super) fn column(streams: CheckedStreams<'_>) -> Result<Option<CheckColumn>, String> {
        let parts = streams
            .topics()
            .iter()
            .filter(|topic| topic.is_video())
            .fold(
                PartsStatement::new(&[
                    "frames",
                    "span_ns",
                    "worst_gap_ns",
                    "worst_at",
                    "late",
                    "period_ns",
                ]),
                |parts, camera| parts.part(camera.name(), Self::sql(streams, camera)),
            );
        CheckColumn::of(CheckKind::Frames, parts)
    }

    /// Every camera's frames over its episodes, judged in Spiral over the
    /// column among `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        streams: CheckedStreams<'_>,
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Frames) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(streams, judged))
    }

    /// The judgment over `parts`, every episode's frames: per camera, its
    /// usual interval, the frames missing and expected at it over every
    /// episode — an episode expects a frame per interval of its span, and
    /// one more — its late frames, the episode with its longest gap, and the
    /// episodes missing more than the flagged share of their frames.
    fn judgment(parts: &str) -> String {
        format!(
            "WITH f AS (SELECT episode_id, part, CAST(frames AS DOUBLE) AS frames, CAST(span_ns AS DOUBLE) AS span_ns, \
               worst_gap_ns, worst_at, late, period_ns FROM {parts} p), \
             t AS (SELECT part, approx_percentile_cont(period_ns, 0.5) AS period_ns FROM f GROUP BY part), \
             e AS (SELECT f.episode_id AS episode_id, f.part AS part, f.frames AS frames, f.worst_gap_ns AS worst_gap_ns, f.worst_at AS worst_at, f.late AS late, \
               CASE WHEN t.period_ns > 0 THEN f.span_ns / t.period_ns + 1 ELSE f.frames END AS expected FROM f JOIN t ON t.part = f.part), \
             m AS (SELECT episode_id, part, worst_gap_ns, worst_at, late, expected, greatest(round(expected - frames), 0) AS missing FROM e), \
             s AS (SELECT part, sum(missing) AS missing, sum(expected) AS expected, sum(late) AS late FROM m GROUP BY part), \
             g AS (SELECT part, episode_id, worst_gap_ns, worst_at, ROW_NUMBER() OVER (PARTITION BY part ORDER BY worst_gap_ns DESC) AS gap_rank \
               FROM m WHERE worst_gap_ns IS NOT NULL), \
             w AS (SELECT part, {longest} AS longest FROM g WHERE gap_rank = 1), \
             d AS (SELECT part, {flagged} AS flagged FROM m WHERE missing / expected > {missing} GROUP BY part) \
             SELECT t.part AS part, t.period_ns AS period_ns, s.missing AS missing, s.expected AS expected, s.late AS late, \
               w.longest AS longest, d.flagged AS flagged \
             FROM t JOIN s ON s.part = t.part LEFT JOIN w ON w.part = t.part LEFT JOIN d ON d.part = t.part",
            missing = Self::MISSING,
            longest = SqlStruct::new()
                .field("episode_id", "episode_id")
                .field("gap_ns", "worst_gap_ns")
                .field("at", "worst_at"),
            flagged = SqlStruct::new()
                .field("episode_id", "episode_id")
                .field("share", "missing / expected")
                .field("at", "worst_at")
                .collected(),
        )
    }

    /// Every camera `streams` declares, as `judged` says it logged its
    /// frames; a camera the judgment does not name has no frames measured.
    fn of(streams: CheckedStreams<'_>, judged: Vec<CameraGaps>) -> Self {
        let cameras = streams
            .topics()
            .iter()
            .filter(|topic| topic.is_video())
            .map(|camera| {
                let mut gaps = judged
                    .iter()
                    .find(|gaps| gaps.topic == camera.name())
                    .cloned()
                    .unwrap_or_else(|| CameraGaps::unmeasured(camera.name()));
                // Most missing first.
                if let Some(flagged) = &mut gaps.flagged {
                    flagged.sort_by(|left, right| right.share.total_cmp(&left.share));
                }
                gaps
            })
            .collect();
        Self { cameras }
    }

    /// The usual frame interval of `camera`, once measured.
    pub(super) fn period_ns(&self, camera: &str) -> Option<f64> {
        self.cameras
            .iter()
            .find(|gaps| gaps.topic == camera)
            .and_then(|gaps| gaps.period_ns)
            .filter(|period| *period > 0.0)
    }

    /// Each episode's frames of `camera`, judged against the episode's own
    /// usual interval. One read of the camera numbers every frame's gap
    /// since the one before, the gap after it, and its episode's longest and
    /// median gap.
    fn sql(streams: CheckedStreams<'_>, camera: &DatasetTopic) -> String {
        let late = Self::LATE;
        format!(
            "WITH \
             g AS (SELECT id, log_time, CAST(log_time AS BIGINT) - CAST(LAG(log_time) OVER (PARTITION BY id ORDER BY log_time) AS BIGINT) AS gap FROM {camera}), \
             l AS (SELECT id, log_time, gap, LEAD(gap) OVER (PARTITION BY id ORDER BY log_time) AS next_gap, \
               max(gap) OVER (PARTITION BY id) AS worst_gap, approx_percentile_cont(gap, 0.5) OVER (PARTITION BY id) AS period FROM g), \
             f AS (SELECT id, count(*) AS frames, max(log_time) - min(log_time) AS span_ns, max(gap) AS worst_gap_ns, \
               min(CASE WHEN gap = worst_gap THEN log_time END) AS worst_time, \
               sum(CASE WHEN gap > {slow} * period AND next_gap < {quick} * period THEN 1 ELSE 0 END) AS late, max(period) AS period_ns \
               FROM l GROUP BY id) \
             SELECT f.id AS id, f.frames AS frames, f.span_ns AS span_ns, f.worst_gap_ns AS worst_gap_ns, {since} AS worst_at, f.late AS late, f.period_ns AS period_ns \
             FROM f JOIN {episodes} e ON e.id = f.id",
            camera = streams.read(camera),
            slow = 1.0 + late,
            quick = 1.0 - late,
            since = CheckedStreams::since_start("f.worst_time"),
            episodes = streams.episodes(),
        )
    }

    pub(super) fn row(&self) -> CheckRow {
        let counts = self
            .cameras
            .iter()
            .map(|camera| camera.counts)
            .fold(FrameCounts::default(), FrameCounts::with);
        let worst = self
            .cameras
            .iter()
            .filter_map(|camera| camera.longest.as_ref())
            .map(|longest| longest.gap_ns)
            .max()
            .unwrap_or_default();
        let flagged = self
            .cameras
            .iter()
            .flat_map(CameraGaps::flagged)
            .map(|episode| &episode.episode)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let longest = format!("{:.0} ms", worst as f64 / 1e6);
        let finding = if self.cameras.is_empty() {
            Finding::why("No camera recorded")
        } else if counts.missing == 0.0 {
            Finding::of_count(0, "frame", "missing,").and(longest, "longest gap")
        } else {
            Finding::of(Percent::of(counts.share()), "of frames missing,")
                .and(longest, "longest gap")
        };
        let verdict = match (self.cameras.is_empty(), flagged) {
            (true, _) => Verdict::Unavailable,
            (false, 0) => Verdict::Clean,
            _ => Verdict::Flagged,
        };
        let cameras = self.cameras.clone();
        let flagged_evidence = self
            .cameras
            .iter()
            .flat_map(CameraGaps::flagged)
            .map(MissingFrames::evidence)
            .collect::<Vec<_>>();
        CheckRow::new("Camera frames", verdict, finding).flagging(flagged_evidence).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Camera frames",
                thead { tr {
                    th { "Camera" } th { class: "is-number", "Interval" } th { class: "is-number", "Missing" }
                    th { class: "is-number", "Late" } th { "Longest gap" }
                    th { "Missing over {Percent::of(Self::MISSING)}" }
                } }
                tbody {
                    for camera in cameras {
                        tr { key: "{camera.topic}",
                            td { StreamName { topic: camera.topic.clone(), camera: true } }
                            td { class: "is-number", "{camera.period_ns.unwrap_or_default() / 1e6:.1} ms" }
                            td { class: "is-number", "{Percent::of(camera.counts.share())}" }
                            td { class: "is-number", "{camera.late}" }
                            td { EvidenceChips { evidence: camera.longest.iter().map(LongestGap::evidence).collect::<Vec<_>>() } }
                            td { EvidenceChips { evidence: camera.flagged().map(MissingFrames::evidence).collect::<Vec<_>>() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "An episode's frames are counted against its span at the camera's usual interval. A frame logged over {Self::LATE * 100.0:.0}% of the episode's usual interval late, that the next frame catches up on, is late rather than missing."
            }
        })
    }
}

impl CameraGaps {
    /// `camera`, which no episode's cell measured.
    fn unmeasured(camera: &str) -> Self {
        Self {
            topic: camera.to_owned(),
            period_ns: None,
            counts: FrameCounts::default(),
            late: 0,
            longest: None,
            flagged: None,
        }
    }
    /// Episodes missing more than the flagged share of their frames, most
    /// missing first.
    fn flagged(&self) -> impl Iterator<Item = &MissingFrames> {
        self.flagged.iter().flatten()
    }
}

impl FrameCounts {
    /// These counts and `other`'s together.
    fn with(self, other: Self) -> Self {
        Self {
            missing: self.missing + other.missing,
            expected: self.expected + other.expected,
        }
    }
    /// The share of the frames expected that are missing.
    fn share(self) -> f64 {
        if self.expected > 0.0 {
            self.missing / self.expected
        } else {
            0.0
        }
    }
}

impl LongestGap {
    /// The episode, opened where the gap ends.
    fn evidence(&self) -> Evidence {
        Evidence::at(
            self.episode.clone(),
            self.at,
            format!("{:.0} ms", self.gap_ns as f64 / 1e6),
        )
    }
}

impl MissingFrames {
    /// The episode, opened where its longest gap ends.
    fn evidence(&self) -> Evidence {
        Evidence::at(self.episode.clone(), self.at, Percent::of(self.share))
    }
}

/// A share as a percentage an operator reads at a glance.
pub(super) struct Percent;

impl Percent {
    pub(super) fn of(share: f64) -> String {
        let percent = share * 100.0;
        if percent == 0.0 {
            "0%".into()
        } else if percent < 0.1 {
            "<0.1%".into()
        } else {
            format!("{percent:.1}%")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::activities::data::dataset_checks::stream_reads::TwoArmDataset;
    use spiral_robotics_common::curation::AnswerSource;
    use spiral_robotics_common::datasets::DatasetSelection;

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    #[test]
    fn the_judgment_counts_missing_frames_at_each_cameras_median_interval_in_spiral() {
        let sql = CameraFrames::judgment("(parts)");
        assert!(sql.contains("FROM (parts) p"), "{sql}");
        assert!(
            sql.contains(
                "approx_percentile_cont(period_ns, 0.5) AS period_ns FROM f GROUP BY part"
            ),
            "{sql}"
        );
        // A camera without an interval expects the frames it logged.
        assert!(
            sql.contains("CASE WHEN t.period_ns > 0 THEN f.span_ns / t.period_ns + 1 ELSE f.frames END AS expected"),
            "{sql}"
        );
        assert!(
            sql.contains("greatest(round(expected - frames), 0) AS missing"),
            "{sql}"
        );
        // An episode that logged one frame has no gap to be the longest.
        assert!(
            sql.contains("FROM m WHERE worst_gap_ns IS NOT NULL"),
            "{sql}"
        );
        assert!(
            sql.contains("named_struct('episode_id', episode_id, 'gap_ns', worst_gap_ns, 'at', worst_at) AS longest FROM g WHERE gap_rank = 1"),
            "{sql}"
        );
        assert!(
            sql.contains("array_agg(named_struct('episode_id', episode_id, 'share', missing / expected, 'at', worst_at)) AS flagged \
                 FROM m WHERE missing / expected > 0.01 GROUP BY part"),
            "{sql}"
        );
        assert!(
            sql.contains("LEFT JOIN w ON w.part = t.part LEFT JOIN d ON d.part = t.part"),
            "{sql}"
        );
    }

    #[test]
    fn each_camera_is_a_part_and_reads_as_its_judgment_answers() {
        let dataset = DatasetSelection::new("folding").unwrap();
        let topics = [
            TwoArmDataset::stream("/top-camera", "foxglove.CompressedVideo").with_coverage(3, 900),
            TwoArmDataset::stream("/wrist-camera", "foxglove.CompressedVideo")
                .with_coverage(3, 900),
            TwoArmDataset::stream("/joints", "JointState").with_coverage(3, 900),
        ];
        let streams = CheckedStreams::new(&dataset, &topics);
        let column = CameraFrames::column(streams).unwrap().unwrap();
        assert_eq!(column.kind(), CheckKind::Frames);
        let fill = column.fill(streams);
        let AnswerSource::DatasetStatement { statement } = fill.source() else {
            panic!("a check fills its column from one statement over every episode");
        };
        assert!(statement.contains("'/top-camera' AS part"));
        assert!(statement.contains("'/wrist-camera' AS part"));
        assert!(!statement.contains("/joints"));
        // Late frames are judged against each episode's own interval.
        assert!(statement.contains("approx_percentile_cont(gap, 0.5) OVER (PARTITION BY id)"));
        let no_camera = [TwoArmDataset::stream("/joints", "JointState")];
        assert!(
            CameraFrames::column(CheckedStreams::new(&dataset, &no_camera))
                .unwrap()
                .is_none()
        );

        let judged: Vec<CameraGaps> = serde_json::from_value(serde_json::json!([{
            "part": "/top-camera", "period_ns": 33.39e6, "missing": 15.0, "expected": 2_994.9, "late": 3,
            "longest": {"episode_id": episode(2).to_string(), "gap_ns": 133_900_000, "at": 12.5},
            "flagged": [
                {"episode_id": episode(1).to_string(), "share": 0.012, "at": 3.0},
                {"episode_id": episode(3).to_string(), "share": 0.018, "at": null}
            ]
        }]))
        .unwrap();
        let frames = CameraFrames::of(streams, judged);
        assert_eq!(frames.cameras.len(), 2);
        assert_eq!(frames.period_ns("/top-camera"), Some(33.39e6));
        let top = &frames.cameras[0];
        assert_eq!(Percent::of(top.counts.share()), "0.5%");
        assert_eq!(top.longest.as_ref().unwrap().evidence().measure(), "134 ms");
        // Most missing first.
        assert_eq!(
            top.flagged()
                .map(|episode| episode.share)
                .collect::<Vec<_>>(),
            [0.018, 0.012]
        );
        // A camera no cell measured has no interval and nothing flagged.
        assert_eq!(frames.period_ns("/wrist-camera"), None);
        assert_eq!(frames.cameras[1].flagged().count(), 0);
        // A camera whose every episode logged one frame has no longest gap,
        // and one no episode misses frames of, no flagged list.
        let quiet: Vec<CameraGaps> = serde_json::from_value(serde_json::json!([{
            "part": "/top-camera", "period_ns": null, "missing": 0.0, "expected": 3.0, "late": 0,
            "longest": null, "flagged": null
        }]))
        .unwrap();
        let quiet = CameraFrames::of(streams, quiet);
        assert_eq!(quiet.period_ns("/top-camera"), None);
        assert!(quiet.cameras[0].longest.is_none());
        assert_eq!(quiet.cameras[0].flagged().count(), 0);
        assert_eq!(Percent::of(0.0), "0%");
        assert_eq!(Percent::of(0.018), "1.8%");
    }
}
