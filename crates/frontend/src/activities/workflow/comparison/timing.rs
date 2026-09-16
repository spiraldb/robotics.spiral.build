//! Developer timings for the comparison critical path, without prompts or result payloads.
use std::future::Future;

pub(super) struct ComparisonTiming {
    phase: String,
    started: f64,
}
impl ComparisonTiming {
    pub(super) async fn measure<T>(phase: impl Into<String>, work: impl Future<Output = T>) -> T {
        let _timing = Self {
            phase: phase.into(),
            started: js_sys::Date::now(),
        };
        web_sys::console::log_1(
            &format!("comparison_profile started phase={}", _timing.phase).into(),
        );
        work.await
    }
}
impl Drop for ComparisonTiming {
    fn drop(&mut self) {
        web_sys::console::log_1(
            &format!(
                "comparison_profile finished phase={} elapsed_ms={}",
                self.phase,
                js_sys::Date::now() - self.started
            )
            .into(),
        );
    }
}
