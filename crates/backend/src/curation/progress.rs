//! Explicit whole-recipe execution uses transient native scores and persists only final chunk decisions.
use super::sql::CurationSql;
use spiral_robotics_common::curation::{
    CurationSpec, progress::ProgressSelection, rules::TransformOperation,
};

pub(super) struct ProgressOutput {
    transform_index: usize,
    transform_id: String,
    selection: ProgressSelection,
}
impl ProgressOutput {
    pub(super) fn for_spec(spec: &CurationSpec) -> Result<Vec<Self>, String> {
        spec.transforms()
            .iter()
            .enumerate()
            .filter_map(|(transform_index, transform)| {
                let TransformOperation::SelectProgressChunks { selection } = transform.operation()
                else {
                    return None;
                };
                Some(selection.validate().map(|()| Self {
                    transform_index,
                    transform_id: transform.id().to_owned(),
                    selection: selection.clone(),
                }))
            })
            .collect()
    }
    pub(super) fn relation(&self, roster: &str) -> String {
        format!("{roster}_chunks_{}", self.transform_index)
    }
    pub(super) fn transform_id(&self) -> &str {
        &self.transform_id
    }
    pub(super) fn materialize_sql(&self, roster: &str, spec: &CurationSpec) -> String {
        let index = self.transform_index;
        let threshold = self.selection.minimum_velocity();
        let time = CurationSql::new(spec).time_predicate("c.start_ns", "c.end_ns");
        let entering = spec.episode_predicate_before(&self.transform_id);
        let prior_time = CurationSql::new(spec)
            .time_predicate_before(&self.transform_id, ("c.start_ns", "c.end_ns"));
        let prior_ids = spec
            .steps()
            .iter()
            .take_while(|step| step.id() != self.transform_id)
            .map(|step| step.id().to_owned())
            .collect::<Vec<_>>();
        let prior = spec.transforms().iter().enumerate().filter(|(_, transform)| prior_ids.iter().any(|id| id == transform.id()) && matches!(transform.operation(), TransformOperation::SelectProgressChunks { .. })).map(|(index, _)| format!("NOT EXISTS (SELECT 1 FROM \"{roster}_chunks_{index}\" previous WHERE previous.id = c.id AND previous.stage_incoming AND NOT previous.stage_retained AND previous.start_frame < c.end_frame AND previous.end_frame > c.start_frame)")).collect::<Vec<_>>();
        let prior = if prior.is_empty() {
            "true".to_owned()
        } else {
            prior.join(" AND ")
        };
        format!(
            "CREATE TABLE \"{}\" AS WITH dense_scores AS ({}), decisions AS (SELECT c.*, {threshold} AS minimum_velocity, {} AS chunk_frames, '{}' AS score_method, v.kept AS episode_kept, ({entering}) AND ({prior_time}) AND ({prior}) AS stage_incoming, ({prior}) AND ({time}) AS allowed_by_other_transforms, (NOT COALESCE(v.transform_{index},false) OR c.mean_velocity IS NULL OR c.support IS NULL OR c.support = 0 OR c.mean_velocity >= {threshold}) AS meets_progress, c.mean_velocity IS NULL OR c.support IS NULL OR c.support = 0 AS needs_review FROM ({}) c JOIN \"{roster}\" v ON c.id = v.id) SELECT *, stage_incoming AND meets_progress AS stage_retained, episode_kept AND allowed_by_other_transforms AND meets_progress AS kept FROM decisions",
            self.relation(roster),
            self.selection.run_score_query(roster, &entering),
            self.selection.chunk_frames(),
            self.selection.scorer().version(),
            self.selection.chunks_sql("dense_scores")
        )
    }
    pub(super) fn coverage_sql(&self, roster: &str) -> String {
        format!(
            "SELECT DISTINCT id FROM \"{}\" WHERE stage_incoming ORDER BY id",
            self.relation(roster)
        )
    }
    pub(super) fn effect_sql(&self, roster: &str) -> String {
        format!(
            "SELECT sum(CASE WHEN stage_retained THEN end_ns-start_ns ELSE 0 END) AS retained_ns, sum(CASE WHEN stage_incoming AND NOT stage_retained THEN end_ns-start_ns ELSE 0 END) AS excluded_ns FROM \"{}\"",
            self.relation(roster)
        )
    }
}
