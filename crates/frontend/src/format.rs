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

pub(crate) trait PreciseClockPresentation {
    /// Minutes, seconds, and milliseconds for message-level inspection.
    fn precise_clock_label(self) -> String;
}

impl PreciseClockPresentation for f64 {
    fn precise_clock_label(self) -> String {
        let seconds = if self.is_finite() { self.max(0.0) } else { 0.0 };
        let total_millis = (seconds * 1000.0).round() as u64;
        let minutes = total_millis / 60_000;
        let millis = total_millis % 60_000;
        format!("{minutes}:{:02}.{:03}", millis / 1000, millis % 1000)
    }
}

pub(crate) trait GroupedCountPresentation {
    /// Thousands separators for message and row counts.
    fn grouped_label(self) -> String;
}

impl GroupedCountPresentation for u64 {
    fn grouped_label(self) -> String {
        let digits = self.to_string();
        let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        grouped
    }
}

pub(crate) trait ByteCountPresentation {
    /// Binary-prefixed size with one decimal above kilobytes.
    fn byte_label(self) -> String;
}

impl ByteCountPresentation for u64 {
    fn byte_label(self) -> String {
        const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
        let mut value = self as f64;
        let mut unit = 0;
        while value >= 1024.0 && unit + 1 < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{self} B")
        } else {
            format!("{value:.1} {}", UNITS[unit])
        }
    }
}

/// Throughput as a training or ingest readout prints it: always a binary
/// prefix per second, never bytes per second, because the figures it describes
/// are never that small.
pub(crate) trait ByteRatePresentation {
    fn byte_rate_label(self) -> String;
}

impl ByteRatePresentation for f64 {
    fn byte_rate_label(self) -> String {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        const MIB: f64 = 1024.0 * 1024.0;
        if self >= GIB {
            format!("{:.2} GiB/s", self / GIB)
        } else {
            format!("{:.1} MiB/s", self / MIB)
        }
    }
}

/// A measured latency, stepping up to seconds and down to tenths so the figure
/// keeps three significant digits. A value that was never measured reads as an
/// em dash rather than as zero.
pub(crate) trait MillisecondPresentation {
    fn millis_label(self) -> String;
}

impl MillisecondPresentation for Option<f64> {
    fn millis_label(self) -> String {
        self.filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| {
                if value >= 1_000.0 {
                    format!("{:.2} s", value / 1_000.0)
                } else if value >= 10.0 {
                    format!("{value:.0} ms")
                } else {
                    format!("{value:.1} ms")
                }
            })
            .unwrap_or_else(|| "—".to_owned())
    }
}

pub(crate) trait UtcTimestampPresentation {
    /// `YYYY-MM-DD HH:MM:SS UTC` from Unix nanoseconds, computed without the
    /// browser so native and WASM builds agree.
    fn utc_label(self) -> String;
    /// `YYYY-MM-DD` only.
    fn utc_date_label(self) -> String;
    /// `HH:MM:SS UTC` only.
    fn utc_time_label(self) -> String;
}

/// A recording whose clock started at the Unix epoch, as simulators and
/// bench rigs counting from zero do, carries no wall-clock time. Such a
/// moment is reported as absent rather than as a day in 1970.
pub(crate) trait WallClockPresentation {
    fn has_wall_clock(self) -> bool;
}

/// 2000-01-01T00:00:00Z in Unix nanoseconds; no robot recording predates it.
const WALL_CLOCK_FLOOR_NS: u64 = 946_684_800_000_000_000;

impl WallClockPresentation for u64 {
    fn has_wall_clock(self) -> bool {
        self >= WALL_CLOCK_FLOOR_NS
    }
}

/// One UTC moment as a calendar reads it, computed without the browser so
/// native and WASM builds agree. Every Studio surface that prints a date —
/// episode headers, dataset facts, a timestamp column in a query result — takes
/// it from here rather than spelling the calendar again.
pub(crate) struct CivilDateTime {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl CivilDateTime {
    fn from_unix_nanos(nanos: u64) -> Self {
        let seconds = (nanos / 1_000_000_000) as i64;
        let remainder = seconds.rem_euclid(86_400) as u32;
        Self {
            hour: remainder / 3_600,
            minute: remainder % 3_600 / 60,
            second: remainder % 60,
            ..Self::from_unix_days(seconds.div_euclid(86_400))
        }
    }

