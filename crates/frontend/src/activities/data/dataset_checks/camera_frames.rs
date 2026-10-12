//! Dropped and late camera frames: how many frames each camera should have
//! logged over an episode at its usual frame interval, how many it did, its
//! longest gap, and the frames logged late and caught up on — a long gap
//! followed by a short one, which drops no frame. Each episode's cell keeps,
//! for every camera, its frames, span and longest gap, and its own usual
//! interval, which its late frames are judged against; the camera's usual
//! interval, which missing frames are counted at, is read over every cell.
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::recording_rate::Median;
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CameraFrames {
    cameras: Vec<CameraGaps>,
}

#[derive(Clone, Debug, PartialEq)]
struct CameraGaps {
    topic: String,
    /// The camera's usual frame interval: the median of its episodes' own.
    period_ns: f64,
    episodes: Vec<EpisodeFrames>,
}

/// One camera's frames in one episode.
#[derive(Clone, Debug, PartialEq)]
struct EpisodeFrames {
    episode: EpisodeId,
    timing: FrameTiming,
}

/// How one camera logged its frames in one episode.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
struct FrameTiming {
    frames: u64,
    span_ns: u64,
    /// The longest gap between two frames; none when the camera logged
    /// one frame.
    worst_gap_ns: Option<u64>,
    /// Seconds into the episode where the longest gap ends.
    worst_at: Option<f64>,
    /// Frames logged late and caught up on, at the episode's own interval.
    late: u64,
    /// The episode's own usual frame interval: the median gap between its
    /// frames; none when the camera logged one frame.
    period_ns: Option<f64>,
}

/// One camera's frames in one episode, as its cell keeps them.
#[derive(Deserialize)]
pub(super) struct FramesPart {
    part: String,
    #[serde(flatten)]
    timing: FrameTiming,
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

    /// Every camera's frames in each episode its `cells` measured.
    pub(super) fn read(streams: CheckedStreams<'_>, cells: &CheckCells<FramesPart>) -> Self {
        Self {
            cameras: streams
                .topics()
                .iter()
                .filter(|topic| topic.is_video())
                .map(|camera| CameraGaps::read(camera.name(), cells))
                .collect(),
        }
    }

