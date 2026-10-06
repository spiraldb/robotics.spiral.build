//! Camera clock offset: in episodes where a stream logs exactly one message
//! per frame of the camera that shows the workspace best, frame n and
//! message n record the same instant, so their timestamps should agree. The
//! offset between them is measured per episode, over the whole episode and
//! over its first five seconds, which tells a constant offset from a drift.
//! Streams logged at the same instants read alike, so they are measured once.
use super::camera_frames::CameraFrames;
use super::recording_rate::{Median, StreamTimings};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use crate::activities::episode::StreamCatalog;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ClockOffset {
    /// The camera every stream is paired with; `None` without a camera.
    camera: Option<String>,
    period_ms: f64,
    pairings: Vec<ClockPairing>,
}

/// The streams logged at one set of instants, paired with the camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ClockPairing {
    streams: Vec<String>,
    /// Frame time minus message time, over every pair of every episode.
    median_ms: f64,
    low_ms: f64,
    high_ms: f64,
    episodes: Vec<EpisodeOffset>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeOffset {
    #[serde(rename = "id")]
    episode: EpisodeId,
    median_ms: f64,
    /// Over the episode's first five seconds.
    opening_ms: Option<f64>,
}

#[derive(Deserialize)]
struct OffsetRow {
    pooled_ms: f64,
    low_ms: f64,
    high_ms: f64,
    #[serde(flatten)]
    episode: EpisodeOffset,
}

impl ClockOffset {
    /// The stretch at the start of an episode compared with the whole of it.
    const OPENING_NS: u64 = 5_000_000_000;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        timings: &StreamTimings,
        frames: &CameraFrames,
    ) -> Result<Self, String> {
        let cameras = streams
            .topics()
            .iter()
            .filter(|topic| topic.is_video())
            .map(|topic| topic.name().to_owned())
            .collect::<Vec<_>>();
        let Some(camera) = StreamCatalog::overview_camera(&cameras).map(str::to_owned) else {
            return Ok(Self {
                camera: None,
                period_ms: 0.0,
                pairings: Vec::new(),
            });
        };
        let period_ms = frames.period_ns(&camera).unwrap_or_default() / 1e6;
        let groups = Self::groups(streams, timings, &camera);
        let camera_topic = streams
            .topics()
            .iter()
            .find(|topic| topic.name() == camera)
            .ok_or("The camera is no longer in the dataset")?;
        let mut pairings = groups
            .iter()
            .map(|group| ClockPairing {
                streams: group.iter().map(|topic| topic.name().to_owned()).collect(),
                median_ms: 0.0,
                low_ms: 0.0,
                high_ms: 0.0,
                episodes: Vec::new(),
            })
            .collect::<Vec<_>>();
        let parts = groups
            .iter()
            .map(|group| Self::sql(streams, (camera_topic, group[0])))
            .collect::<Vec<_>>();
        for (part, row) in streams.parts::<OffsetRow>(&parts).await? {
            let pairing = &mut pairings[part];
            (pairing.median_ms, pairing.low_ms, pairing.high_ms) =
                (row.pooled_ms, row.low_ms, row.high_ms);
            pairing.episodes.push(row.episode);
        }
        Ok(Self {
            camera: Some(camera),
            period_ms,
            pairings: ClockPairing::merged(pairings),
        })
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
               FROM c JOIN s ON s.id = c.id AND s.i = c.i WHERE c.n = s.n), \
             d AS (SELECT spql.agg.approx_quantile(offset_ms, 0.5) AS pooled_ms, min(offset_ms) AS low_ms, max(offset_ms) AS high_ms FROM p), \
             r AS (SELECT id, spql.agg.approx_quantile(offset_ms, 0.5) AS median_ms, \
               spql.agg.approx_quantile(CASE WHEN since_ns < {opening} THEN offset_ms END, 0.5) AS opening_ms FROM p GROUP BY id) \
             SELECT r.id AS id, r.median_ms AS median_ms, r.opening_ms AS opening_ms, d.pooled_ms AS pooled_ms, d.low_ms AS low_ms, d.high_ms AS high_ms \
             FROM r CROSS JOIN d",
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
    use super::*;

    fn episode(ordinal: u32, median_ms: f64, opening_ms: f64) -> EpisodeOffset {
        EpisodeOffset {
            episode: EpisodeId::try_from(format!("{ordinal:032x}")).unwrap(),
            median_ms,
            opening_ms: Some(opening_ms),
        }
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
}
