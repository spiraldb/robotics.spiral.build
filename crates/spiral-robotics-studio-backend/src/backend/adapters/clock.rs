//! Production wall-clock adapter.

use std::time::{SystemTime, UNIX_EPOCH};

use spiral_robotics_studio_application::clock::Clock;

#[derive(Debug, Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}
