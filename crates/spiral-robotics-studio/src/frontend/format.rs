//! Stable display formatting shared across Studio surfaces.

/// Minutes, seconds, and tenths for every episode and Timeline clock.
pub(crate) fn format_clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}

/// Fixed precision for ordinary losses and scientific notation near zero.
pub(crate) fn format_loss(value: f64) -> String {
    if value >= 0.01 {
        format!("{value:.4}")
    } else {
        format!("{value:.3e}")
    }
}

#[cfg(test)]
mod tests {
    use super::{format_clock, format_loss};

    #[test]
    fn clock_format_is_stable_for_invalid_and_elapsed_values() {
        assert_eq!(format_clock(f64::NAN), "0:00.0");
        assert_eq!(format_clock(-2.0), "0:00.0");
        assert_eq!(format_clock(61.25), "1:01.2");
    }

    #[test]
    fn loss_precision_is_shared_by_training_and_rollout() {
        assert_eq!(format_loss(0.125), "0.1250");
        assert_eq!(format_loss(0.00125), "1.250e-3");
    }
}
