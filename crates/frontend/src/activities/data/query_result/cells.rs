//! How a result column paints its cells. The exact value never changes; the
//! kind decides how it is read: images as previews, decisions as chips,
//! measurements with a bar, timestamps as clock time, identifiers in mono.
//!
//! Every kind is inferred from the column's Arrow type, its name, and the
//! rows actually returned, and every inference is display-only. The
//! inspector and every tooltip keep the stored value verbatim.

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use spiral_robotics_common::relations::QueryColumn;

use super::{OptionalQueryValue, QueryColumnPresentation, QueryJsonValue};

const CATEGORY_NAME_HINTS: [&str; 12] = [
    "decision", "status", "label", "class", "category", "kind", "verdict", "outcome", "state",
    "type", "tag", "bucket",
];
const IDENTITY_NAME_HINTS: [&str; 9] = [
    "_id", "id", "_path", "path", "_key", "_uri", "_url", "hash", "revision",
];
const POSITIONAL_NAME_HINTS: [&str; 9] = [
    "offset", "ordinal", "index", "frame", "position", "seq", "sequence", "step", "number",
];
const MAX_CATEGORY_VALUES: usize = 8;
const MAX_CATEGORY_LENGTH: usize = 40;
pub(super) const CATEGORY_HUES: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TimeUnit {
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

impl TimeUnit {
    fn of(column: &QueryColumn, values: &[Option<&Value>]) -> Option<Self> {
        let data_type = column.data_type().to_ascii_lowercase();
        if data_type.contains("timestamp") {
            return Some(if data_type.contains("nano") || data_type.contains("[ns") {
                Self::Nanoseconds
            } else if data_type.contains("micro")
                || data_type.contains("[us")
                || data_type.contains("[µs")
            {
                Self::Microseconds
            } else if data_type.contains("milli") || data_type.contains("[ms") {
                Self::Milliseconds
            } else {
                Self::Seconds
            });
        }
        let name = column.name().to_ascii_lowercase();
        let named = name.contains("time")
            || name.ends_with("_at")
            || name.ends_with("_ns")
            || name.ends_with("_ms")
            || name.ends_with("_us");
        if !named {
            return None;
        }
        let mut magnitude = 0.0_f64;
        for value in values.iter().flatten() {
            let number = value.as_f64()?;
            magnitude = magnitude.max(number.abs());
        }
        Some(if magnitude >= 1e17 {
            Self::Nanoseconds
        } else if magnitude >= 1e14 {
            Self::Microseconds
        } else if magnitude >= 1e11 {
            Self::Milliseconds
        } else if magnitude >= 1e9 {
            Self::Seconds
        } else {
            return None;
        })
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Seconds => "s",
            Self::Milliseconds => "ms",
            Self::Microseconds => "µs",
            Self::Nanoseconds => "ns",
        }
    }

    /// Clock time in UTC to the millisecond, or `None` when the value cannot
    /// be a timestamp anyone would recognise.
    fn format(self, value: &Value) -> Option<String> {
        let (seconds, millis) = match value {
            Value::Number(number) if number.is_f64() && self == Self::Seconds => {
                let seconds = number.as_f64()?;
                let whole = seconds.floor();
                (whole as i128, ((seconds - whole) * 1000.0).round() as i128)
            }
            Value::Number(number) => {
                let ticks = number
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| number.as_u64().map(i128::from))
                    .or_else(|| number.as_f64().map(|value| value as i128))?;
                let per_second: i128 = match self {
                    Self::Seconds => 1,
                    Self::Milliseconds => 1_000,
                    Self::Microseconds => 1_000_000,
                    Self::Nanoseconds => 1_000_000_000,
                };
                (
                    ticks.div_euclid(per_second),
                    ticks.rem_euclid(per_second) * 1000 / per_second,
                )
            }
            _ => return None,
        };
        let days = seconds.div_euclid(86_400);
        let second_of_day = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(i64::try_from(days).ok()?);
        if !(1900..=2200).contains(&year) {
            return None;
        }
        Some(format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}",
            second_of_day / 3_600,
            second_of_day % 3_600 / 60,
            second_of_day % 60,
        ))
    }
}

