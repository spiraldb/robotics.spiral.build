//! Camera clock offset: in episodes where a stream logs exactly one message
//! per frame of the camera that shows the workspace best, frame n and
//! message n record the same instant, so their timestamps should agree. The
//! offset between them is measured per episode, over the whole episode and
//! over its first five seconds, which tells a constant offset from a drift.
//! Each episode's cell keeps, for every stream logged more than once, its
//! offset in that episode when it logs one message per frame; which streams
//! are logged at the same instants, read alike and are shown once, and the
//! offset over every episode, are read from every cell.
use super::camera_frames::CameraFrames;
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::recording_rate::{Median, RecordingRate, StreamTimings};
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

/// The streams logged at one set of instants, paired with the camera.
#[derive(Clone, Debug, PartialEq)]
struct ClockPairing {
    streams: Vec<String>,
    /// Frame time minus message time over every episode: the median of the
    /// episodes' medians, each episode weighing alike, and the lowest and
    /// the highest of any pair.
    median_ms: f64,
    low_ms: f64,
    high_ms: f64,
    episodes: Vec<EpisodeOffset>,
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeOffset {
    episode: EpisodeId,
    median_ms: f64,
    /// Over the episode's first five seconds.
    opening_ms: Option<f64>,
}

/// One stream's offset from the camera in one episode, as its cell keeps
/// it: frame time minus message time over the episode's pairs — their
/// median, the median over its first five seconds, the lowest and the
/// highest.
#[derive(Deserialize)]
pub(super) struct OffsetPart {
    part: String,
    median_ms: f64,
    opening_ms: Option<f64>,
    low_ms: f64,
    high_ms: f64,
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

