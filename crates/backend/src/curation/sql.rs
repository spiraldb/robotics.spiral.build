//! One recipe becomes one read-only query: the episode population, each
//! measured fact joined by episode, every rule and transform judged
//! as a boolean column, and `kept` as the verdict.
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
    roster_sql: String,
    totals_sql: String,
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
                .recipe
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
    /// Materialize the full judged population, including excluded rows and
    /// the exact submitted recipe. Preview limits never define training data.
    pub(super) fn materialize_sql(
        &self,
        relation: &str,
        recipe: &CurationSpec,
    ) -> Result<String, String> {
        let definition = serde_json::to_string(recipe).map_err(|error| error.to_string())?;
        Ok(format!(
            "CREATE TABLE \"{}\" AS SELECT judged.*, {} AS curation_recipe FROM ({}) judged",
            relation.replace('"', "\"\""),
            SqlLiteral::text(&definition),
            self.roster_sql
        ))
    }
    pub(super) fn output_roster_sql(&self, relation: &str) -> String {
        format!(
            "SELECT * FROM \"{}\" ORDER BY message_start_time, source_uri, id",
            relation.replace('"', "\"\"")
        )
    }
    pub(super) fn output_totals_sql(&self, relation: &str) -> String {
        let select = self
            .totals_sql
            .rsplit_once("\nSELECT ")
            .map(|(_, select)| select)
            .expect("compiled totals end in a SELECT");
        format!(
            "SELECT {}",
            select.replace(
                "FROM verdicts",
                &format!("FROM \"{}\"", relation.replace('"', "\"\""))
            )
        )
    }
    pub(super) fn stage_totals_sql(&self, relation: &str, recipe: &CurationSpec) -> String {
        let mut retained = "true".to_owned();
        let mut columns = Vec::new();
        for (index, step) in recipe.steps().iter().enumerate() {
            columns.push(format!(
                "sum(CASE WHEN {retained} THEN 1 ELSE 0 END) AS stage_{index}_incoming"
            ));
            if let CurationStep::Transform(transform) = step {
                if let Some(transform_index) =
                    self.transforms.iter().position(|id| id == transform.id())
                {
                    columns.push(format!("sum(CASE WHEN {retained} AND COALESCE(transform_{transform_index},false) THEN 1 ELSE 0 END) AS stage_{index}_affected"));
                }
            }
            if let CurationStep::Filter(rule) = step {
                if let Some(rule_index) = self.rules.iter().position(|id| id == rule.id()) {
                    retained = format!("{retained} AND NOT COALESCE(rule_{rule_index},false)");
                }
            }
            columns.push(format!(
                "sum(CASE WHEN {retained} THEN 1 ELSE 0 END) AS stage_{index}_retained"
            ));
        }
        if columns.is_empty() {
            columns.push("count(*) AS episodes".into());
        }
        format!(
            "SELECT {} FROM {}",
            columns.join(", "),
            CurationSql::quoted_relation(relation)
        )
    }
    pub(super) fn stage_impacts(
        &self,
        recipe: &CurationSpec,
        totals: &SpiralQuerySnapshot,
    ) -> Result<Vec<CurationStageImpact>, String> {
        let row = totals
            .rows()
            .first()
            .ok_or("The stage totals returned no row.")?;
        Ok(recipe
            .steps()
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let count = |suffix| {
                    row.get(&format!("stage_{index}_{suffix}"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                };
                let mut impact = CurationStageImpact::new(
                    step.id().to_owned(),
                    count("incoming"),
                    count("retained"),
                );
                if matches!(step, CurationStep::Transform(_)) {
                    impact.set_affected_episodes(count("affected"));
                }
                impact
            })
            .collect())
    }
    /// One row per episode, bounded by the result preview limit.
    pub(super) fn roster_sql(&self) -> &str {
        &self.roster_sql
    }
    /// Exact population counts, for when the roster was truncated.
    pub(super) fn totals_sql(&self) -> &str {
        &self.totals_sql
    }
    /// Read every listed episode's verdict from the roster result.
    pub(super) fn verdicts(
        &self,
        roster: &SpiralQuerySnapshot,
    ) -> Result<Vec<CurationVerdict>, String> {
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
    /// Assemble the outcome. Totals come from the population query when the
    /// roster was bounded, otherwise from the verdicts themselves.
    pub(super) fn outcome(
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
        let progress_predicates = self.recipe.transforms().iter().enumerate().filter_map(|(index, transform)| matches!(transform.operation(), TransformOperation::SelectProgressChunks { .. }).then(|| format!("NOT EXISTS (SELECT 1 FROM {} c WHERE c.id = s.id AND NOT c.kept AND f.request_index >= c.start_frame AND f.request_index < c.end_frame)", Self::quoted_relation(&format!("{roster}_chunks_{index}"))))).collect::<Vec<_>>();
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
