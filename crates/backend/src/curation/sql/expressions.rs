use spiral_robotics_common::{
    curation::{
        facts::{FactMeasure, FieldAggregate},
        rules::{ConditionOp, ConditionValue, RuleCondition},
    },
    sql::SqlLiteral,
};

pub(super) struct FactSql<'a>(pub(super) &'a FactMeasure);
impl FactSql<'_> {
    /// The CTEs that compute this fact as `(id, value)`, named from `prefix`.
    /// The last CTE is `prefix` itself.
    pub(super) fn sql_ctes(&self, prefix: &str, dataset: &str) -> Vec<String> {
        match self.0 {
            FactMeasure::CameraBrightness { camera } => Self::camera_ctes(
                prefix,
                camera.frame_ctes(prefix, dataset),
                "spql.image.mean_luminance(image)".to_owned(),
            ),
            FactMeasure::CameraScore { camera, prompt } => Self::camera_ctes(
                prefix,
                camera.frame_ctes(prefix, dataset),
                format!("spql.ai.score(image, {})", SqlLiteral::text(prompt)),
            ),
            FactMeasure::TopicField {
                topic,
                field,
                aggregate,
                ..
            } => vec![format!(
                "{prefix} AS (SELECT id, {} AS value FROM {} GROUP BY id)",
                Self::aggregate(*aggregate, &format!("value.{field}")),
                topic.sql_source(dataset)
            )],
            FactMeasure::Sql { sql, .. } => vec![format!(
                "{prefix} AS (SELECT id, value FROM ({}) AS custom)",
                sql.trim().trim_end_matches(';')
            )],
        }
    }
    fn camera_ctes(prefix: &str, mut ctes: Vec<String>, measurement: String) -> Vec<String> {
        ctes.push(format!(
            "{prefix} AS (SELECT id, avg({measurement}) AS value FROM {prefix}_frames GROUP BY id)"
        ));
        ctes
    }
    fn aggregate(aggregate: FieldAggregate, expression: &str) -> String {
        match aggregate {
            FieldAggregate::First => format!("spql.agg.first({expression})"),
            FieldAggregate::Last => format!("spql.agg.last({expression})"),
            FieldAggregate::Min => format!("min({expression})"),
            FieldAggregate::Max => format!("max({expression})"),
            FieldAggregate::Avg => format!("avg({expression})"),
            FieldAggregate::Count => format!("count({expression})"),
        }
    }
}

pub(super) struct ConditionSql<'a>(pub(super) &'a RuleCondition);
impl ConditionSql<'_> {
    /// A boolean SQL expression over the named fact expression. NULL facts
    /// never fire a comparison; only `Missing` sees them.
    pub(super) fn sql(&self, expression: &str) -> Result<String, String> {
        let value = || {
            Self::value(self.0.value())
                .ok_or_else(|| format!("rule on {} has no value", self.0.fact()))
        };
        Ok(match self.0.op() {
            ConditionOp::Below => format!("({expression} < {})", value()?),
            ConditionOp::AtMost => format!("({expression} <= {})", value()?),
            ConditionOp::Above => format!("({expression} > {})", value()?),
            ConditionOp::AtLeast => format!("({expression} >= {})", value()?),
            ConditionOp::Is => format!("({expression} = {})", value()?),
            ConditionOp::IsNot => format!("({expression} <> {})", value()?),
            ConditionOp::Contains => format!(
                "(CAST({expression} AS VARCHAR) ILIKE {})",
                Self::like_pattern(self.0.value()).ok_or("contains needs text")?
            ),
            ConditionOp::Lacks => format!(
                "(CAST({expression} AS VARCHAR) NOT ILIKE {})",
                Self::like_pattern(self.0.value()).ok_or("does not contain needs text")?
            ),
            ConditionOp::Missing => format!("({expression} IS NULL)"),
            ConditionOp::Present => format!("({expression} IS NOT NULL)"),
        })
    }
    fn value(value: &ConditionValue) -> Option<String> {
        match value {
            ConditionValue::Number(_) => Some(value.display()),
            ConditionValue::Text(text) => Some(SqlLiteral::text(text)),
            ConditionValue::None => None,
        }
    }
    fn like_pattern(value: &ConditionValue) -> Option<String> {
        match value {
            ConditionValue::None => None,
            _ => Some(SqlLiteral::text(&format!("%{}%", value.display()))),
        }
    }
}
