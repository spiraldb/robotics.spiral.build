//! Camera timestamps use the exact active interval published by the control loop.
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct CaptureClock {
    active: Arc<RwLock<Option<CaptureInterval>>>,
}

struct CaptureInterval {
    started: Instant,
    elapsed: Duration,
}
impl CaptureClock {
    pub fn update(&self, now: Instant, elapsed: Option<Duration>) {
        *self
            .active
            .write()
            .unwrap_or_else(|error| error.into_inner()) = elapsed.map(|elapsed| CaptureInterval {
            started: now,
            elapsed,
        });
    }
    pub fn timestamp_ns(&self, captured: Instant) -> Option<u64> {
        let interval = self
            .active
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let interval = interval.as_ref()?;
        let elapsed = interval
            .elapsed
            .checked_add(captured.checked_duration_since(interval.started)?)?;
        u64::try_from(elapsed.as_nanos()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_time_excludes_pauses_and_rejects_stale_frames() {
        let clock = CaptureClock::default();
        let start = Instant::now();
        assert_eq!(clock.timestamp_ns(start), None);
        clock.update(start, Some(Duration::ZERO));
        assert_eq!(
            clock.timestamp_ns(start + Duration::from_millis(25)),
            Some(25_000_000)
        );
        clock.update(start + Duration::from_millis(50), None);
        assert_eq!(clock.timestamp_ns(start + Duration::from_secs(1)), None);
        let resumed = start + Duration::from_secs(2);
        clock.update(resumed, Some(Duration::from_millis(50)));
        assert_eq!(clock.timestamp_ns(start), None);
        assert_eq!(
            clock.timestamp_ns(resumed + Duration::from_millis(25)),
            Some(75_000_000)
        );
    }
}
