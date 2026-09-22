//! One recipe becomes one read-only query: the episode population, each
//! measured fact joined by episode, every rule and transform judged
//! as a boolean column, and `kept` as the verdict. Every roster row also
//! carries the population's totals and per-stage counts as window
//! aggregates, so a bounded roster still reports exact counts, and preview
//! and run judge the same way.
use spiral_robotics_common::{
    curation::{
        CurationSpec, CurationStep,
        facts::BuiltinFact,
        rules::TransformOperation,
        run::{
            CurationOutcome, CurationStageImpact, CurationTotals, CurationVerdict, EpisodeFacts,
        },
    },
    relations::SpiralQuerySnapshot,
    sql::SqlLiteral,
};
use std::collections::BTreeMap;

mod expressions;
use expressions::{ConditionSql, FactSql};

/// Backend-owned SQL generation for a shared, validated recipe.
pub(super) struct CurationSql<'a> {
    recipe: &'a CurationSpec,
}
impl<'a> CurationSql<'a> {
    pub(super) fn new(recipe: &'a CurationSpec) -> Self {
        Self { recipe }
    }
}

/// The compiled statements and the identities behind their positional aliases.
pub(super) struct CompiledCuration {
    /// The CTEs that end in the judged `verdicts` population.
    with: String,
    /// Window aggregates over the whole population, added to roster rows.
    summary: String,
    facts: Vec<String>,
    rules: Vec<String>,
    transforms: Vec<String>,
}

