/// Literal spelling for generated SQL. Dataset names, topics, prompts, and
/// rule values all travel this way; nothing is interpolated raw.
pub struct SqlLiteral;

impl SqlLiteral {
    pub fn text(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
}