/// Proleptic Gregorian date from days since 1970-01-01 (Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CellKind {
    Image,
    Timestamp(TimeUnit),
    /// Numbers that vary across the rows and are worth comparing at a glance.
    Measure {
        min: f64,
        max: f64,
    },
    Number,
    Boolean,
    Identity,
    /// Short repeated strings; `values` is the sorted distinct set that fixes
    /// each value's hue for this result.
    Category {
        values: Vec<String>,
    },
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResultColumn {
    column: QueryColumn,
    kind: CellKind,
}

impl ResultColumn {
    pub(super) fn analyze(column: &QueryColumn, rows: &[Map<String, Value>]) -> Self {
        let values = rows
            .iter()
            .map(|row| row.get(column.name()))
            .collect::<Vec<_>>();
        let present = values.iter().flatten().filter(|value| !value.is_null());
        let kind = if column.is_image() {
            CellKind::Image
        } else if let Some(unit) = TimeUnit::of(column, &values) {
            CellKind::Timestamp(unit)
        } else if present.clone().any(|value| value.is_boolean())
            && present.clone().all(|value| value.is_boolean())
        {
            CellKind::Boolean
        } else if present.clone().any(|value| value.is_number())
            && present.clone().all(|value| value.is_number())
        {
            if Self::names(column.name(), &IDENTITY_NAME_HINTS)
                || Self::names(column.name(), &POSITIONAL_NAME_HINTS)
            {
                CellKind::Number
            } else {
                Self::measure_range(&values).map_or(CellKind::Number, |(min, max)| {
                    CellKind::Measure { min, max }
                })
            }
        } else if Self::names(column.name(), &IDENTITY_NAME_HINTS) {
            CellKind::Identity
        } else if let Some(values) = Self::category_values(column.name(), &values) {
            CellKind::Category { values }
        } else {
            CellKind::Text
        };
        Self {
            column: column.clone(),
            kind,
        }
    }

    fn names(name: &str, hints: &[&str]) -> bool {
        let name = name.to_ascii_lowercase();
        hints.iter().any(|hint| {
            if hint.starts_with('_') {
                name.ends_with(hint)
            } else {
                name == *hint || name.ends_with(&format!("_{hint}"))
            }
        })
    }

    fn measure_range(values: &[Option<&Value>]) -> Option<(f64, f64)> {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut distinct = BTreeSet::new();
        for number in values.iter().flatten().filter_map(|value| value.as_f64()) {
            if !number.is_finite() {
                continue;
            }
            min = min.min(number);
            max = max.max(number);
            distinct.insert(number.to_bits());
        }
        (distinct.len() >= 2 && max > min).then_some((min, max))
    }

    fn category_values(name: &str, values: &[Option<&Value>]) -> Option<Vec<String>> {
        let strings = values
            .iter()
            .flatten()
            .filter(|value| !value.is_null())
            .map(|value| value.as_str())
            .collect::<Option<Vec<_>>>()?;
        if strings.is_empty()
            || strings.iter().any(|value| {
                value.chars().count() > MAX_CATEGORY_LENGTH
                    || value.contains("://")
                    || value.starts_with("data:")
            })
        {
            return None;
        }
        let distinct = strings.iter().copied().collect::<BTreeSet<_>>();
        let named = Self::names(name, &CATEGORY_NAME_HINTS);
        let repeated = strings.len() >= 3
            && distinct.len() <= MAX_CATEGORY_VALUES
            && distinct.len() * 2 <= strings.len();
        (named || repeated).then(|| distinct.into_iter().map(str::to_owned).collect())
    }

    pub(super) fn column(&self) -> &QueryColumn {
        &self.column
    }

    pub(super) fn is_image(&self) -> bool {
        self.kind == CellKind::Image
    }

