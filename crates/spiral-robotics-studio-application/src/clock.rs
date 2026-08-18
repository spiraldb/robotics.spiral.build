//! Application-owned wall-clock and timestamped-ID boundaries.

use std::sync::atomic::{AtomicU64, Ordering};

/// Supplies application-level wall time.
///
/// Deadline and elapsed-time code intentionally continues to use monotonic
/// `Instant`s directly; this boundary is for persisted and externally visible
/// timestamps only.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// An identifier and its single authoritative creation timestamp.
#[derive(Debug, PartialEq, Eq)]
pub struct TimestampedId {
    pub value: String,
    pub created_ms: i64,
}

/// Adds process-local collision resistance to timestamp-based identifiers.
///
/// Keep these sequences static at their call sites so reconstructing a manager
/// in the same process cannot reset its namespace.
pub struct StableIdSequence {
    prefix: &'static str,
    next: AtomicU64,
}

impl StableIdSequence {
    pub const fn new(prefix: &'static str, first: u64) -> Self {
        Self {
            prefix,
            next: AtomicU64::new(first),
        }
    }

    /// Generate an ID and timestamp from one clock observation.
    pub fn next(&self, clock: &dyn Clock) -> TimestampedId {
        let created_ms = clock.now_ms();
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        TimestampedId {
            value: format!("{}-{created_ms}-{sequence}", self.prefix),
            created_ms,
        }
    }
}

#[cfg(test)]
pub struct FixedClock {
    now_ms: std::sync::atomic::AtomicI64,
}

#[cfg(test)]
impl FixedClock {
    pub const fn new(now_ms: i64) -> Self {
        Self {
            now_ms: std::sync::atomic::AtomicI64::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, FixedClock, StableIdSequence};

    #[test]
    fn fixed_clock_is_deterministic_and_controllable() {
        let clock = FixedClock::new(1_700_000_000_123);
        assert_eq!(clock.now_ms(), 1_700_000_000_123);

        clock.set(1_700_000_000_456);
        assert_eq!(clock.now_ms(), 1_700_000_000_456);
    }

    #[test]
    fn timestamped_ids_share_one_deterministic_clock_observation() {
        let clock = FixedClock::new(1_700_000_000_123);
        let ids = StableIdSequence::new("train", 0);

        let first = ids.next(&clock);
        let second = ids.next(&clock);

        assert_eq!(first.value, "train-1700000000123-0");
        assert_eq!(first.created_ms, 1_700_000_000_123);
        assert_eq!(second.value, "train-1700000000123-1");
        assert_eq!(second.created_ms, 1_700_000_000_123);
    }
}