impl CurationSql<'_> {
    /// Compile the recipe, or explain the first thing that stops it.
    pub(super) fn compile(&self) -> Result<CompiledCuration, String> {
        let report = self.recipe.validate();
        if let Some(error) = report.errors().first() {
            return Err(error.message().to_owned());
        }
        let dataset = self
            .recipe
            .source()
            .ok_or("No dataset is selected.")?
            .sql_name();
        let mut ctes = vec![format!(
            "episodes AS (SELECT id, source.uri AS source_uri, source.size AS source_bytes, \
             message_count, channel_count, message_start_time, duration_ns \
             FROM spql.mcap.episodes({}))",
            SqlLiteral::text(&dataset)
        )];
        let mut fact_columns = Vec::new();
        let mut joins = String::new();
        for (index, fact) in self.recipe.facts().iter().enumerate() {
            let prefix = format!("f{index}");
            ctes.extend(FactSql(fact.measure()).sql_ctes(&prefix, &dataset));
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
        let expressions = FactExpressions::for_spec(self.recipe);
        let mut judged_columns = Vec::new();
        let mut rule_ids = Vec::new();
        for rule in self.recipe.rules().iter() {
            let index = rule_ids.len();
            judged_columns.push(format!(
                "{} AS rule_{index}",
                ConditionSql(rule.condition())
                    .sql(&expressions.resolve(rule.condition().fact())?)?
            ));
            rule_ids.push(rule.id().to_owned());
        }
        let mut transform_ids = Vec::new();
        for transform in self.recipe.transforms().iter() {
            let index = transform_ids.len();
            let predicate = match transform.condition() {
                Some(condition) => {
                    ConditionSql(condition).sql(&expressions.resolve(condition.fact())?)?
                }
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
        let summary = self.summary_columns(&rule_ids, &transform_ids).join(", ");
        Ok(CompiledCuration {
            with,
            summary,
            facts: self
                .recipe
                .facts()
                .iter()
                .map(|fact| fact.key().to_owned())
                .collect(),
            rules: rule_ids,
            transforms: transform_ids,
        })
    }

    /// Population totals, then each step's incoming, reached, and retained
    /// episodes, as window aggregates over every judged row.
    fn summary_columns(&self, rules: &[String], transforms: &[String]) -> Vec<String> {
        let count = |predicate: &str, alias: String| {
            format!("sum(CASE WHEN {predicate} THEN 1 ELSE 0 END) OVER () AS {alias}")
        };
        let mut columns = vec![
            "count(*) OVER () AS total_episodes".to_owned(),
            count("kept", "total_kept".into()),
        ];
        columns.extend((0..rules.len()).map(|index| {
            count(
                &format!("COALESCE(rule_{index}, false)"),
                format!("total_rule_{index}"),
            )
        }));
        columns.extend((0..transforms.len()).map(|index| {
            count(
                &format!("kept AND COALESCE(transform_{index}, false)"),
                format!("total_transform_{index}"),
            )
        }));
        let mut retained = "true".to_owned();
        for (index, step) in self.recipe.steps().iter().enumerate() {
            columns.push(count(&retained, format!("stage_{index}_incoming")));
            if let CurationStep::Transform(transform) = step
                && let Some(position) = transforms.iter().position(|id| id == transform.id())
            {
                columns.push(count(
                    &format!("{retained} AND COALESCE(transform_{position}, false)"),
                    format!("stage_{index}_affected"),
                ));
            }
            if let CurationStep::Filter(rule) = step
                && let Some(position) = rules.iter().position(|id| id == rule.id())
            {
                retained = format!("{retained} AND NOT COALESCE(rule_{position}, false)");
            }
            columns.push(count(&retained, format!("stage_{index}_retained")));
        }
        columns
    }
}

impl CompiledCuration {
    /// The judged population, one row per episode, with the population
    /// summary on every row. The preview bounds the rows it reads.
    pub(super) fn roster_sql(&self) -> String {
        format!(
            "{}\nSELECT *, {} FROM verdicts ORDER BY message_start_time, source_uri, id",
            self.with, self.summary
        )
    }
    /// Materialize the full judged population, including excluded rows and
    /// the exact submitted recipe. Preview limits never define training data.
    pub(super) fn materialize_sql(
        &self,
        relation: &str,
        recipe: &CurationSpec,
    ) -> Result<String, String> {
        let definition = serde_json::to_string(recipe).map_err(|error| error.to_string())?;
        Ok(format!(
            "CREATE TABLE {} AS SELECT judged.*, {} AS curation_recipe FROM ({}\nSELECT * FROM verdicts) judged",
            CurationSql::quoted_relation(relation),
            SqlLiteral::text(&definition),
            self.with
        ))
    }
    /// The materialized population read back like the preview roster.
    pub(super) fn output_roster_sql(&self, relation: &str) -> String {
        format!(
            "SELECT *, {} FROM {} ORDER BY message_start_time, source_uri, id",
            self.summary,
            CurationSql::quoted_relation(relation)
        )
    }
    /// Each step's episode counts, from the summary on the roster's rows.
    pub(super) fn stage_impacts(
        &self,
        recipe: &CurationSpec,
        roster: &SpiralQuerySnapshot,
    ) -> Vec<CurationStageImpact> {
        let summary = RosterSummary(roster.rows().first());
        recipe
            .steps()
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let mut impact = CurationStageImpact::new(
                    step.id().to_owned(),
                    summary.count(&format!("stage_{index}_incoming")),
                    summary.count(&format!("stage_{index}_retained")),
                );
                if matches!(step, CurationStep::Transform(_)) {
                    impact.set_affected_episodes(summary.count(&format!("stage_{index}_affected")));
                }
                impact
            })
            .collect()
    }
    /// Read every listed episode's verdict from the roster result.
    fn verdicts(&self, roster: &SpiralQuerySnapshot) -> Result<Vec<CurationVerdict>, String> {
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
        for (index, id) in self.transforms.iter().enumerate() {
            if column(&format!("transform_{index}")).as_bool() == Some(true) {
                verdict = verdict.with_eligible_transform(id.clone());
                if verdict.kept() {
                    verdict = verdict.with_transform(id.clone());
                }
            }
        }
        Ok(verdict)
    }
    /// Assemble the outcome: the listed verdicts, and exact totals from the
    /// population summary even when the roster was bounded.
    pub(super) fn outcome(&self, roster: &SpiralQuerySnapshot) -> Result<CurationOutcome, String> {
        let verdicts = self.verdicts(roster)?;
        let summary = RosterSummary(roster.rows().first());
        let totals = CurationTotals::new(
            summary.count("total_episodes"),
            summary.count("total_kept"),
            (verdicts.len() as u64, roster.truncated()),
        );
        let rule_counts = self
            .rules
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), summary.count(&format!("total_rule_{index}"))))
            .collect();
        let transform_counts = self
            .transforms
            .iter()
            .enumerate()
            .map(|(index, id)| {
                (
                    id.clone(),
                    summary.count(&format!("total_transform_{index}")),
                )
            })
            .collect();
        Ok(CurationOutcome::new(totals, verdicts)
            .with_counts(rule_counts, transform_counts)
            .with_elapsed_ms(roster.elapsed_ms()))
    }
}

