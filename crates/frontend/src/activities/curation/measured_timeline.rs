//! One episode's timeline as a preview reads it: a Score progress node's
//! progress rate at every scored frame, or a Track objects node's count of
//! objects in every sampled picture. Each entry holds its value until the
//! next one, so the timeline is a run of moments, which a Select keeps
//! stretches of exactly as its SQL does (`select/step.rs`).
use crate::activities::episode::Episode;
use arrow_array::{Array, Float64Array, Int64Array};
use arrow_schema::DataType;
use std::io::Cursor;

/// One entry: its time, in seconds from the episode's first message, and
/// its value, which a frame the scorer could not read has none of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) struct TimelineEntry {
    time: f64,
    value: Option<f64>,
}

impl TimelineEntry {
    pub(in crate::activities::curation) fn new(time: f64, value: Option<f64>) -> Self {
        Self {
            time,
            value: value.filter(|value| value.is_finite()),
        }
    }
}

/// One entry's moment: from its time to the next entry's, holding its value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) struct TimelineMoment {
    start: f64,
    end: f64,
    value: Option<f64>,
}

impl TimelineMoment {
    pub(in crate::activities::curation) fn start(&self) -> f64 {
        self.start
    }
    pub(in crate::activities::curation) fn end(&self) -> f64 {
        self.end
    }
    pub(in crate::activities::curation) fn value(&self) -> Option<f64> {
        self.value
    }
}

/// One episode's entries, in time order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct MeasuredTimeline(Vec<TimelineEntry>);

impl MeasuredTimeline {
    /// The most entries one episode's preview reads: every frame of five and
    /// a half minutes at 30 Hz, the Arrow episode query's own bound. A longer
    /// recording is refused rather than cut short.
    pub(in crate::activities::curation) const MAX_ENTRIES: usize = 10_000;

    pub(in crate::activities::curation) fn new(mut entries: Vec<TimelineEntry>) -> Self {
        entries.sort_by(|left, right| left.time.total_cmp(&right.time));
        Self(entries)
    }

    /// One episode's timeline from the Arrow stream its preview answered:
    /// `timestamp_ns` in the recording's clock, which the browser reads from
    /// the episode's first message, and `value`.
    pub(in crate::activities::curation) fn read(
        stream: &[u8],
        episode: &Episode,
    ) -> Result<Self, String> {
        let origin = episode
            .facts()
            .message_start_time()
            .and_then(|start| i64::try_from(start).ok())
            .ok_or("This episode has no recorded start time to place its timeline by.")?;
        let reader = arrow_ipc::reader::StreamReader::try_new(Cursor::new(stream), None)
            .map_err(|error| error.to_string())?;
        let mut entries = Vec::new();
        for batch in reader {
            let batch = batch.map_err(|error| error.to_string())?;
            let column = |name: &str, kind: &DataType| {
                let column = batch
                    .column_by_name(name)
                    .ok_or_else(|| format!("The preview has no {name} column."))?;
                arrow_cast::cast(column, kind).map_err(|error| error.to_string())
            };
            let (times, values) = (
                column("timestamp_ns", &DataType::Int64)?,
                column("value", &DataType::Float64)?,
            );
            let (Some(times), Some(values)) = (
                times.as_any().downcast_ref::<Int64Array>(),
                values.as_any().downcast_ref::<Float64Array>(),
            ) else {
                return Err("The preview's columns are not a timeline's types.".into());
            };
            for row in 0..batch.num_rows() {
                if times.is_null(row) {
                    return Err("An entry of the preview has no time.".into());
                }
                entries.push(TimelineEntry::new(
                    times.value(row).saturating_sub(origin) as f64 / 1e9,
                    (!values.is_null(row)).then(|| values.value(row)),
                ));
            }
        }
        Ok(Self::new(entries))
    }

