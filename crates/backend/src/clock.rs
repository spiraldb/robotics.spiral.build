//! Wall-clock timestamps and timestamped IDs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Backend wall time, for persisted and externally visible timestamps only.
///
/// Deadline and elapsed-time code intentionally uses monotonic `Instant`s
/// directly.
pub(crate) struct WallClock;

impl WallClock {
    pub(crate) fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}

/// An identifier and its single authoritative creation timestamp.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TimestampedId {
    value: String,
    created_ms: i64,
}

impl TimestampedId {
    pub(crate) fn into_parts(self) -> (String, i64) {
        (self.value, self.created_ms)
    }
}

/// Adds process-local collision resistance to timestamp-based identifiers.
pub(crate) struct StableIdSequence {
    prefix: &'static str,
    next: AtomicU64,
}

impl StableIdSequence {
    pub(crate) const fn new(prefix: &'static str, first: u64) -> Self {
        Self {
            prefix,
            next: AtomicU64::new(first),
        }
    }

    /// Generate an ID and timestamp from one clock observation.
    pub(crate) fn next(&self) -> TimestampedId {
        let created_ms = WallClock::now_ms();
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        TimestampedId {
            value: format!("{}-{created_ms}-{sequence}", self.prefix),
            created_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StableIdSequence;

    #[test]
    fn timestamped_ids_carry_their_one_clock_observation() {
        let ids = StableIdSequence::new("train", 0);
        for sequence in 0..2 {
            let (id, created_ms) = ids.next().into_parts();
            assert_eq!(id, format!("train-{created_ms}-{sequence}"));
        }
    }
}
