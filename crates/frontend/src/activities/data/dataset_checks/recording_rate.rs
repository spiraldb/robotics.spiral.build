//! Recording rate: how often every stream logs a message, and the episodes
//! that log a stream at another rate than the dataset's other episodes do.
//! Each episode's cell keeps, for every stream it logs more than once, how
//! many messages it logged and when the first and the last were. The report
//! judges them in Spiral: each episode's rate, the stream's typical rate —
//! the median over its episodes — and the episodes that stray from it.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, StreamName, Verdict};
use super::stream_reads::CheckedStreams;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeId};
use spiral_robotics_common::sql::SqlStruct;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RecordingRate {
    streams: Vec<StreamRate>,
}

/// One stream's typical rate, and the episodes that log it at another.
#[derive(Clone, Debug, PartialEq)]
struct StreamRate {
    topic: String,
    camera: bool,
    /// Messages per second in its usual episode; none for a stream logged
    /// once per episode.
    typical: Option<f64>,
    departures: Vec<EpisodeRate>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeRate {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    hz: f64,
}

/// One stream as the judgment answers it: its typical rate and the episodes
/// that depart from it. A stream no episode departs from answers one empty
/// departure, which reads as none.
#[derive(Deserialize)]
struct JudgedStream {
    part: String,
    typical: Option<f64>,
    departures: Option<Vec<JudgedDeparture>>,
}

#[derive(Deserialize)]
struct JudgedDeparture {
    episode_id: Option<EpisodeId>,
    hz: Option<f64>,
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

    /// Every stream's typical rate and the episodes that depart from it,
    /// judged in Spiral over the column among `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        streams: CheckedStreams<'_>,
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Rates) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(streams, judged))
    }

    /// The judgment over `parts`, every episode's timings: per stream, the
    /// median of the episodes' rates and the episodes whose rate strays from
    /// it by more than the departure share.
    fn judgment(parts: &str) -> String {
        format!(
            "WITH r AS (SELECT episode_id, part, (CAST(messages AS DOUBLE) - 1) / ((CAST(last_ns AS DOUBLE) - CAST(first_ns AS DOUBLE)) / 1e9) AS hz \
               FROM {parts} p WHERE last_ns > first_ns), \
             t AS (SELECT part, approx_percentile_cont(hz, 0.5) AS typical FROM r GROUP BY part), \
             d AS (SELECT r.part AS part, r.episode_id AS episode_id, r.hz AS hz FROM r JOIN t ON t.part = r.part \
               WHERE abs(r.hz / t.typical - 1) > {departure}) \
             SELECT t.part AS part, t.typical AS typical, {departures} AS departures \
             FROM t LEFT JOIN d ON d.part = t.part GROUP BY t.part, t.typical",
            departure = Self::DEPARTURE,
            departures = SqlStruct::new()
                .field("episode_id", "d.episode_id")
                .field("hz", "d.hz")
                .collected(),
        )
    }

    /// Every stream `streams` declares, as `judged` says it logs; a stream
    /// the judgment does not name is logged once per episode.
    fn of(streams: CheckedStreams<'_>, judged: Vec<JudgedStream>) -> Self {
        let rates = streams
            .topics()
            .iter()
            .map(|topic| {
                let judged = judged.iter().find(|stream| stream.part == topic.name());
                StreamRate {
                    topic: topic.name().to_owned(),
                    camera: topic.is_video(),
                    typical: judged.and_then(|stream| stream.typical),
                    departures: judged
                        .and_then(|stream| stream.departures.as_ref())
                        .into_iter()
                        .flatten()
                        .filter_map(|departure| {
                            Some(EpisodeRate {
                                episode: departure.episode_id.clone()?,
                                hz: departure.hz?,
                            })
                        })
                        .collect(),
                }
            })
            .collect();
        Self { streams: rates }
    }

    /// Whether `topic` logs more than one message per episode, so it has a
    /// rate to measure.
    pub(super) fn repeats(topic: &DatasetTopic) -> bool {
        topic.samples() > topic.members()
    }

    pub(super) fn row(&self) -> CheckRow {
        let departing = self
            .streams
            .iter()
            .flat_map(|stream| &stream.departures)
            .map(|rate| &rate.episode)
            .collect::<std::collections::BTreeSet<_>>();
        let streams = self
            .streams
            .iter()
            .filter(|stream| !stream.departures.is_empty())
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
        rows.sort_by_key(|stream| (stream.departures.is_empty(), !stream.camera));
        let flagged = self
            .streams
            .iter()
            .flat_map(StreamRate::evidence)
            .collect::<Vec<_>>();
        CheckRow::new("Recording rate", verdict, finding).flagging(flagged).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Stream rates",
                thead { tr { th { "Stream" } th { class: "is-number", "Rate" } th { "Episodes at another rate" } } }
                tbody {
                    for stream in rows {
                        tr { key: "{stream.topic}",
                            td { StreamName { topic: stream.topic.clone(), camera: stream.camera } }
                            td { class: "is-number", "{stream.rate_label()}" }
                            td { EvidenceChips { evidence: stream.evidence() } }
                        }
                    }
                }
            }
        })
    }
}