    /// Midnight UTC of the day `days` after the Unix epoch; a negative count is
    /// days before it, so a column of pre-1970 timestamps still reads.
    pub(crate) fn from_unix_days(days: i64) -> Self {
        // Howard Hinnant's civil-from-days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        } as u32;
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        Self {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    // The three accessors below are what the timestamp column reads.
    pub(crate) const fn year(&self) -> i64 {
        self.year
    }
    pub(crate) const fn month(&self) -> u32 {
        self.month
    }
    pub(crate) const fn day(&self) -> u32 {
        self.day
    }
}

impl UtcTimestampPresentation for u64 {
    fn utc_label(self) -> String {
        let civil = CivilDateTime::from_unix_nanos(self);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
        )
    }
    fn utc_date_label(self) -> String {
        let civil = CivilDateTime::from_unix_nanos(self);
        format!("{:04}-{:02}-{:02}", civil.year, civil.month, civil.day)
    }
    fn utc_time_label(self) -> String {
        let civil = CivilDateTime::from_unix_nanos(self);
        format!(
            "{:02}:{:02}:{:02} UTC",
            civil.hour, civil.minute, civil.second
        )
    }
}

pub(crate) trait LongDurationPresentation {
    /// Hours, minutes, and seconds for totals across many recordings.
    fn long_duration_label(self) -> String;
}

impl LongDurationPresentation for f64 {
    fn long_duration_label(self) -> String {
        let total = if self.is_finite() {
            self.max(0.0).round() as u64
        } else {
            0
        };
        let (hours, minutes, seconds) = (total / 3600, total % 3600 / 60, total % 60);
        if hours > 0 {
            format!("{hours} h {minutes} min")
        } else if minutes > 0 {
            format!("{minutes} min {seconds} s")
        } else {
            format!("{seconds} s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every clock here is fed a number a recording can actually produce: a
    /// missing measurement, a negative elapsed time, and a value that rounds
    /// across a unit.
    #[test]
    fn clocks_survive_the_values_a_recording_hands_them() {
        assert_eq!(f64::NAN.clock_label(), "0:00.0");
        assert_eq!((-2.0_f64).clock_label(), "0:00.0");
        assert_eq!(61.25_f64.clock_label(), "1:01.2");
        assert_eq!(61.0005_f64.precise_clock_label(), "1:01.001");
        assert_eq!(f64::NAN.precise_clock_label(), "0:00.000");
        assert_eq!(3_549.9_f64.long_duration_label(), "59 min 10 s");
        assert_eq!(7_260.0_f64.long_duration_label(), "2 h 1 min");
        assert_eq!(f64::NAN.long_duration_label(), "0 s");
    }

    /// Each ladder is checked where it steps: the separator's first comma, the
    /// byte unit's first prefix, the rate's GiB threshold, the latency's two
    /// thresholds and its missing value, and the loss's switch to scientific
    /// notation.
    #[test]
    fn every_unit_ladder_is_checked_where_it_steps() {
        assert_eq!(11_404_u64.grouped_label(), "11,404");
        assert_eq!(1_000_000_u64.grouped_label(), "1,000,000");
        assert_eq!(512_u64.byte_label(), "512 B");
        assert_eq!(15_932_137_u64.byte_label(), "15.2 MiB");
        assert_eq!(
            (1024.0_f64 * 1024.0 * 1024.0).byte_rate_label(),
            "1.00 GiB/s"
        );
        assert_eq!(
            (1024.0_f64 * 1024.0 * 900.0).byte_rate_label(),
            "900.0 MiB/s"
        );
        assert_eq!(Some(1_250.0).millis_label(), "1.25 s");
        assert_eq!(Some(42.4).millis_label(), "42 ms");
        assert_eq!(Some(4.25).millis_label(), "4.2 ms");
        assert_eq!(None.millis_label(), "—");
        assert_eq!(0.125_f64.loss_label(), "0.1250");
        assert_eq!(0.00125_f64.loss_label(), "1.250e-3");
    }

    /// The calendar is computed here rather than read from the browser, so a
    /// leap day and the epoch itself pin it.
    #[test]
    fn utc_labels_come_from_exact_nanoseconds() {
        assert_eq!(
            1_743_701_335_734_982_912_u64.utc_label(),
            "2025-04-03 17:28:55 UTC"
        );
        assert_eq!(0_u64.utc_label(), "1970-01-01 00:00:00 UTC");
        assert_eq!(951_782_400_000_000_000_u64.utc_date_label(), "2000-02-29");
        assert_eq!(
            1_743_701_335_734_982_912_u64.utc_time_label(),
            "17:28:55 UTC"
        );
        let leap = CivilDateTime::from_unix_days(11_016);
        assert_eq!((leap.year(), leap.month(), leap.day()), (2000, 2, 29));
        // A rig counting from zero has no wall clock; a recording made in 2000
        // or later has one.
        assert!(!5_000_000_000_u64.has_wall_clock());
        assert!(WALL_CLOCK_FLOOR_NS.has_wall_clock());
    }
}
