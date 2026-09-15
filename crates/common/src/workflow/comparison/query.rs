use super::*;

impl EpisodeComparison {
    /// One summary plus at most ten bins per metric: never more than 61 rows,
    /// regardless of episode count. Null/nonfinite values do not enter averages.
    pub fn summary_sql(&self, population: &str) -> Result<String, String> {
        let prefix = self.query_prefix(population)?;
        let measurements = self.metrics.iter().enumerate().map(|(index, _)| {
            format!("SELECT {index} AS metric, metric_{index} AS value, cohort FROM comparison_population")
        }).collect::<Vec<_>>().join("\nUNION ALL\n");
        let sql = format!(
            r#"{prefix},
comparison_values AS ({measurements}),
comparison_ranges AS (
 SELECT metric, MIN(value) AS low, MAX(value) AS high
 FROM comparison_values WHERE value BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308 GROUP BY metric
),
comparison_bins AS (
 SELECT v.*, r.low, r.high,
 CASE WHEN r.high = r.low THEN 0 WHEN v.value = r.high THEN 9
 ELSE CAST(spql.math.floor((v.value - r.low) / (r.high - r.low) * 10) AS BIGINT) END AS bucket
 FROM comparison_values v JOIN comparison_ranges r ON v.metric = r.metric
 WHERE v.value BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308
)
SELECT -1 AS metric, -1 AS bucket, CAST(NULL AS DOUBLE) AS low, CAST(NULL AS DOUBLE) AS high,
 COUNT(*) AS all_n,
 COUNT(CASE WHEN cohort = 1 THEN 1 END) AS selected_n,
 COUNT(CASE WHEN cohort = 0 THEN 1 END) AS remaining_n,
 COUNT(CASE WHEN cohort = -1 THEN 1 END) AS unknown_n,
 SUM(duration_seconds) AS all_sum,
 SUM(CASE WHEN cohort = 1 THEN duration_seconds END) AS selected_sum,
 SUM(CASE WHEN cohort = 0 THEN duration_seconds END) AS remaining_sum,
 COUNT(*) - COUNT(DISTINCT episode_id) AS invalid_count
FROM comparison_population
UNION ALL
SELECT metric, bucket, low, high, COUNT(*),
 COUNT(CASE WHEN cohort = 1 THEN 1 END), COUNT(CASE WHEN cohort = 0 THEN 1 END),
 COUNT(CASE WHEN cohort = -1 THEN 1 END), SUM(value),
 SUM(CASE WHEN cohort = 1 THEN value END), SUM(CASE WHEN cohort = 0 THEN value END), 0
FROM comparison_bins GROUP BY metric, bucket, low, high
ORDER BY metric, bucket"#
        );
        ReadOnlyQuery::new(&sql, ReadOnlyQueryMode::Workflow)
            .authorize()
            .map_err(|e| e.to_string())?;
        Ok(sql)
    }

    /// Evidence is a fresh native read, checked against the dataset version by
    /// the caller. It is never reconstructed from histogram values.
    pub fn evidence_sql(
        &self,
        population: &str,
        selection: &ComparisonEvidence,
    ) -> Result<String, String> {
        let prefix = self.query_prefix(population)?;
        let cohort = match selection.cohort {
            EvidenceCohort::Selected => "cohort = 1",
            EvidenceCohort::Remaining => "cohort = 0",
            EvidenceCohort::Unknown => "cohort = -1",
            EvidenceCohort::All => "TRUE",
        };
        let metric_filter = if let Some((metric, bucket)) = selection.bin {
            if metric >= self.metrics.len() || bucket > 9 {
                return Err("That histogram bin is unavailable.".into());
            }
            format!(
                " AND metric_{metric} BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308 AND CASE WHEN bounds.high = bounds.low THEN 0 WHEN metric_{metric} = bounds.high THEN 9 ELSE CAST(spql.math.floor((metric_{metric} - bounds.low) / (bounds.high - bounds.low) * 10) AS BIGINT) END = {bucket}"
            )
        } else {
            String::new()
        };
        let join = selection.bin.map(|(metric, _)| format!(" CROSS JOIN (SELECT MIN(metric_{metric}) AS low, MAX(metric_{metric}) AS high FROM comparison_population WHERE metric_{metric} BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308) bounds")).unwrap_or_default();
        Ok(format!(
            "{prefix}\nSELECT p.* FROM comparison_population p{join} WHERE {cohort}{metric_filter} ORDER BY episode_id LIMIT 21 OFFSET {}",
            selection.offset
        ))
    }

    fn query_prefix(&self, population: &str) -> Result<String, String> {
        self.validate_population(population)?;
        let population = population.trim().trim_end_matches(';');
        let source = self.dataset.sql_name().replace('\'', "''");
        let value = ComparisonMetric::column_expression(self.criterion.column());
        // Parse then format the threshold; user text never enters executable SQL.
        let threshold = self
            .criterion
            .threshold
            .parse::<f64>()
            .map_err(|e| e.to_string())?;
        let operator = self.criterion.operator.sql();
        let metrics = self
            .metrics
            .iter()
            .enumerate()
            .map(|(index, metric)| format!(", {} AS metric_{index}", metric.expression()))
            .collect::<String>();
        Ok(format!(
            r#"WITH comparison_measurements AS (
{population}
), comparison_population AS (
 SELECT e.id AS episode_id, e.source.uri AS source_uri, e.message_start_time,
 e.message_end_time, e.duration_ns, e.message_count,
 CAST(e.duration_ns AS DOUBLE) / 1000000000.0 AS duration_seconds,
 CASE WHEN m.episode_id IS NULL OR {value} IS NULL OR NOT ({value} BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308) THEN -1
 WHEN {value} {operator} {threshold} THEN 1 ELSE 0 END AS cohort,
 {value} AS criterion_value{metrics}
 FROM spql.mcap.episodes('{source}') e
 LEFT JOIN comparison_measurements m ON e.id = m.episode_id
)"#
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EvidenceCohort {
    #[default]
    Selected,
    Remaining,
    Unknown,
    All,
}
impl EvidenceCohort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Selected => "Selected",
            Self::Remaining => "Remaining",
            Self::Unknown => "Unknown",
            Self::All => "All episodes",
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComparisonEvidence {
    cohort: EvidenceCohort,
    bin: Option<(usize, usize)>,
    offset: usize,
}
impl ComparisonEvidence {
    pub fn new(cohort: EvidenceCohort) -> Self {
        Self {
            cohort,
            ..Self::default()
        }
    }
    pub fn with_bin(mut self, bin: Option<(usize, usize)>) -> Self {
        self.bin = bin;
        self.offset = 0;
        self
    }
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }
    pub fn offset(&self) -> usize {
        self.offset
    }
    pub fn cohort(&self) -> EvidenceCohort {
        self.cohort
    }
    pub fn bin(&self) -> Option<(usize, usize)> {
        self.bin
    }
}