impl StreamRate {
    /// The episodes at another rate, each as the finding names it.
    fn evidence(&self) -> Vec<Evidence> {
        self.departures
            .iter()
            .map(|rate| Evidence::whole(rate.episode.clone(), format!("{:.1} Hz", rate.hz)))
            .collect()
    }
    fn rate_label(&self) -> String {
        match self.typical {
            Some(hz) => format!("{hz:.1} Hz"),
            None => "once per episode".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::data::dataset_checks::stream_reads::TwoArmDataset;
    use spiral_robotics_common::datasets::DatasetSelection;

    #[test]
    fn the_judgment_finds_each_streams_typical_rate_and_its_departures_in_spiral() {
        let sql = RecordingRate::judgment("(parts)");
        assert!(
            sql.contains("FROM (parts) p WHERE last_ns > first_ns"),
            "{sql}"
        );
        assert!(
            sql.contains("approx_percentile_cont(hz, 0.5) AS typical"),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE abs(r.hz / t.typical - 1) > 0.1"),
            "{sql}"
        );
        assert!(
            sql.contains(
                "array_agg(named_struct('episode_id', d.episode_id, 'hz', d.hz)) AS departures"
            ),
            "{sql}"
        );
        assert!(
            sql.contains("FROM t LEFT JOIN d ON d.part = t.part GROUP BY t.part, t.typical"),
            "{sql}"
        );
    }

    #[test]
    fn every_declared_stream_is_listed_as_judged_and_once_per_episode_streams_too() {
        let dataset = DatasetSelection::new("folding").unwrap();
        let topics = [
            TwoArmDataset::stream("/joints", "JointState").with_coverage(2, 600),
            TwoArmDataset::stream("/instruction", "Instruction").with_coverage(2, 2),
        ];
        let streams = CheckedStreams::new(&dataset, &topics);
        let column = RecordingRate::column(streams).unwrap().unwrap();
        assert_eq!(column.kind(), CheckKind::Rates);
        let judged: Vec<JudgedStream> = serde_json::from_value(serde_json::json!([{
            "part": "/joints", "typical": 29.8,
            "departures": [{"episode_id": EpisodeId::fixture(4), "hz": 281.9}]
        }]))
        .unwrap();
        let rates = RecordingRate::of(streams, judged);
        assert_eq!(rates.streams[0].rate_label(), "29.8 Hz");
        assert_eq!(rates.streams[0].departures.len(), 1);
        assert_eq!(rates.streams[1].rate_label(), "once per episode");
        // A stream no episode departs from answers one empty departure.
        let quiet: Vec<JudgedStream> = serde_json::from_value(serde_json::json!([{
            "part": "/joints", "typical": 29.8,
            "departures": [{"episode_id": null, "hz": null}]
        }]))
        .unwrap();
        assert!(
            RecordingRate::of(streams, quiet).streams[0]
                .departures
                .is_empty()
        );
    }
}
