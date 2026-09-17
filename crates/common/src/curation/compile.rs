//! One recipe becomes one read-only query: the episode population, each
//! measured fact joined by episode, every enabled rule and transform judged
//! as a boolean column, and `kept` as the verdict.
use super::CurationSpec;
use super::facts::{BuiltinFact, SqlLiteral};
use super::run::{CurationOutcome, CurationTotals, CurationVerdict, EpisodeFacts};
use crate::relations::SpiralQuerySnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The compiled statements and the identities behind their positional aliases.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompiledCuration {
    roster_sql: String,
    totals_sql: String,
    facts: Vec<String>,
    rules: Vec<String>,
    transforms: Vec<String>,
}

impl CurationSpec {
    /// Compile the recipe, or explain the first thing that stops it.
    pub fn compile(&self) -> Result<CompiledCuration, String> {
        let report = self.validate();
        if let Some(error) = report.errors().first() {
            return Err(error.message().to_owned());
        }
        let dataset = self.source().ok_or("No dataset is selected.")?.sql_name();
        let mut ctes = vec![format!(
            "episodes AS (SELECT id, source.uri AS source_uri, source.size AS source_bytes, \
             message_count, channel_count, message_start_time, duration_ns \
             FROM spql.mcap.episodes({}))",
            SqlLiteral::text(&dataset)
        )];
        let mut fact_columns = Vec::new();
        let mut joins = String::new();
        for (index, fact) in self.facts().iter().enumerate() {
            let prefix = format!("f{index}");
            ctes.extend(fact.measure().sql_ctes(&prefix, &dataset));
            fact_columns.push(format!("{prefix}.value AS fact_{index}"));
            joins.push_str(&format!(" LEFT JOIN {prefix} ON {prefix}.id = e.id"));
        }
        let fact_select = if fact_columns.is_empty() {
            String::new()
        } else {
            format!(", {}", fact_columns.join(", "))
        };
        ctes.push(format!(
            "facts AS (SELECT e.id, e.source_uri, e.source_bytes, e.message_count, e.channel_count, \
             e.message_start_time, e.duration_ns{fact_select} FROM episodes e{joins})"
        ));
        let expressions = FactExpressions::for_spec(self);
        let mut judged_columns = Vec::new();
        let mut rule_ids = Vec::new();
        for rule in self.rules().iter().filter(|rule| rule.enabled()) {
            let index = rule_ids.len();
            judged_columns.push(format!(
                "{} AS rule_{index}",
                rule.condition()
                    .sql(&expressions.resolve(rule.condition().fact())?)?
            ));
            rule_ids.push(rule.id().to_owned());
        }
        let mut transform_ids = Vec::new();
        for transform in self
            .transforms()
            .iter()
            .filter(|transform| transform.enabled())
        {
            let index = transform_ids.len();
            let predicate = match transform.condition() {
                Some(condition) => condition.sql(&expressions.resolve(condition.fact())?)?,
                None => "true".to_owned(),
            };
            judged_columns.push(format!("{predicate} AS transform_{index}"));
            transform_ids.push(transform.id().to_owned());
        }
        let judged_select = if judged_columns.is_empty() {
            String::new()
        } else {
            format!(", {}", judged_columns.join(", "))
        };
        ctes.push(format!("judged AS (SELECT *{judged_select} FROM facts)"));
        let kept = if rule_ids.is_empty() {
            "true".to_owned()
        } else {
            format!(
                "NOT ({})",
                (0..rule_ids.len())
                    .map(|index| format!("COALESCE(rule_{index}, false)"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            )
        };
        ctes.push(format!(
            "verdicts AS (SELECT *, {kept} AS kept FROM judged)"
        ));
        let with = format!("WITH {}", ctes.join(",\n"));
        let roster_sql =
            format!("{with}\nSELECT * FROM verdicts ORDER BY message_start_time, source_uri, id");
        let mut totals = vec![
            "count(*) AS episodes".to_owned(),
            "sum(CASE WHEN kept THEN 1 ELSE 0 END) AS kept".to_owned(),
        ];
        totals.extend((0..rule_ids.len()).map(|index| {
            format!(
                "sum(CASE WHEN COALESCE(rule_{index}, false) THEN 1 ELSE 0 END) AS rule_{index}"
            )
        }));
        totals.extend((0..transform_ids.len()).map(|index| {
            format!(
                "sum(CASE WHEN kept AND COALESCE(transform_{index}, false) THEN 1 ELSE 0 END) AS transform_{index}"
            )
        }));
        let totals_sql = format!("{with}\nSELECT {} FROM verdicts", totals.join(", "));
        Ok(CompiledCuration {
            roster_sql,
            totals_sql,
            facts: self
                .facts()
                .iter()
                .map(|fact| fact.key().to_owned())
                .collect(),
            rules: rule_ids,
            transforms: transform_ids,
        })
    }
}

impl CompiledCuration {
    /// One row per episode, bounded by the result preview limit.
    pub fn roster_sql(&self) -> &str {
        &self.roster_sql
    }
    /// Exact population counts, for when the roster was truncated.
    pub fn totals_sql(&self) -> &str {
        &self.totals_sql
    }
    /// Read every listed episode's verdict from the roster result.
    pub fn verdicts(&self, roster: &SpiralQuerySnapshot) -> Result<Vec<CurationVerdict>, String> {
        roster.rows().iter().map(|row| self.verdict(row)).collect()
    }
    fn verdict(
        &self,
        row: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<CurationVerdict, String> {
        let column = |name: &str| row.get(name).cloned().unwrap_or(serde_json::Value::Null);
        let id = column("id")
            .as_str()
            .map(str::to_owned)
            .ok_or("Roster row has no episode id")?;
        let episode: EpisodeFacts = serde_json::from_value(serde_json::json!({
            "id": id,
            "source_uri": column("source_uri"),
            "source_bytes": column("source_bytes"),
            "message_count": column("message_count"),
            "channel_count": column("channel_count"),
            "message_start_time": column("message_start_time"),
            "duration_ns": column("duration_ns"),
        }))
        .map_err(|error| format!("Reading episode statistics: {error}"))?;
        let mut verdict = CurationVerdict::new(episode);
        for (index, key) in self.facts.iter().enumerate() {
            verdict = verdict.with_fact(key.clone(), column(&format!("fact_{index}")));
        }
        for (index, id) in self.rules.iter().enumerate() {
            if column(&format!("rule_{index}")).as_bool() == Some(true) {
                verdict = verdict.with_exclusion(id.clone());
            }
        }
        if verdict.kept() {
            for (index, id) in self.transforms.iter().enumerate() {
                if column(&format!("transform_{index}")).as_bool() == Some(true) {
                    verdict = verdict.with_transform(id.clone());
                }
            }
        }
        Ok(verdict)
    }
    /// Assemble the outcome. Totals come from the population query when the
    /// roster was bounded, otherwise from the verdicts themselves.
    pub fn outcome(
        &self,
        roster: &SpiralQuerySnapshot,
        totals: Option<&SpiralQuerySnapshot>,
    ) -> Result<CurationOutcome, String> {
        let verdicts = self.verdicts(roster)?;
        let listed = (verdicts.len() as u64, roster.truncated());
        let (totals, rule_counts, transform_counts) = match totals {
            Some(totals) => {
                let row = totals
                    .rows()
                    .first()
                    .ok_or("The population totals returned no row")?;
                let count = |name: &str| {
                    row.get(name)
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                };
                (
                    CurationTotals::new(count("episodes"), count("kept"), listed),
                    self.rules
                        .iter()
                        .enumerate()
                        .map(|(index, id)| (id.clone(), count(&format!("rule_{index}"))))
                        .collect(),
                    self.transforms
                        .iter()
                        .enumerate()
                        .map(|(index, id)| (id.clone(), count(&format!("transform_{index}"))))
                        .collect(),
                )
            }
            None => {
                let kept = verdicts.iter().filter(|verdict| verdict.kept()).count() as u64;
                let mut rule_counts: BTreeMap<String, u64> = BTreeMap::new();
                let mut transform_counts: BTreeMap<String, u64> = BTreeMap::new();
                for verdict in &verdicts {
                    for rule in verdict.excluded_by() {
                        *rule_counts.entry(rule.clone()).or_default() += 1;
                    }
                    for transform in verdict.transforms() {
                        *transform_counts.entry(transform.clone()).or_default() += 1;
                    }
                }
                (
                    CurationTotals::new(listed.0, kept, listed),
                    rule_counts,
                    transform_counts,
                )
            }
        };
        Ok(CurationOutcome::new(totals, verdicts)
            .with_counts(rule_counts, transform_counts)
            .with_elapsed_ms(roster.elapsed_ms()))
    }
}

/// Where each fact lives in the `facts` CTE.
struct FactExpressions {
    declared: Vec<String>,
}

impl FactExpressions {
    fn for_spec(spec: &CurationSpec) -> Self {
        Self {
            declared: spec
                .facts()
                .iter()
                .map(|fact| fact.key().to_owned())
                .collect(),
        }
    }
    fn resolve(&self, key: &str) -> Result<String, String> {
        if let Some(builtin) = BuiltinFact::from_key(key) {
            return Ok(builtin.sql_expression().to_owned());
        }
        self.declared
            .iter()
            .position(|declared| declared == key)
            .map(|index| format!("fact_{index}"))
            .ok_or_else(|| format!("Rule names an unknown fact “{key}”."))
    }
}
