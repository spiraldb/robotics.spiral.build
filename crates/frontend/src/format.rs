//! Stable display formatting shared across Studio surfaces.

pub(crate) trait ClockPresentation {
    /// Minutes, seconds, and tenths for every episode and Timeline clock.
    fn clock_label(self) -> String;
}

impl ClockPresentation for f64 {
    fn clock_label(self) -> String {
        let seconds = if self.is_finite() { self.max(0.0) } else { 0.0 };
        let minutes = (seconds / 60.0).floor() as u64;
        let remainder = seconds - minutes as f64 * 60.0;
        format!("{minutes}:{remainder:04.1}")
    }
}

pub(crate) trait LossPresentation {
    /// Fixed precision for ordinary losses and scientific notation near zero.
    fn loss_label(self) -> String;
}

impl LossPresentation for f64 {
    fn loss_label(self) -> String {
        if self >= 0.01 {
            format!("{self:.4}")
        } else {
            format!("{self:.3e}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClockPresentation, LossPresentation};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn clock_format_is_stable_for_invalid_and_elapsed_values() {
        assert_eq!(f64::NAN.clock_label(), "0:00.0");
        assert_eq!((-2.0_f64).clock_label(), "0:00.0");
        assert_eq!(61.25_f64.clock_label(), "1:01.2");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn loss_precision_is_shared_by_training_and_rollout() {
        assert_eq!(0.125_f64.loss_label(), "0.1250");
        assert_eq!(0.00125_f64.loss_label(), "1.250e-3");
    }
}
