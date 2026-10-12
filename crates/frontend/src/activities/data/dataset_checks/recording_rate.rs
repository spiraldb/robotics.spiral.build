//! Recording rate: how often every stream logs a message, and the episodes
//! that log a stream at another rate than the dataset's other episodes do.
//! Each episode's cell keeps, for every stream it logs more than once, how
//! many messages it logged and when the first and the last were; the
//! typical rate a departure is judged against is read over every cell. The
//! same timings are how the clock check tells which streams share a clock.
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RecordingRate {
    streams: Vec<StreamRate>,
}

/// One stream's rate in each episode that logs it more than once.
#[derive(Clone, Debug, PartialEq)]
struct StreamRate {
    topic: String,
    camera: bool,
    /// Messages per second in each episode; empty for a stream logged once
    /// per episode.
    rates: Vec<EpisodeRate>,
}

#[derive(Clone, Debug, PartialEq)]
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

/// One stream's timing in one episode, as its cell keeps it.
#[derive(Deserialize)]
pub(super) struct TimingPart {
    part: String,
    #[serde(flatten)]
    timing: StreamTiming,
}

impl RecordingRate {
    /// How far an episode's rate may stray from the dataset's typical rate
    /// for the stream, as a share of it.
    const DEPARTURE: f64 = 0.1;

    /// The column keeping each episode's timing of every stream that logs
    /// more than once in an episode; `None` when no stream does.
    pub(super) fn column(streams: CheckedStreams<'_>) -> Result<Option<CheckColumn>, String> {
        let parts = streams
            .topics()
            .iter()
            .filter(|topic| Self::repeats(topic))
            .fold(
                PartsStatement::new(&["messages", "first_ns", "last_ns"]),
                |parts, topic| {
                    parts.part(
                        topic.name(),
                        format!(
                            "SELECT id, count(*) AS messages, min(log_time) AS first_ns, max(log_time) AS last_ns FROM {} GROUP BY id",
                            streams.read(topic)
                        ),
                    )
                },
            );
        CheckColumn::of(CheckKind::Rates, parts)
    }

    /// Every stream's rate in each episode its `cells` time, and the
    /// timings themselves, by topic.
    pub(super) fn read(
        streams: CheckedStreams<'_>,
        cells: &CheckCells<TimingPart>,
    ) -> (Self, StreamTimings) {
        let mut timings = StreamTimings::default();
        for (episode, part) in cells.parts() {
            timings
                .streams
                .entry(part.part.clone())
                .or_default()
                .insert(episode.clone(), part.timing);
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
        (Self { streams: rates }, timings)
    }

    /// Whether `topic` logs more than one message per episode, so it has a
    /// rate to measure.
    pub(super) fn repeats(topic: &DatasetTopic) -> bool {
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

    #[test]
    fn rates_are_read_from_each_episodes_cell_and_once_per_episode_streams_listed() {
        use spiral_robotics_common::datasets::{DatasetSelection, DatasetTopic};
        let dataset = DatasetSelection::new(vec!["dataset_folding".into()]).unwrap();
        let topics = [
            DatasetTopic::new("/joints".into(), vec![]).with_coverage(2, Some(600)),
            DatasetTopic::new("/instruction".into(), vec![]).with_coverage(2, Some(2)),
        ];
        let streams = CheckedStreams::new(&dataset, &topics);
        let column = RecordingRate::column(streams).unwrap().unwrap();
        assert_eq!(column.kind(), CheckKind::Rates);
        let timing = |messages, last_ns| TimingPart {
            part: "/joints".into(),
            timing: StreamTiming {
                messages,
                first_ns: 0,
                last_ns,
            },
        };
        let cells = CheckCells::fixture(vec![
            (episode(1), vec![timing(301, 10_000_000_000)]),
            (episode(2), Vec::new()),
        ]);
        let (rates, timings) = RecordingRate::read(streams, &cells);
        assert_eq!(rates.streams.len(), 2);
        assert_eq!(rates.streams[0].rates.len(), 1);
        assert_eq!(rates.streams[0].rate_label(), "30.0 Hz");
        assert_eq!(rates.streams[1].rate_label(), "once per episode");
        assert_eq!(timings.of("/joints").map(BTreeMap::len), Some(1));
    }
}