    /// The offsets `cells` measured, each set of streams logged at the same
    /// instants — as `timings` tell them — once, judged against the
    /// camera's usual frame interval in `frames`.
    pub(super) fn read(
        streams: CheckedStreams<'_>,
        timings: &StreamTimings,
        frames: &CameraFrames,
        cells: &CheckCells<OffsetPart>,
    ) -> Self {
        let Some(camera) = Self::camera(streams).map(DatasetTopic::name) else {
            return Self {
                camera: None,
                period_ms: 0.0,
                pairings: Vec::new(),
            };
        };
        let pairings = Self::groups(streams, timings, camera)
            .iter()
            .map(|group| ClockPairing::read(group, cells))
            .collect();
        Self {
            camera: Some(camera.to_owned()),
            period_ms: frames.period_ns(camera).unwrap_or_default() / 1e6,
            pairings: ClockPairing::merged(pairings),
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

    /// The data streams logged at the same instants in every episode, each
    /// set once, when some episode logs them once per frame of `camera`.
    fn groups<'a>(
        streams: CheckedStreams<'a>,
        timings: &StreamTimings,
        camera: &str,
    ) -> Vec<Vec<&'a DatasetTopic>> {
        let Some(frames) = timings.of(camera) else {
            return Vec::new();
        };
        let mut groups: Vec<Vec<&DatasetTopic>> = Vec::new();
        for topic in streams.topics().iter().filter(|topic| !topic.is_video()) {
            let Some(timing) = timings.of(topic.name()) else {
                continue;
            };
            let per_frame = timing.iter().any(|(episode, timing)| {
                frames
                    .get(episode)
                    .is_some_and(|frames| frames.messages() == timing.messages())
            });
            if !per_frame {
                continue;
            }
            match groups
                .iter_mut()
                .find(|group| timings.of(group[0].name()) == Some(timing))
            {
                Some(group) => group.push(topic),
                None => groups.push(vec![topic]),
            }
        }
        groups
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
        let flagged = self
            .pairings
            .iter()
            .any(|pairing| pairing.flagged(self.period_ms));
        let others = self
            .pairings
            .iter()
            .map(|pairing| pairing.streams.len())
            .sum::<usize>();
        // How far apart the camera's frames and the streams logged with them
        // are stamped, and which comes first when they are apart at all.
        let (offset, order) = match first.median_ms {
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
            .filter(|pairing| pairing.flagged(self.period_ms))
            .flat_map(|pairing| pairing.apart(self.period_ms))
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
                            td { class: "is-number", "{Milliseconds::signed(pairing.median_ms)}" }
                            td { class: "is-number", "{Milliseconds::signed(pairing.low_ms)} to {Milliseconds::signed(pairing.high_ms)}" }
                            td { class: "is-number", "{pairing.episodes.len()} of {episodes}" }
                            td { EvidenceChips { evidence: pairing.furthest() } }
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
    /// The streams of `group`, logged at the same instants, as `cells`
    /// measured the first of them in each episode.
    fn read(group: &[&DatasetTopic], cells: &CheckCells<OffsetPart>) -> Self {
        let measured = cells
            .parts()
            .filter(|(_, part)| part.part == group[0].name())
            .collect::<Vec<_>>();
        Self {
            streams: group.iter().map(|topic| topic.name().to_owned()).collect(),
            median_ms: Median::of(measured.iter().map(|(_, part)| part.median_ms))
                .unwrap_or_default(),
            low_ms: measured
                .iter()
                .map(|(_, part)| part.low_ms)
                .reduce(f64::min)
                .unwrap_or_default(),
            high_ms: measured
                .iter()
                .map(|(_, part)| part.high_ms)
                .reduce(f64::max)
                .unwrap_or_default(),
            episodes: measured
                .iter()
                .map(|&(episode, part)| EpisodeOffset {
                    episode: episode.clone(),
                    median_ms: part.median_ms,
                    opening_ms: part.opening_ms,
                })
                .collect(),
        }
    }
    /// Pairings whose every episode reads alike are one set of instants.
    fn merged(pairings: Vec<Self>) -> Vec<Self> {
        let mut merged: Vec<Self> = Vec::new();
        for pairing in pairings
            .into_iter()
            .filter(|pairing| !pairing.episodes.is_empty())
        {
            match merged.iter_mut().find(|kept| kept.reads_like(&pairing)) {
                Some(kept) => kept.streams.extend(pairing.streams),
                None => merged.push(pairing),
            }
        }
        merged
    }
    fn reads_like(&self, other: &Self) -> bool {
        let offsets = |pairing: &Self| {
            let mut offsets = pairing
                .episodes
                .iter()
                .map(|episode| {
                    (
                        episode.episode.clone(),
                        (episode.median_ms * 100.0).round() as i64,
                    )
                })
                .collect::<Vec<_>>();
            offsets.sort();
            offsets
        };
        offsets(self) == offsets(other)
    }
    /// Whether the camera and these streams disagree by more than a frame,
    /// episodes disagree with each other by more than one, or an episode
    /// opens more than half a frame away from where it runs.
    fn flagged(&self, period_ms: f64) -> bool {
        let medians = self.episodes.iter().map(|episode| episode.median_ms);
        let spread = medians.clone().fold(f64::NEG_INFINITY, f64::max)
            - medians.fold(f64::INFINITY, f64::min);
        let drift = self
            .episodes
            .iter()
            .filter_map(|episode| Some((episode.opening_ms? - episode.median_ms).abs()))
            .fold(0.0, f64::max);
        self.median_ms.abs() > period_ms || spread > period_ms || drift > period_ms / 2.0
    }
    /// The episodes whose frames sit over a frame interval from their
    /// messages, or drift over half of one in their first five seconds.
    fn apart(&self, period_ms: f64) -> Vec<Evidence> {
        self.episodes
            .iter()
            .filter(|episode| {
                episode.median_ms.abs() > period_ms
                    || episode.opening_ms.is_some_and(|opening| {
                        (opening - episode.median_ms).abs() > period_ms / 2.0
                    })
            })
            .map(|episode| {
                Evidence::whole(
                    episode.episode.clone(),
                    Milliseconds::signed(episode.median_ms),
                )
            })
            .collect()
    }
    /// The three episodes whose offset is furthest from the others'.
    fn furthest(&self) -> Vec<Evidence> {
        let Some(typical) = Median::of(self.episodes.iter().map(|episode| episode.median_ms))
        else {
            return Vec::new();
        };
        let mut episodes = self.episodes.iter().collect::<Vec<_>>();
        episodes.sort_by(|left, right| {
            (right.median_ms - typical)
                .abs()
                .total_cmp(&(left.median_ms - typical).abs())
        });
        episodes
            .into_iter()
            .take(3)
            .map(|episode| {
                Evidence::whole(
                    episode.episode.clone(),
                    Milliseconds::signed(episode.median_ms),
                )
            })
            .collect()
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
    use super::super::camera_frames::FramesPart;
    use super::super::recording_rate::TimingPart;
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::curation::AnswerSource;
    use spiral_robotics_common::datasets::DatasetSelection;

    fn episode_id(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    fn episode(ordinal: u32, median_ms: f64, opening_ms: f64) -> EpisodeOffset {
        EpisodeOffset {
            episode: episode_id(ordinal),
            median_ms,
            opening_ms: Some(opening_ms),
        }
    }

    /// One part of a cell, as the answers table stores it.
    fn stored<P: serde::de::DeserializeOwned>(part: serde_json::Value) -> P {
        serde_json::from_value(part).unwrap()
    }

    #[test]
    fn streams_logged_at_the_same_instants_merge_and_offsets_past_a_frame_are_flagged() {
        let pairing = |stream: &str, offsets: &[(f64, f64)]| ClockPairing {
            streams: vec![stream.into()],
            median_ms: -99.0,
            low_ms: -188.0,
            high_ms: -14.0,
            episodes: offsets
                .iter()
                .zip(1..)
                .map(|((median, opening), ordinal)| episode(ordinal, *median, *opening))
                .collect(),
        };
        let state = pairing(
            "/left-arm-state",
            &[(-96.1, -95.0), (-105.2, -104.0), (-125.9, -122.0)],
        );
        let action = pairing(
            "/left-arm-action",
            &[(-96.1, -95.0), (-105.2, -104.0), (-125.9, -122.0)],
        );
        let other = pairing("/right-arm-state", &[(-10.0, -10.0)]);
        let merged = ClockPairing::merged(vec![state, action, other]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].streams, ["/left-arm-state", "/left-arm-action"]);
        assert!(merged[0].flagged(33.4));
        assert_eq!(merged[0].furthest()[0].measure(), "−126 ms");
        let aligned = ClockPairing {
            median_ms: 0.0,
            ..pairing("/frame", &[(0.0, 0.0), (0.2, 0.1)])
        };
        assert!(!aligned.flagged(33.3));
        assert_eq!(Milliseconds::signed(0.0), "0 ms");
        assert_eq!(Milliseconds::signed(12.4), "+12 ms");
    }

    #[test]
    fn each_repeated_stream_pairs_with_the_overview_camera_and_shared_clocks_read_once() {
        let dataset = DatasetSelection::new(vec!["dataset_folding".into()]).unwrap();
        let video = || vec!["foxglove.CompressedVideo".to_owned()];
        let topics = [
            DatasetTopic::new("/left-wrist-camera".into(), video()).with_coverage(2, Some(7)),
            DatasetTopic::new("/top-camera".into(), video()).with_coverage(2, Some(7)),
            DatasetTopic::new("/left-arm-state".into(), vec![]).with_coverage(2, Some(7)),
            DatasetTopic::new("/left-arm-action".into(), vec![]).with_coverage(2, Some(7)),
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

        let timing = |topic: &str, messages: u64| -> TimingPart {
            stored(
                json!({"part": topic, "messages": messages, "first_ns": 0, "last_ns": messages * 33_000_000}),
            )
        };
        let (_, timings) = RecordingRate::read(
            streams,
            &CheckCells::fixture(vec![
                (
                    episode_id(1),
                    vec![
                        timing("/top-camera", 3),
                        timing("/left-arm-state", 3),
                        timing("/left-arm-action", 3),
                    ],
                ),
                (
                    episode_id(2),
                    vec![
                        timing("/top-camera", 4),
                        timing("/left-arm-state", 4),
                        timing("/left-arm-action", 4),
                    ],
                ),
            ]),
        );
        let camera: FramesPart = stored(json!({
            "part": "/top-camera", "frames": 3, "span_ns": 66_000_000, "worst_gap_ns": 33_000_000,
            "worst_at": 0.033, "late": 0, "period_ns": 33_300_000.0
        }));
        let frames = CameraFrames::read(
            streams,
            &CheckCells::fixture(vec![(episode_id(1), vec![camera])]),
        );
        let offset = |topic: &str, (median, low, high): (f64, f64, f64)| -> OffsetPart {
            stored(
                json!({"part": topic, "median_ms": median, "opening_ms": null, "low_ms": low, "high_ms": high}),
            )
        };
        let cells = CheckCells::fixture(
            [(1, (-100.0, -120.0, -30.0)), (2, (-110.0, -130.0, -90.0))]
                .into_iter()
                .map(|(ordinal, measured)| {
                    (
                        episode_id(ordinal),
                        vec![
                            offset("/left-arm-state", measured),
                            offset("/left-arm-action", measured),
                        ],
                    )
                })
                .collect(),
        );
        let clock = ClockOffset::read(streams, &timings, &frames, &cells);
        assert_eq!(clock.camera.as_deref(), Some("/top-camera"));
        assert!((clock.period_ms - 33.3).abs() < 1e-9);
        // The state and action share a clock, so they read as one.
        assert_eq!(clock.pairings.len(), 1);
        let pairing = &clock.pairings[0];
        assert_eq!(pairing.streams, ["/left-arm-state", "/left-arm-action"]);
        assert_eq!(
            (pairing.median_ms, pairing.low_ms, pairing.high_ms),
            (-105.0, -130.0, -30.0)
        );
        assert_eq!(pairing.episodes.len(), 2);
        assert!(pairing.flagged(clock.period_ms));
    }
}
