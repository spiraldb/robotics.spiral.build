//! Dropped and late camera frames: how many frames each camera should have
//! logged over an episode at its usual frame interval, how many it did, its
//! longest gap, and the frames logged late and caught up on — a long gap
//! followed by a short one, which drops no frame.
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct CameraFrames {
    cameras: Vec<CameraGaps>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CameraGaps {
    topic: String,
    /// The camera's usual frame interval: the median gap between frames.
    period_ns: f64,
    episodes: Vec<EpisodeFrames>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeFrames {
    #[serde(rename = "id")]
    episode: EpisodeId,
    frames: u64,
    span_ns: u64,
    worst_gap_ns: u64,
    /// Seconds into the episode where the longest gap ends.
    worst_at: Option<f64>,
    late: u64,
}

/// A camera's usual frame interval, over every episode.
#[derive(Deserialize)]
struct PeriodRow {
    period_ns: Option<f64>,
}

#[derive(Deserialize)]
struct FramesRow {
    period_ns: f64,
    #[serde(flatten)]
    frames: EpisodeFrames,
}

impl CameraFrames {
    /// An episode missing more than this share of its frames is flagged.
    const MISSING: f64 = 0.01;
    /// A frame logged later than this share of an interval, and followed by
    /// a gap shorter than the rest of it, was late rather than dropped.
    const LATE: f64 = 0.4;

    pub(super) async fn measure(streams: CheckedStreams<'_>) -> Result<Self, String> {
        let cameras = streams
            .topics()
            .iter()
            .filter(|topic| topic.is_video())
            .collect::<Vec<_>>();
        let mut measured = cameras
            .iter()
            .map(|topic| CameraGaps {
                topic: topic.name().to_owned(),
                period_ns: 0.0,
                episodes: Vec::new(),
            })
            .collect::<Vec<_>>();
        let periods = cameras
            .iter()
            .map(|topic| Self::period_sql(streams, topic))
            .collect::<Vec<_>>();
        let mut period_ns = vec![None; cameras.len()];
        for (camera, row) in streams.dataset_parts::<PeriodRow>(&periods).await? {
            period_ns[camera] = row.period_ns;
        }
        let frames = streams.parts::<FramesRow>(|streams| {
            Ok(cameras
                .iter()
                .zip(&period_ns)
                .map(|(topic, period)| Self::sql(streams, topic, *period))
                .collect())
        });
        for (camera, row) in frames.await? {
            measured[camera].period_ns = row.period_ns;
            measured[camera].episodes.push(row.frames);
        }
        Ok(Self { cameras: measured })
    }

    /// The usual frame interval of `camera`, once measured.
    pub(super) fn period_ns(&self, camera: &str) -> Option<f64> {
        self.cameras
            .iter()
            .find(|gaps| gaps.topic == camera)
            .map(|gaps| gaps.period_ns)
            .filter(|period| *period > 0.0)
    }

    /// Each frame of `camera` beside the nanoseconds since the one before.
    fn gaps(streams: CheckedStreams<'_>, camera: &DatasetTopic) -> String {
        format!(
            "SELECT id, log_time, CAST(log_time AS BIGINT) - CAST(LAG(log_time) OVER (PARTITION BY id ORDER BY log_time) AS BIGINT) AS gap FROM {}",
            streams.read(camera)
        )
    }

    /// The median gap between frames of `camera`, over every episode.
    fn period_sql(streams: CheckedStreams<'_>, camera: &DatasetTopic) -> String {
        format!(
            "SELECT spql.agg.approx_quantile(gap, 0.5) AS period_ns FROM ({})",
            Self::gaps(streams, camera)
        )
    }

    /// Each episode's frames of `camera`, judged against its usual interval.
    fn sql(streams: CheckedStreams<'_>, camera: &DatasetTopic, period_ns: Option<f64>) -> String {
        let late = Self::LATE;
        let period = period_ns.map_or_else(|| "NULL".to_owned(), |period| period.to_string());
        format!(
            "WITH \
             g AS ({gaps}), \
             p AS (SELECT CAST({period} AS DOUBLE) AS period), \
             l AS (SELECT id, log_time, gap, LEAD(gap) OVER (PARTITION BY id ORDER BY log_time) AS next_gap FROM g), \
             w AS (SELECT id, log_time, ROW_NUMBER() OVER (PARTITION BY id ORDER BY gap DESC) AS rank FROM g WHERE gap IS NOT NULL), \
             f AS (SELECT l.id AS id, count(*) AS frames, max(l.log_time) - min(l.log_time) AS span_ns, max(l.gap) AS worst_gap_ns, \
               sum(CASE WHEN l.gap > {slow} * p.period AND l.next_gap < {quick} * p.period THEN 1 ELSE 0 END) AS late, max(p.period) AS period_ns \
               FROM l CROSS JOIN p GROUP BY l.id) \
             SELECT f.id AS id, f.frames AS frames, f.span_ns AS span_ns, f.worst_gap_ns AS worst_gap_ns, f.late AS late, f.period_ns AS period_ns, {since} AS worst_at \
             FROM f JOIN {episodes} e ON e.id = f.id LEFT JOIN w ON w.id = f.id AND w.rank = 1",
            gaps = Self::gaps(streams, camera),
            slow = 1.0 + late,
            quick = 1.0 - late,
            since = CheckedStreams::since_start("w.log_time"),
            episodes = streams.episodes(),
        )
    }

    pub(super) fn row(&self) -> CheckRow {
        let (missing, expected) = self
            .cameras
            .iter()
            .flat_map(|camera| {
                camera
                    .episodes
                    .iter()
                    .map(|episode| episode.counts(camera.period_ns))
            })
            .fold((0.0, 0.0), |(missing, expected), (m, e)| {
                (missing + m, expected + e)
            });
        let worst = self
            .cameras
            .iter()
            .flat_map(|camera| camera.episodes.iter())
            .map(|episode| episode.worst_gap_ns)
            .max()
            .unwrap_or_default();
        let flagged = self
            .cameras
            .iter()
            .flat_map(CameraGaps::flagged)
            .map(|(episode, _)| &episode.episode)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let share = if expected > 0.0 {
            missing / expected
        } else {
            0.0
        };
        let longest = format!("{:.0} ms", worst as f64 / 1e6);
        let finding = if self.cameras.is_empty() {
            Finding::why("No camera recorded")
        } else if missing == 0.0 {
            Finding::of_count(0, "frame", "missing,").and(longest, "longest gap")
        } else {
            Finding::of(Percent::of(share), "of frames missing,").and(longest, "longest gap")
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
            .flat_map(|camera| {
                camera.flagged().into_iter().map(|(episode, share)| {
                    Evidence::at(
                        episode.episode.clone(),
                        episode.worst_at,
                        Percent::of(share),
                    )
                })
            })
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
                            td { class: "is-number", "{camera.period_ns / 1e6:.1} ms" }
                            td { class: "is-number", "{Percent::of(camera.missing_share())}" }
                            td { class: "is-number", "{camera.late()}" }
                            td { EvidenceChips { evidence: camera.longest().into_iter().collect::<Vec<_>>() } }
                            td {
                                EvidenceChips {
                                    evidence: camera.flagged().into_iter().map(|(episode, share)| Evidence::at(episode.episode.clone(), episode.worst_at, Percent::of(share))).collect::<Vec<_>>(),
                                }
                            }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "An episode's frames are counted against its span at the camera's usual interval. A frame logged over {Self::LATE * 100.0:.0}% of an interval late, that the next frame catches up on, is late rather than missing."
            }
        })
    }
}