/// The population summary every roster row repeats; an empty population
/// has no rows and counts zero.
struct RosterSummary<'a>(Option<&'a serde_json::Map<String, serde_json::Value>>);
impl RosterSummary<'_> {
    fn count(&self, column: &str) -> u64 {
        self.0
            .and_then(|row| row.get(column))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
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
            return Ok(match builtin {
                BuiltinFact::DurationSeconds => "duration_ns / 1000000000.0",
                BuiltinFact::Messages => "message_count",
                BuiltinFact::Channels => "channel_count",
                BuiltinFact::SizeMegabytes => "source_bytes / 1048576.0",
                BuiltinFact::SourcePath => "source_uri",
            }
            .to_owned());
        }
        self.declared
            .iter()
            .position(|declared| declared == key)
            .map(|index| format!("fact_{index}"))
            .ok_or_else(|| format!("Rule names an unknown fact “{key}”."))
    }
}

pub(super) struct DerivedOutput {
    relation: String,
    statement: String,
}
impl DerivedOutput {
    pub(super) fn relation(&self) -> &str {
        &self.relation
    }
    pub(super) fn statement(&self) -> &str {
        &self.statement
    }
}

impl CurationSql<'_> {
    pub(super) fn derived_outputs(&self, roster: &str) -> Result<Vec<DerivedOutput>, String> {
        let dataset = self
            .recipe
            .source()
            .ok_or("No dataset is selected.")?
            .sql_name();
        let mut cameras = BTreeMap::<String, Vec<(usize, &TransformOperation)>>::new();
        for (index, transform) in self.recipe.transforms().iter().enumerate() {
            if let Some(topic) = transform.operation().topic() {
                cameras
                    .entry(topic.topic().to_owned())
                    .or_default()
                    .push((index, transform.operation()));
            }
        }
        let mut outputs = Vec::new();
        let time_ns = "spql.dt.duration_total(f.time, 'ns')";
        let time_predicate = self.time_predicate(time_ns, &format!("{time_ns} + 1"));
        let progress_predicates = self
            .recipe
            .transforms()
            .iter()
            .enumerate()
            .filter(|(_, transform)| {
                matches!(
                    transform.operation(),
                    TransformOperation::SelectProgressChunks { .. }
                )
            })
            .map(|(index, _)| format!("NOT EXISTS (SELECT 1 FROM {} c WHERE c.id = s.id AND NOT c.kept AND f.request_index >= c.start_frame AND f.request_index < c.end_frame)", Self::quoted_relation(&format!("{roster}_chunks_{index}"))))
            .collect::<Vec<_>>();
        let selection = std::iter::once(time_predicate)
            .chain(progress_predicates)
            .collect::<Vec<_>>()
            .join(" AND ");
        for (index, (topic, operations)) in cameras.into_iter().enumerate() {
            let mut image = "spql.video.decode(f.frame)".to_owned();
            for (transform, operation) in operations {
                image = format!(
                    "CASE WHEN COALESCE(v.transform_{transform}, false) THEN {} ELSE {image} END",
                    operation.image_sql(&image)?
                );
            }
            let relation = format!("{roster}_camera_{index}");
            let statement = format!(
                "CREATE VIEW {} AS WITH packets AS (SELECT r.id, r.value.data AS payload, spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(CAST(v.message_start_time AS BIGINT), 'ns') AS presentation_time, \
                CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal)-1 AS BIGINT) AS decode_index \
                FROM spql.mcap.read_topic({}, {}) r JOIN {} v ON r.id = v.id WHERE v.kept), \
                streams AS (SELECT id, spql.video.from_packets(spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source FROM packets GROUP BY id) \
                SELECT s.id, {} AS topic, f.request_index AS frame_index, f.time, f.duration, {image} AS image \
                FROM streams s JOIN {} v ON s.id = v.id CROSS JOIN LATERAL spql.video.frames(s.source, spql.video.frame_slice()) f WHERE {selection}",
                Self::quoted_relation(&relation),
                SqlLiteral::text(&dataset),
                SqlLiteral::text(&topic),
                Self::quoted_relation(roster),
                SqlLiteral::text(&topic),
                Self::quoted_relation(roster)
            );
            outputs.push(DerivedOutput {
                relation,
                statement,
            });
        }
        if self
            .recipe
            .transforms()
            .iter()
            .any(|transform| matches!(transform.operation(), TransformOperation::TrimTime { .. }))
        {
            let relation = format!("{roster}_time_windows");
            let mut lower = "0".to_owned();
            let mut upper = "v.duration_ns + 1".to_owned();
            for (index, transform) in self.recipe.transforms().iter().enumerate() {
                let TransformOperation::TrimTime {
                    start_seconds,
                    end_seconds,
                } = transform.operation()
                else {
                    continue;
                };
                let start = format!("{:.0}", start_seconds * 1e9);
                lower = format!(
                    "CASE WHEN COALESCE(v.transform_{index}, false) AND {start} > ({lower}) THEN {start} ELSE ({lower}) END"
                );
                if let Some(end) = end_seconds {
                    let end = format!("{:.0}", end * 1e9);
                    upper = format!(
                        "CASE WHEN COALESCE(v.transform_{index}, false) AND {end} < ({upper}) THEN {end} ELSE ({upper}) END"
                    );
                }
            }
            let statement = format!(
                "CREATE VIEW {} AS SELECT id,start_ns,end_ns,episode_kept AND start_ns < end_ns AS kept FROM (SELECT v.id, {lower} AS start_ns, {upper} AS end_ns, v.kept AS episode_kept FROM {} v) windows",
                Self::quoted_relation(&relation),
                Self::quoted_relation(roster)
            );
            outputs.push(DerivedOutput {
                relation,
                statement,
            });
        }
        Ok(outputs)
    }
    /// Intersect half-open time windows. The end expression is
    /// exclusive, so image samples pass their timestamp plus one nanosecond.
    pub(super) fn time_predicate(&self, start: &str, end: &str) -> String {
        self.time_predicate_for((start, end), None)
    }
    pub(super) fn time_predicate_before(&self, step_id: &str, bounds: (&str, &str)) -> String {
        self.time_predicate_for(bounds, Some(step_id))
    }
    fn time_predicate_for(&self, (start, end): (&str, &str), before: Option<&str>) -> String {
        let eligible = self
            .recipe
            .steps()
            .iter()
            .take_while(|step| before != Some(step.id()))
            .map(|step| step.id().to_owned())
            .collect::<Vec<_>>();
        let clauses = self
            .recipe
            .transforms()
            .iter()
            .enumerate()
            .filter_map(|(index, transform)| {
                if !eligible.iter().any(|id| id == transform.id()) {
                    return None;
                }
                let TransformOperation::TrimTime {
                    start_seconds,
                    end_seconds,
                } = transform.operation()
                else {
                    return None;
                };
                let from = format!("{start} >= {:.0}", start_seconds * 1e9);
                let bounds = end_seconds.map_or(from.clone(), |until| {
                    format!("{from} AND {end} <= {:.0}", until * 1e9)
                });
                Some(format!(
                    "(NOT COALESCE(v.transform_{index}, false) OR ({bounds}))"
                ))
            })
            .collect::<Vec<_>>();
        if clauses.is_empty() {
            "true".into()
        } else {
            clauses.join(" AND ")
        }
    }
    fn quoted_relation(relation: &str) -> String {
        format!("\"{}\"", relation.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests;