    /// Images lead, then the fields that decide, then measurements; long
    /// identifiers and paths wait at the end of the same table.
    pub(super) fn priority(&self) -> u8 {
        match self.kind {
            CellKind::Image => 0,
            CellKind::Category { .. } => 1,
            CellKind::Measure { .. } => 2,
            CellKind::Timestamp(_) | CellKind::Number | CellKind::Boolean | CellKind::Text => 3,
            CellKind::Identity => 4,
        }
    }

    /// The column's own name, spelled as the query or schema spelled it, so
    /// an operator can type it back into SQL.
    pub(super) fn label(&self) -> String {
        self.column.name().to_owned()
    }

    /// The type shown under a header when types are requested.
    pub(super) fn kind_label(&self) -> String {
        match &self.kind {
            CellKind::Timestamp(unit) => format!("Timestamp · {}", unit.label()),
            _ => self.column.presentation_kind().to_owned(),
        }
    }

    pub(super) fn cell_class(&self, value: Option<&Value>) -> String {
        let mut classes = vec!["query-cell"];
        match self.kind {
            CellKind::Measure { .. } => classes.push("query-cell-measure"),
            CellKind::Number => classes.push("query-cell-number"),
            CellKind::Timestamp(_) => classes.push("query-cell-time"),
            CellKind::Identity => classes.push("query-cell-identity"),
            CellKind::Category { .. } => classes.push("query-cell-category"),
            CellKind::Boolean => classes.push("query-cell-boolean"),
            CellKind::Image | CellKind::Text => {}
        }
        let base = value.presentation_class();
        if !base.is_empty() {
            classes.push(base);
        }
        classes.join(" ")
    }

    /// The readable form of a cell. Nulls read as a dash.
    pub(super) fn display(&self, value: Option<&Value>) -> String {
        match (value, &self.kind) {
            (None | Some(Value::Null), _) => "—".to_owned(),
            (Some(value), CellKind::Timestamp(unit)) => unit
                .format(value)
                .or_else(|| {
                    value
                        .as_str()
                        .map(|text| text.replacen('T', " ", 1).trim_end_matches('Z').to_owned())
                })
                .unwrap_or_else(|| ResultValue(Some(value)).display()),
            (Some(value), _) => ResultValue(Some(value)).display(),
        }
    }

    /// The stored value, verbatim, for tooltips and the row inspector.
    pub(super) fn exact(&self, value: Option<&Value>) -> String {
        match (value, &self.kind) {
            (Some(value), CellKind::Timestamp(unit)) if value.is_number() => {
                match unit.format(value) {
                    Some(clock) => format!("{} {} · {clock} UTC", value.cell_value(), unit.label()),
                    None => value.cell_value(),
                }
            }
            (Some(Value::Null) | None, _) => "null".to_owned(),
            (Some(value), _) => value.cell_value(),
        }
    }

    /// Which of the six category hues this value takes, stable for the result.
    pub(super) fn category_hue(&self, value: Option<&Value>) -> Option<usize> {
        let CellKind::Category { values } = &self.kind else {
            return None;
        };
        let text = value?.as_str()?;
        values
            .iter()
            .position(|candidate| candidate == text)
            .map(|index| index % CATEGORY_HUES + 1)
    }

    /// Where this value sits between the column's smallest and largest row.
    /// A column that crosses zero anchors its bars at zero so sign is visible.
    pub(super) fn measure_bar(&self, value: Option<&Value>) -> Option<MeasureBar> {
        let CellKind::Measure { min, max } = self.kind else {
            return None;
        };
        let number = value?.as_f64()?;
        if !number.is_finite() {
            return None;
        }
        let position = |value: f64| ((value - min) / (max - min)).clamp(0.0, 1.0);
        let at = position(number);
        if min < 0.0 && max > 0.0 {
            let zero = position(0.0);
            Some(MeasureBar {
                start: at.min(zero),
                end: at.max(zero),
                zero: Some(zero),
                negative: number < 0.0,
            })
        } else {
            Some(MeasureBar {
                start: 0.0,
                end: at,
                zero: None,
                negative: false,
            })
        }
    }

