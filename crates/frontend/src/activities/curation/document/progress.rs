//! One episode's progress as the scorer read it, several samples a second.
//!
//! A detection keeps each second as a moment, because a second is the least
//! of a recording worth keeping or leaving out; a Select decides by those.
//! Drawn from the moments alone, the recording's progress was a line of
//! straight strokes between seconds. The curve is what the timeline draws
//! instead: finer, and only ever shown.
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::EpisodeId;

/// Samples are kept to three decimals; the scorer's own precision past that
/// is noise, and every digit is stored for every sample of every episode.
const KEPT_DECIMALS: f64 = 1_000.0;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProgressCurve {
    episode: EpisodeId,
    /// When the first sample was read, in seconds from the episode's start.
    start_time: f64,
    /// Seconds between samples.
    period: f64,
    /// One mean progress per `period`; `None` where the scorer could not read
    /// the frames.
    values: Vec<Option<f64>>,
}

impl ProgressCurve {
    /// A curve whose first sample was read at `start_time` and the rest
    /// every `period` seconds after it.
    pub(crate) fn new(
        episode: EpisodeId,
        (start_time, period): (f64, f64),
        values: Vec<Option<f64>>,
    ) -> Result<Self, String> {
        if !start_time.is_finite() || start_time < 0.0 || !period.is_finite() || period <= 0.0 {
            return Err("A progress curve needs a start and a positive period.".into());
        }
        Ok(Self {
            episode,
            start_time,
            period,
            values: values
                .into_iter()
                .map(|value| {
                    value
                        .filter(|value| value.is_finite())
                        .map(|value| (value * KEPT_DECIMALS).round() / KEPT_DECIMALS)
                })
                .collect(),
        })
    }
    pub(crate) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    /// Every sample as its time and value, `None` where it was not read.
    pub(crate) fn samples(&self) -> impl Iterator<Item = (f64, Option<f64>)> + '_ {
        self.values
            .iter()
            .enumerate()
            .map(|(index, value)| (self.start_time + index as f64 * self.period, *value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_keeps_its_timing_and_three_decimals() {
        let curve = ProgressCurve::new(
            EpisodeId::fixture(1),
            (0.1, 0.2),
            vec![Some(0.123_456), None, Some(f64::NAN), Some(-1.0)],
        )
        .unwrap();
        let samples = curve.samples().collect::<Vec<_>>();
        assert_eq!(samples[0], (0.1, Some(0.123)));
        assert_eq!(samples[1].1, None);
        assert_eq!(
            samples[2].1, None,
            "a value the scorer could not read is unread"
        );
        assert!((samples[3].0 - 0.7).abs() < 1e-9);
        assert!(ProgressCurve::new(EpisodeId::fixture(1), (0.0, 0.0), Vec::new()).is_err());
    }
}
