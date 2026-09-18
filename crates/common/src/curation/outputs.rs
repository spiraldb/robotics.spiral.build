//! Native, immutable run outputs. The roster stores decisions and the exact
//! recipe; camera views derive pixels lazily from the selected source frames.
use super::{CurationSpec, facts::SqlLiteral};
use std::collections::BTreeMap;

pub struct DerivedOutput {
    relation: String,
    statement: String,
}
impl DerivedOutput {
    pub fn relation(&self) -> &str {
        &self.relation
    }
    pub fn statement(&self) -> &str {
        &self.statement
    }
}

impl CurationSpec {
    pub fn derived_outputs(&self, roster: &str) -> Result<Vec<DerivedOutput>, String> {
        let dataset = self.source().ok_or("No dataset is selected.")?.sql_name();
        let mut cameras =
            BTreeMap::<String, Vec<(usize, &super::rules::TransformOperation)>>::new();
        for (index, transform) in self.transforms().iter().enumerate() {
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
        let progress_predicates = self.transforms().iter().enumerate().filter_map(|(index, transform)| matches!(transform.operation(), super::rules::TransformOperation::SelectProgressChunks { .. }).then(|| format!("NOT EXISTS (SELECT 1 FROM {} c WHERE c.id = s.id AND NOT c.kept AND f.request_index >= c.start_frame AND f.request_index < c.end_frame)", Self::quoted_relation(&format!("{roster}_chunks_{index}"))))).collect::<Vec<_>>();
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
        if self.transforms().iter().any(|transform| {
            matches!(
                transform.operation(),
                super::rules::TransformOperation::TrimTime { .. }
            )
        }) {
            let relation = format!("{roster}_time_windows");
            let mut lower = "0".to_owned();
            let mut upper = "v.duration_ns + 1".to_owned();
            for (index, transform) in self.transforms().iter().enumerate() {
                let super::rules::TransformOperation::TrimTime {
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
    pub fn time_predicate(&self, start: &str, end: &str) -> String {
        self.time_predicate_for((start, end), None)
    }
    pub fn time_predicate_before(&self, step_id: &str, bounds: (&str, &str)) -> String {
        self.time_predicate_for(bounds, Some(step_id))
    }
    fn time_predicate_for(&self, (start, end): (&str, &str), before: Option<&str>) -> String {
        let eligible = self
            .steps()
            .iter()
            .take_while(|step| before != Some(step.id()))
            .map(|step| step.id().to_owned())
            .collect::<Vec<_>>();
        let clauses = self
            .transforms()
            .iter()
            .enumerate()
            .filter_map(|(index, transform)| {
                if !eligible.iter().any(|id| id == transform.id()) {
                    return None;
                }
                let super::rules::TransformOperation::TrimTime {
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
    pub(super) fn quoted_relation(relation: &str) -> String {
        format!("\"{}\"", relation.replace('"', "\"\""))
    }
}