impl CameraGaps {
    fn missing_share(&self) -> f64 {
        let (missing, expected) = self
            .episodes
            .iter()
            .map(|episode| episode.counts(self.period_ns))
            .fold((0.0, 0.0), |(missing, expected), (m, e)| {
                (missing + m, expected + e)
            });
        if expected > 0.0 {
            missing / expected
        } else {
            0.0
        }
    }
    fn late(&self) -> u64 {
        self.episodes.iter().map(|episode| episode.late).sum()
    }
    /// The episode with the camera's longest gap, opened where it ends.
    fn longest(&self) -> Option<Evidence> {
        let episode = self
            .episodes
            .iter()
            .max_by_key(|episode| episode.worst_gap_ns)?;
        Some(Evidence::at(
            episode.episode.clone(),
            episode.worst_at,
            format!("{:.0} ms", episode.worst_gap_ns as f64 / 1e6),
        ))
    }
    /// Episodes missing more than the flagged share of their frames, most
    /// missing first.
    fn flagged(&self) -> Vec<(&EpisodeFrames, f64)> {
        let mut flagged = self
            .episodes
            .iter()
            .map(|episode| {
                let (missing, expected) = episode.counts(self.period_ns);
                (
                    episode,
                    if expected > 0.0 {
                        missing / expected
                    } else {
                        0.0
                    },
                )
            })
            .filter(|(_, share)| *share > CameraFrames::MISSING)
            .collect::<Vec<_>>();
        flagged.sort_by(|left, right| right.1.total_cmp(&left.1));
        flagged
    }
}

impl EpisodeFrames {
    /// Whole frames missing, and frames expected, at interval `period_ns`.
    fn counts(&self, period_ns: f64) -> (f64, f64) {
        if period_ns <= 0.0 {
            return (0.0, self.frames as f64);
        }
        let expected = self.span_ns as f64 / period_ns + 1.0;
        ((expected - self.frames as f64).round().max(0.0), expected)
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

    #[test]
    fn frames_count_against_the_span_at_the_usual_interval() {
        let episode = EpisodeFrames {
            episode: EpisodeId::try_from(format!("{:032x}", 1)).unwrap(),
            frames: 2_980,
            span_ns: 99_966_000_000,
            worst_gap_ns: 133_900_000,
            worst_at: Some(12.5),
            late: 3,
        };
        let (missing, expected) = episode.counts(33_390_000.0);
        assert!((expected - 2_994.9).abs() < 0.1);
        assert_eq!(missing, 15.0);
        let camera = CameraGaps {
            topic: "/top-camera".into(),
            period_ns: 33_390_000.0,
            episodes: vec![episode],
        };
        assert_eq!(Percent::of(camera.missing_share()), "0.5%");
        assert!(camera.flagged().is_empty());
        assert_eq!(camera.late(), 3);
        assert_eq!(Percent::of(0.0), "0%");
        assert_eq!(Percent::of(0.018), "1.8%");
    }
}