    pub(in crate::activities::curation) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every entry's moment, in time order: it lasts until the next entry,
    /// and the last as long as the one before it. A lone entry has none.
    pub(in crate::activities::curation) fn moments(&self) -> Vec<TimelineMoment> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(place, entry)| {
                let end = match self.0.get(place + 1) {
                    Some(next) => next.time,
                    None => entry.time + (entry.time - self.0.get(place.checked_sub(1)?)?.time),
                };
                Some(TimelineMoment {
                    start: entry.time,
                    end,
                    value: entry.value,
                })
            })
            .collect()
    }

    /// The stretches of moments that `passes` keeps, a moment of no length
    /// never among them, each joined with the passing moments right after
    /// it.
    pub(in crate::activities::curation) fn stretches(
        &self,
        passes: impl Fn(Option<f64>) -> bool,
    ) -> Vec<(f64, f64)> {
        let mut stretches = Vec::<(f64, f64)>::new();
        let mut follows = false;
        for moment in self.moments() {
            let kept = moment.end > moment.start && passes(moment.value);
            match stretches.last_mut() {
                Some(last) if kept && follows => last.1 = last.1.max(moment.end),
                _ if kept => stretches.push((moment.start, moment.end)),
                _ => {}
            }
            follows = kept;
        }
        stretches
    }

    /// The largest value any entry holds.
    pub(in crate::activities::curation) fn most(&self) -> Option<f64> {
        self.0
            .iter()
            .filter_map(|entry| entry.value)
            .reduce(f64::max)
    }

    /// The mean of the values the entries within `span` hold, of those that
    /// hold one.
    pub(in crate::activities::curation) fn mean_within(
        &self,
        (start, end): (f64, f64),
    ) -> Option<f64> {
        let values = self
            .0
            .iter()
            .filter(|entry| entry.time >= start && entry.time < end)
            .filter_map(|entry| entry.value)
            .collect::<Vec<_>>();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, RecordBatch};
    use arrow_schema::{Field, Schema};
    use spiral_robotics_common::datasets::EpisodeId;
    use std::sync::Arc;

    fn timeline(entries: &[(f64, Option<f64>)]) -> MeasuredTimeline {
        MeasuredTimeline::new(
            entries
                .iter()
                .map(|(time, value)| TimelineEntry::new(*time, *value))
                .collect(),
        )
    }

    #[test]
    fn a_moment_lasts_until_the_next_entry_and_the_last_as_long_as_the_one_before() {
        let moments = timeline(&[(0.0, Some(0.5)), (0.5, None), (1.5, Some(0.1))]).moments();
        assert_eq!(
            moments
                .iter()
                .map(|moment| (moment.start(), moment.end(), moment.value()))
                .collect::<Vec<_>>(),
            [
                (0.0, 0.5, Some(0.5)),
                (0.5, 1.5, None),
                (1.5, 2.5, Some(0.1))
            ]
        );
        assert!(timeline(&[(3.0, Some(1.0))]).moments().is_empty());
    }

    #[test]
    fn passing_moments_join_into_stretches() {
        let rates = timeline(&[
            (0.0, Some(0.5)),
            (1.0, None),
            (2.0, Some(0.1)),
            (3.0, Some(0.3)),
            (4.0, Some(0.4)),
        ]);
        let progressing = |rate: Option<f64>| rate.is_none_or(|rate| rate >= 0.2);
        assert_eq!(rates.stretches(progressing), [(0.0, 2.0), (3.0, 5.0)]);
        assert_eq!(rates.most(), Some(0.5));
        assert_eq!(rates.mean_within((0.0, 3.0)), Some(0.3));
    }

    #[test]
    fn a_preview_reads_back_from_the_episodes_first_message() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("timestamp_ns", DataType::Int64, false),
            Field::new("value", DataType::UInt64, true),
        ]));
        let start = 1_000_000_000_i64;
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(vec![start + 200_000_000, start])),
            Arc::new(arrow_array::UInt64Array::from(vec![Some(2), None])),
        ];
        let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
        let mut bytes = Vec::new();
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        drop(writer);
        // The fixture episode's first message is at one second of log time.
        let episode = Episode::fixture(&EpisodeId::fixture(1), 2_000_000_000);
        assert_eq!(
            MeasuredTimeline::read(&bytes, &episode).unwrap(),
            timeline(&[(0.0, None), (0.2, Some(2.0))])
        );
    }
}