    /// The usual frame interval of `camera`, once measured.
    pub(super) fn period_ns(&self, camera: &str) -> Option<f64> {
        self.cameras
            .iter()
            .find(|gaps| gaps.topic == camera)
            .map(|gaps| gaps.period_ns)
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
               max(gap) OVER (PARTITION BY id) AS worst_gap, spql.agg.approx_quantile(gap, 0.5) OVER (PARTITION BY id) AS period FROM g), \
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
        let (missing, expected) = self
            .cameras
            .iter()
            .flat_map(|camera| {
                camera
                    .episodes
                    .iter()
                    .map(|episode| episode.timing.counts(camera.period_ns))
            })
            .fold((0.0, 0.0), |(missing, expected), (m, e)| {
                (missing + m, expected + e)
            });
        let worst = self
            .cameras
            .iter()
            .flat_map(|camera| camera.episodes.iter())
            .filter_map(|episode| episode.timing.worst_gap_ns)
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
                        episode.timing.worst_at,
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
                                    evidence: camera.flagged().into_iter().map(|(episode, share)| Evidence::at(episode.episode.clone(), episode.timing.worst_at, Percent::of(share))).collect::<Vec<_>>(),
                                }
                            }
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
    /// `camera`'s frames in each episode `cells` measured it, at the median
    /// of those episodes' own usual intervals: every episode weighs alike,
    /// however many frames it logged.
    fn read(camera: &str, cells: &CheckCells<FramesPart>) -> Self {
        let episodes = cells
            .parts()
            .filter(|(_, part)| part.part == camera)
            .map(|(episode, part)| EpisodeFrames {
                episode: episode.clone(),
                timing: part.timing,
            })
            .collect::<Vec<_>>();
        Self {
            topic: camera.to_owned(),
            period_ns: Median::of(
                episodes
                    .iter()
                    .filter_map(|episode| episode.timing.period_ns),
            )
            .unwrap_or_default(),
            episodes,
        }
    }
    fn missing_share(&self) -> f64 {
        let (missing, expected) = self
            .episodes
            .iter()
            .map(|episode| episode.timing.counts(self.period_ns))
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
        self.episodes
            .iter()
            .map(|episode| episode.timing.late)
            .sum()
    }
    /// The episode with the camera's longest gap, opened where it ends.
    fn longest(&self) -> Option<Evidence> {
        let (episode, worst_gap_ns) = self
            .episodes
            .iter()
            .filter_map(|episode| Some((episode, episode.timing.worst_gap_ns?)))
            .max_by_key(|(_, worst_gap_ns)| *worst_gap_ns)?;
        Some(Evidence::at(
            episode.episode.clone(),
            episode.timing.worst_at,
            format!("{:.0} ms", worst_gap_ns as f64 / 1e6),
        ))
    }
    /// Episodes missing more than the flagged share of their frames, most
    /// missing first.
    fn flagged(&self) -> Vec<(&EpisodeFrames, f64)> {
        let mut flagged = self
            .episodes
            .iter()
            .map(|episode| {
                let (missing, expected) = episode.timing.counts(self.period_ns);
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

impl FrameTiming {
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

    use spiral_robotics_common::curation::AnswerSource;
    use spiral_robotics_common::datasets::DatasetSelection;

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    fn timing(frames: u64, span_ns: u64, period_ns: f64) -> FrameTiming {
        FrameTiming {
            frames,
            span_ns,
            worst_gap_ns: Some(133_900_000),
            worst_at: Some(12.5),
            late: 3,
            period_ns: Some(period_ns),
        }
    }

    #[test]
    fn frames_count_against_the_span_at_the_usual_interval() {
        let episode = EpisodeFrames {
            episode: episode(1),
            timing: timing(2_980, 99_966_000_000, 33_390_000.0),
        };
        let (missing, expected) = episode.timing.counts(33_390_000.0);
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

    #[test]
    fn each_camera_is_a_part_and_its_interval_the_median_of_its_episodes() {
        let dataset = DatasetSelection::new(vec!["dataset_folding".into()]).unwrap();
        let video = || vec!["foxglove.CompressedVideo".to_owned()];
        let topics = [
            DatasetTopic::new("/top-camera".into(), video()).with_coverage(3, Some(900)),
            DatasetTopic::new("/wrist-camera".into(), video()).with_coverage(3, Some(900)),
            DatasetTopic::new("/joints".into(), vec![]).with_coverage(3, Some(900)),
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
        assert!(statement.contains("approx_quantile(gap, 0.5) OVER (PARTITION BY id)"));
        let no_camera = [DatasetTopic::new("/joints".into(), vec![])];
        assert!(
            CameraFrames::column(CheckedStreams::new(&dataset, &no_camera))
                .unwrap()
                .is_none()
        );

        let part = |frames, period_ns| FramesPart {
            part: "/top-camera".into(),
            timing: timing(frames, 9_900_000_000, period_ns),
        };
        let cells = CheckCells::fixture(vec![
            (episode(1), vec![part(301, 33.0e6)]),
            (episode(2), vec![part(300, 34.0e6)]),
            (episode(3), vec![part(250, 40.0e6)]),
        ]);
        let frames = CameraFrames::read(streams, &cells);
        assert_eq!(frames.cameras.len(), 2);
        assert_eq!(frames.period_ns("/top-camera"), Some(34.0e6));
        assert_eq!(frames.cameras[0].episodes.len(), 3);
        // A camera no cell measured has no interval.
        assert_eq!(frames.period_ns("/wrist-camera"), None);
        assert!(frames.cameras[1].episodes.is_empty());
    }
}
