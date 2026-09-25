/// Literal spelling for generated SQL. Dataset names, topics, prompts, and
/// rule values all travel this way; nothing is interpolated raw.
pub struct SqlLiteral;

impl SqlLiteral {
    pub fn text(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
    /// A finite number as a double-precision literal; SQL has no spelling for
    /// the others.
    pub fn double(value: f64) -> Option<String> {
        value
            .is_finite()
            .then(|| format!("CAST({value} AS DOUBLE)"))
    }
}

#[cfg(test)]
mod tests {
    use super::SqlLiteral;

    #[test]
    fn numbers_are_written_out_in_full_and_only_when_finite() {
        assert_eq!(
            SqlLiteral::double(20.0).as_deref(),
            Some("CAST(20 AS DOUBLE)")
        );
        assert_eq!(
            SqlLiteral::double(-0.000_000_1).as_deref(),
            Some("CAST(-0.0000001 AS DOUBLE)")
        );
        assert_eq!(SqlLiteral::double(f64::NAN), None);
        assert_eq!(SqlLiteral::double(f64::INFINITY), None);
        assert_eq!(SqlLiteral::text("owner's"), "'owner''s'");
    }
}