    pub(super) fn measure_title(&self, value: Option<&Value>, rows: usize) -> String {
        let CellKind::Measure { min, max } = self.kind else {
            return self.exact(value);
        };
        format!(
            "{} · bar spans {} to {} across these {rows} rows",
            self.exact(value),
            ResultValue::number(min),
            ResultValue::number(max)
        )
    }
}

/// A value's bar as fractions of the cell width. `zero` is set only when the
/// column has both signs, and then marks where the bar is anchored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MeasureBar {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) zero: Option<f64>,
    pub(super) negative: bool,
}

pub(super) struct ResultValue<'a>(pub(super) Option<&'a Value>);

impl ResultValue<'_> {
    pub(super) fn display(&self) -> String {
        match self.0 {
            None | Some(Value::Null) => "—".to_owned(),
            Some(Value::Number(number)) if number.is_f64() => {
                Self::number(number.as_f64().unwrap_or_default())
            }
            value => value.cell_text(),
        }
    }

    /// Readable precision: about six significant figures, no trailing zeros,
    /// scientific notation only at the extremes.
    fn number(value: f64) -> String {
        if value == 0.0 {
            return "0".to_owned();
        }
        if value.abs() < 0.00001 || value.abs() >= 1e9 {
            return format!("{value:.4e}");
        }
        let decimals = (5.0 - value.abs().log10().floor()).clamp(0.0, 10.0) as usize;
        let text = format!("{value:.decimals$}");
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text
        }
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(name: &str, values: Vec<Value>) -> Vec<Map<String, Value>> {
        values
            .into_iter()
            .map(|value| Map::from_iter([(name.to_owned(), value)]))
            .collect()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn readable_floats_preserve_integer_identity_and_nonzero_magnitudes() {
        assert_eq!(
            ResultValue(Some(&json!(18446744073709551615_u64))).display(),
            "18446744073709551615"
        );
        assert_eq!(
            ResultValue(Some(&json!(0.002061118))).display(),
            "0.00206112"
        );
        assert_eq!(
            ResultValue(Some(&json!(0.000000002))).display(),
            "2.0000e-9"
        );
        assert_eq!(ResultValue(Some(&json!(0.03))).display(), "0.03");
        assert_eq!(ResultValue(None).display(), "—");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn nanosecond_log_times_read_as_utc_clock_time_and_keep_the_raw_value() {
        let column = QueryColumn::new("log_time", "Int64", false);
        let times = rows("log_time", vec![json!(1786639677057000000_i64)]);
        let result = ResultColumn::analyze(&column, &times);
        assert_eq!(result.kind, CellKind::Timestamp(TimeUnit::Nanoseconds));
        let value = times[0].get("log_time");
        assert_eq!(result.display(value), "2026-08-13 16:47:57.057");
        assert_eq!(
            result.exact(value),
            "1786639677057000000 ns · 2026-08-13 16:47:57.057 UTC"
        );
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(951782400 / 86400), (2000, 2, 29));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn arrow_timestamp_types_take_their_unit_from_the_type_name() {
        let column = QueryColumn::new("captured", "Timestamp(Millisecond, None)", true);
        let captured = rows("captured", vec![json!(0_i64), json!(4102444800000_i64)]);
        let result = ResultColumn::analyze(&column, &captured);
        assert_eq!(result.kind, CellKind::Timestamp(TimeUnit::Milliseconds));
        assert_eq!(
            result.display(captured[1].get("captured")),
            "2100-01-01 00:00:00.000"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn positional_counters_and_identifiers_stay_plain_numbers() {
        let offsets = rows("chunk_start_offset", vec![json!(1), json!(2034583)]);
        let column = QueryColumn::new("chunk_start_offset", "Int64", false);
        assert_eq!(
            ResultColumn::analyze(&column, &offsets).kind,
            CellKind::Number
        );
        let ids = rows("episode_id", vec![json!(7), json!(9)]);
        let column = QueryColumn::new("episode_id", "Int64", false);
        assert_eq!(ResultColumn::analyze(&column, &ids).kind, CellKind::Number);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn varying_measurements_get_a_bar_between_their_extremes() {
        let column = QueryColumn::new("mean_linear_luminance", "Float64", true);
        let luminance = rows(
            "mean_linear_luminance",
            vec![json!(0.02), json!(0.06), json!(0.1), Value::Null],
        );
        let result = ResultColumn::analyze(&column, &luminance);
        assert_eq!(
            result.kind,
            CellKind::Measure {
                min: 0.02,
                max: 0.1
            }
        );
        let bar = result
            .measure_bar(luminance[1].get("mean_linear_luminance"))
            .unwrap();
        assert_eq!(bar.start, 0.0);
        assert!((bar.end - 0.5).abs() < 1e-9);
        assert_eq!(bar.zero, None);
        assert!(!bar.negative);
        assert_eq!(
            result.measure_bar(luminance[3].get("mean_linear_luminance")),
            None
        );
        let column = QueryColumn::new("holding_vs_empty_score", "Float64", true);
        let signed = rows(
            "holding_vs_empty_score",
            vec![json!(-0.2), json!(0.2), json!(0.6)],
        );
        let result = ResultColumn::analyze(&column, &signed);
        let below = result
            .measure_bar(signed[0].get("holding_vs_empty_score"))
            .unwrap();
        assert!((below.zero.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(below.start, 0.0);
        assert!((below.end - 0.25).abs() < 1e-9);
        assert!(below.negative);
        let above = result
            .measure_bar(signed[2].get("holding_vs_empty_score"))
            .unwrap();
        assert!((above.start - 0.25).abs() < 1e-9);
        assert_eq!(above.end, 1.0);
        assert!(!above.negative);
        let constant = rows("score", vec![json!(1.5), json!(1.5)]);
        let column = QueryColumn::new("score", "Float64", false);
        assert_eq!(
            ResultColumn::analyze(&column, &constant).kind,
            CellKind::Number
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn decisions_become_categories_with_stable_hues_but_paths_do_not() {
        let column = QueryColumn::new("decision", "Utf8", false);
        let decisions = rows(
            "decision",
            vec![json!("retained"), json!("excluded"), json!("retained")],
        );
        let result = ResultColumn::analyze(&column, &decisions);
        assert!(
            matches!(&result.kind, CellKind::Category { values } if values == &["excluded", "retained"])
        );
        assert_eq!(result.category_hue(decisions[0].get("decision")), Some(2));
        assert_eq!(result.category_hue(decisions[1].get("decision")), Some(1));
        assert_eq!(result.priority(), 1);

        let column = QueryColumn::new("topic", "Utf8", false);
        let repeated = rows("topic", vec![json!("/camera"); 6]);
        assert!(matches!(
            ResultColumn::analyze(&column, &repeated).kind,
            CellKind::Category { .. }
        ));

        let column = QueryColumn::new("source_path", "Utf8", false);
        let paths = rows("source_path", vec![json!("/a"); 6]);
        assert_eq!(
            ResultColumn::analyze(&column, &paths).kind,
            CellKind::Identity
        );
        assert_eq!(ResultColumn::analyze(&column, &paths).priority(), 4);

        let column = QueryColumn::new("note", "Utf8", false);
        let unique = rows("note", vec![json!("a"), json!("b"), json!("c")]);
        assert_eq!(ResultColumn::analyze(&column, &unique).kind, CellKind::Text);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn image_columns_lead_and_report_their_kind() {
        let column = QueryColumn::new("frame", "Binary", false)
            .with_semantic_type(Some("spql.image.Image".to_owned()));
        let result = ResultColumn::analyze(&column, &[]);
        assert!(result.is_image());
        assert_eq!(result.priority(), 0);
        assert_eq!(result.kind_label(), "Image");
        assert_eq!(result.label(), "frame");
    }
}
