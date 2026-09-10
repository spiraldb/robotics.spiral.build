/// Bound wall-clock gaps and clamp the final frame to the episode duration.
pub(crate) struct PreviewClock {
    duration: f64,
    previous_tick: Option<f64>,
}

impl PreviewClock {
    pub(crate) fn new(duration: f64, previous_tick: Option<f64>) -> Self {
        Self {
            duration,
            previous_tick,
        }
    }
    pub(crate) fn advance(&self, playhead: f64, now: f64) -> f64 {
        let delta = self
            .previous_tick
            .map(|previous| (now - previous).clamp(0.0, 0.25))
            .unwrap_or_default();
        (playhead + delta).clamp(0.0, self.duration.max(0.0))
    }
}
