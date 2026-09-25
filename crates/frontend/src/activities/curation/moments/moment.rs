//! One scored frame of one episode, as the preview reads it back.
use super::scoring::ProgressScorer;
use crate::activities::episode::Episode;
use arrow_array::{Array, Float64Array, Int64Array};
use arrow_schema::DataType;
use spiral_robotics_common::datasets::EpisodeId;
use std::io::Cursor;

/// One moment: the stretch of recording one scored frame stands for, from its
/// own time to the next frame's, in seconds from the episode's first message,
/// and the pace the scorer read there. `pace` is `None` where the scorer had
/// no support for the frame.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct Moment {
    episode: EpisodeId,
    start_time: f64,
    end_time: f64,
    pace: Option<f64>,
}

impl Moment {
    /// The column a moment's pace travels in, on every edge below the node.
    pub(in crate::activities::curation) const PACE: &'static str = "pace";

    pub(in crate::activities::curation) fn new(
        episode: EpisodeId,
        (start_time, end_time): (f64, f64),
        pace: Option<f64>,
    ) -> Result<Self, String> {
        if !start_time.is_finite()
            || !end_time.is_finite()
            || start_time < 0.0
            || end_time <= start_time
        {
            return Err(format!(
                "Moment [{start_time}, {end_time}) is not a valid time range."
            ));
        }
        Ok(Self {
            episode,
            start_time,
            end_time,
            pace: pace.filter(|value| value.is_finite()),
        })
    }
    pub(in crate::activities::curation) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(in crate::activities::curation) fn start_time(&self) -> f64 {
        self.start_time
    }
    pub(in crate::activities::curation) fn end_time(&self) -> f64 {
        self.end_time
    }
    pub(in crate::activities::curation) fn pace(&self) -> Option<f64> {
        self.pace
    }

    /// One episode's moments from the Arrow stream its preview answered, in
    /// time order. The relation's times are the recording's clock; the
    /// browser reads them from the episode's first message.
    pub(in crate::activities::curation) fn read(
        stream: &[u8],
        episode: &Episode,
    ) -> Result<Vec<Self>, String> {
        let origin = episode
            .facts()
            .message_start_time()
            .and_then(|start| i64::try_from(start).ok())
            .ok_or("This episode has no recorded start time to place its moments by.")?;
        let seconds = |nanos: i64| nanos.saturating_sub(origin) as f64 / 1e9;
        let reader = arrow_ipc::reader::StreamReader::try_new(Cursor::new(stream), None)
            .map_err(|error| error.to_string())?;
        let mut moments = Vec::new();
        for batch in reader {
            let batch = batch.map_err(|error| error.to_string())?;
            let column = |name: &str, kind: &DataType| {
                let column = batch
                    .column_by_name(name)
                    .ok_or_else(|| format!("The preview has no {name} column."))?;
                arrow_cast::cast(column, kind).map_err(|error| error.to_string())
            };
            let (starts, ends, paces) = (
                column("start_ns", &DataType::Int64)?,
                column("end_ns", &DataType::Int64)?,
                column(Self::PACE, &DataType::Float64)?,
            );
            let (Some(starts), Some(ends), Some(paces)) = (
                starts.as_any().downcast_ref::<Int64Array>(),
                ends.as_any().downcast_ref::<Int64Array>(),
                paces.as_any().downcast_ref::<Float64Array>(),
            ) else {
                return Err("The preview's columns are not the moments' types.".into());
            };
            for row in 0..batch.num_rows() {
                if starts.is_null(row) || ends.is_null(row) {
                    return Err("A moment of the preview has no interval.".into());
                }
                moments.push(Self::new(
                    episode.id().clone(),
                    (seconds(starts.value(row)), seconds(ends.value(row))),
                    (!paces.is_null(row)).then(|| paces.value(row)),
                )?);
            }
        }
        if moments.is_empty() {
            return Err(format!(
                "This recording has no {} frames to score.",
                ProgressScorer::WarpSimBottles.camera()
            ));
        }
        moments.sort_by(|left, right| left.start_time.total_cmp(&right.start_time));
        Ok(moments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, RecordBatch};
    use arrow_schema::{Field, Schema};
    use std::sync::Arc;

    fn stream(rows: &[(i64, i64, Option<f64>)]) -> Vec<u8> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, false),
            Field::new("start_ns", DataType::Int64, false),
            Field::new("end_ns", DataType::Int64, false),
            Field::new(Moment::PACE, DataType::Float64, true),
        ]));
        let columns: Vec<ArrayRef> = vec![
            Arc::new(arrow_array::StringArray::from(vec!["e"; rows.len()])),
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|row| row.0))),
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|row| row.1))),
            Arc::new(Float64Array::from_iter(rows.iter().map(|row| row.2))),
        ];
        let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
        let mut bytes = Vec::new();
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        drop(writer);
        bytes
    }

    #[test]
    fn a_preview_reads_back_as_moments_from_the_episodes_first_message() {
        // The fixture episode's first message is at one second of log time.
        let episode = Episode::fixture(&EpisodeId::fixture(1), 2_000_000_000);
        let start = 1_000_000_000;
        let bytes = stream(&[
            (start + 33_333_333, start + 66_666_667, None),
            (start, start + 33_333_333, Some(0.4)),
        ]);
        let moments = Moment::read(&bytes, &episode).unwrap();
        assert_eq!(moments.len(), 2);
        assert_eq!(moments[0].start_time(), 0.0);
        assert_eq!(moments[0].end_time(), moments[1].start_time());
        assert_eq!(moments[0].pace(), Some(0.4));
        assert_eq!(moments[1].pace(), None, "an unsupported frame is unread");
        assert!(
            Moment::read(&stream(&[]), &episode)
                .unwrap_err()
                .contains("no /cameras/top_camera-images-rgb frames")
        );
    }
}
