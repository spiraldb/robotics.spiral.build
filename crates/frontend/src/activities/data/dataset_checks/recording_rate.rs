//! Recording rate: how often every stream logs a message, and the episodes
//! that log a stream at another rate than the dataset's other episodes do.
//! The same read says when each stream's first and last message were
//! logged, which is how the clock check tells which streams share a clock.
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct RecordingRate {
    streams: Vec<StreamRate>,
}

/// One stream's rate in each episode that logs it more than once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StreamRate {
    topic: String,
    camera: bool,
    /// Messages per second in each episode; empty for a stream logged once
    /// per episode.
    rates: Vec<EpisodeRate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeRate {
    episode: EpisodeId,
    hz: f64,
}

/// How many messages one stream logged in one episode, and when the first
/// and the last were logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct StreamTiming {
    messages: u64,
    first_ns: u64,
    last_ns: u64,
}

impl StreamTiming {
    pub(super) fn messages(&self) -> u64 {
        self.messages
    }
    fn hz(&self) -> Option<f64> {
        let span = self.last_ns.checked_sub(self.first_ns)? as f64 / 1e9;
        (span > 0.0).then(|| (self.messages - 1) as f64 / span)
    }
}

/// Every repeated stream's timing in each episode, by topic: what a rate is
/// measured from, and what tells two streams logged on one clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct StreamTimings {
    streams: BTreeMap<String, BTreeMap<EpisodeId, StreamTiming>>,
}

impl StreamTimings {
    pub(super) fn of(&self, topic: &str) -> Option<&BTreeMap<EpisodeId, StreamTiming>> {
        self.streams.get(topic)
    }
}

#[derive(Deserialize)]
struct TimingRow {
    id: EpisodeId,
    #[serde(flatten)]
    timing: StreamTiming,
}

impl RecordingRate {
    /// How far an episode's rate may stray from the dataset's typical rate
    /// for the stream, as a share of it.
    const DEPARTURE: f64 = 0.1;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
    ) -> Result<(Self, StreamTimings), String> {
        let repeated = streams
            .topics()
            .iter()
            .filter(|topic| Self::repeats(topic))
            .collect::<Vec<_>>();
        let mut timings = StreamTimings::default();
        for (part, row) in streams.parts::<TimingRow>(|streams| Ok(repeated
            .iter()
            .map(|topic| {
                format!(
                    "SELECT id, count(*) AS messages, min(log_time) AS first_ns, max(log_time) AS last_ns FROM {} GROUP BY id",
                    streams.read(topic)
                )
            })
            .collect::<Vec<_>>())).await? {
            timings
                .streams
                .entry(repeated[part].name().to_owned())
                .or_default()
                .insert(row.id, row.timing);
        }
        let rates = streams
            .topics()
            .iter()
            .map(|topic| StreamRate {
                topic: topic.name().to_owned(),
                camera: topic.is_video(),
                rates: timings
                    .of(topic.name())
                    .into_iter()
                    .flatten()
                    .filter_map(|(episode, timing)| {
                        Some(EpisodeRate {
                            episode: episode.clone(),
                            hz: timing.hz()?,
                        })
                    })
                    .collect(),
            })
            .collect();
        Ok((Self { streams: rates }, timings))
    }

    /// Whether `topic` logs more than one message per episode, so it has a
    /// rate to measure.
    fn repeats(topic: &DatasetTopic) -> bool {
        topic
            .samples()
            .is_none_or(|samples| samples > topic.members())
    }

    pub(super) fn row(&self) -> CheckRow {
        let departing = self
            .streams
            .iter()
            .flat_map(StreamRate::departures)
            .map(|rate| &rate.episode)
            .collect::<std::collections::BTreeSet<_>>();
        let streams = self
            .streams
            .iter()
            .filter(|stream| !stream.departures().is_empty())
            .count();
        let (verdict, finding) = match departing.len() {
            0 => (
                Verdict::Clean,
                Finding::of_count(self.streams.len(), "stream", "at one rate in every episode"),
            ),
            episodes => (
                Verdict::Flagged,
                Finding::of_count(streams, "stream", "at another rate in")
                    .and_count(episodes, "episode", ""),
            ),
        };
        // The streams that depart lead the ledger, then cameras, then the rest.
        let mut rows = self.streams.clone();
        rows.sort_by_key(|stream| (stream.departures().is_empty(), !stream.camera));
        let flagged = self
            .streams
            .iter()
            .flat_map(StreamRate::departures)
            .map(|rate| Evidence::whole(rate.episode.clone(), format!("{:.1} Hz", rate.hz)))
            .collect::<Vec<_>>();
        CheckRow::new("Recording rate", verdict, finding).flagging(flagged).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Stream rates",
                thead { tr { th { "Stream" } th { class: "is-number", "Rate" } th { "Episodes at another rate" } } }
                tbody {
                    for stream in rows {
                        tr { key: "{stream.topic}",
                            td { StreamName { topic: stream.topic.clone(), camera: stream.camera } }
                            td { class: "is-number", "{stream.rate_label()}" }
                            td {
                                EvidenceChips {
                                    evidence: stream.departures().into_iter().map(|rate| Evidence::whole(rate.episode.clone(), format!("{:.1} Hz", rate.hz))).collect::<Vec<_>>(),
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

impl StreamRate {
    /// The median of the episodes' rates.
    fn typical(&self) -> Option<f64> {
        Median::of(self.rates.iter().map(|rate| rate.hz))
    }
    fn departures(&self) -> Vec<&EpisodeRate> {
        let Some(typical) = self.typical() else {
            return Vec::new();
        };
        self.rates
            .iter()
            .filter(|rate| (rate.hz / typical - 1.0).abs() > RecordingRate::DEPARTURE)
            .collect()
    }
    fn rate_label(&self) -> String {
        match self.typical() {
            Some(hz) => format!("{hz:.1} Hz"),
            None => "once per episode".into(),
        }
    }
}

/// The middle of some measurements.
pub(super) struct Median;

impl Median {
    pub(super) fn of(values: impl IntoIterator<Item = f64>) -> Option<f64> {
        let mut values = values
            .into_iter()
            .filter(|value| value.is_finite())
            .collect::<Vec<_>>();
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        Some(if values.len() % 2 == 0 {
            (values[middle - 1] + values[middle]) / 2.0
        } else {
            values[middle]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    #[test]
    fn an_episode_is_flagged_when_it_logs_a_stream_far_from_the_typical_rate() {
        let stream = StreamRate {
            topic: "/left-arm-state".into(),
            camera: false,
            rates: [29.8, 29.7, 29.9, 281.9, 29.8]
                .into_iter()
                .zip(1..)
                .map(|(hz, ordinal)| EpisodeRate {
                    episode: episode(ordinal),
                    hz,
                })
                .collect(),
        };
        assert_eq!(stream.typical(), Some(29.8));
        assert_eq!(stream.rate_label(), "29.8 Hz");
        assert_eq!(
            stream
                .departures()
                .iter()
                .map(|rate| rate.episode.clone())
                .collect::<Vec<_>>(),
            [episode(4)]
        );
        let once = StreamRate {
            topic: "/instruction".into(),
            camera: false,
            rates: Vec::new(),
        };
        assert_eq!(once.rate_label(), "once per episode");
        assert!(once.departures().is_empty());
        let timing = StreamTiming {
            messages: 301,
            first_ns: 1_000_000_000,
            last_ns: 11_000_000_000,
        };
        assert_eq!(timing.hz(), Some(30.0));
        assert_eq!(Median::of([3.0, 1.0, 2.0, 10.0]), Some(2.5));
    }
}
