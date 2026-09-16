//! Validate result claims against retained native rows, without rerunning inference.
use serde_json::{Map, Value, json};
use spiral_robotics_common::{relations::SpiralQuerySnapshot, workflow::document::WorkflowRun};
use std::collections::BTreeSet;

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResultKind {
    Result,
    EpisodeFilter,
    Diagnostic,
}

impl ResultKind {
    pub(super) fn is_diagnostic(self) -> bool {
        matches!(self, Self::Diagnostic)
    }

    pub(super) fn verify(self, run: &WorkflowRun) -> Result<Value, String> {
        let result = run
            .snapshot()
            .result()
            .ok_or("This run has no result preview.")?;
        let counts = if matches!(self, Self::EpisodeFilter) {
            EpisodeCounts::verify(result)?
        } else {
            None
        };
        Ok(json!({
            "kind": self,
            "run_id": run.snapshot().run_id(),
            "preview_rows": result.rows().len(),
            "preview_truncated": result.truncated(),
            "total_result_rows": (!result.truncated()).then_some(result.rows().len()),
            "cohort_counts": counts,
            "notice": if self.is_diagnostic() {
                "This is diagnostic evidence. The method was not validated; do not claim successful filtering."
            } else if result.truncated() {
                "Checks cover retained preview rows and count consistency only. Unretained rows were not inspected."
            } else if matches!(self, Self::EpisodeFilter) && counts.is_none() {
                "The filtered result is empty. Cohort and excluded counts are unavailable; do not infer them from zero returned rows."
            } else {
                "Report these result facts. Row/count consistency does not establish visual classification accuracy."
            }
        }))
    }
}

#[derive(PartialEq, serde::Serialize)]
struct EpisodeCounts {
    total_episodes: u64,
    excluded_count: u64,
    retained_count: u64,
    unknown_count: u64,
}

impl EpisodeCounts {
    fn from_row(row: &Map<String, Value>) -> Result<Self, String> {
        let count = |name: &str| {
            row.get(name).and_then(Value::as_u64).ok_or_else(|| {
                format!("Episode filters require a nonnegative integer {name} column.")
            })
        };
        let counts = Self {
            total_episodes: count("total_episodes")?,
            excluded_count: count("excluded_count")?,
            retained_count: count("retained_count")?,
            unknown_count: count("unknown_count")?,
        };
        if counts
            .excluded_count
            .checked_add(counts.retained_count)
            .and_then(|sum| sum.checked_add(counts.unknown_count))
            != Some(counts.total_episodes)
        {
            return Err("Episode counts must satisfy total_episodes = excluded_count + retained_count + unknown_count.".into());
        }
        Ok(counts)
    }

    fn verify(result: &SpiralQuerySnapshot) -> Result<Option<Self>, String> {
        let Some(first) = result.rows().first() else {
            return Ok(None);
        };
        let counts = Self::from_row(first)?;
        let mut ids = BTreeSet::new();
        let mut retained = 0_u64;
        let mut unknown = 0_u64;
        for row in result.rows() {
            let id = row
                .get("episode_id")
                .filter(|id| matches!(id, Value::String(s) if !s.is_empty()) || id.is_number())
                .ok_or("Episode filters require a non-null episode_id for every row.")?;
            if !ids.insert(id.to_string()) {
                return Err("Episode filters must contain one row per episode_id.".into());
            }
            match row.get("decision").and_then(Value::as_str) {
                Some("retained") => retained += 1,
                Some("unknown") => unknown += 1,
                _ => return Err("Episode filters may return only decision='retained' or 'unknown'. Remove excluded rows in SQL before presenting the filter.".into()),
            }
            if Self::from_row(row)? != counts {
                return Err("Cohort counts disagree between result rows.".into());
            }
        }
        if retained > counts.retained_count
            || unknown > counts.unknown_count
            || (!result.truncated()
                && (retained != counts.retained_count || unknown != counts.unknown_count))
        {
            return Err(
                "Retained/unknown counts do not agree with the returned episode decisions.".into(),
            );
        }
        Ok(Some(counts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FilterFixture;
    impl FilterFixture {
        fn row(id: &str, decision: &str) -> Map<String, Value> {
            serde_json::from_value(json!({"episode_id":id,"decision":decision,
                "total_episodes":4,"excluded_count":2,"retained_count":1,"unknown_count":1}))
            .unwrap()
        }
        fn preview(rows: Vec<Map<String, Value>>, truncated: bool) -> SpiralQuerySnapshot {
            SpiralQuerySnapshot::new(Vec::new(), rows, truncated)
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn filter_contract_rejects_excluded_duplicate_and_unaccounted_rows() {
        let retained = FilterFixture::row("one", "retained");
        let unknown = FilterFixture::row("two", "unknown");
        assert!(
            EpisodeCounts::verify(&FilterFixture::preview(
                vec![retained.clone(), unknown],
                false
            ))
            .is_ok()
        );
        for rows in [
            vec![retained.clone(), FilterFixture::row("two", "excluded")],
            vec![retained.clone(), FilterFixture::row("one", "unknown")],
            vec![retained.clone()],
        ] {
            assert!(EpisodeCounts::verify(&FilterFixture::preview(rows, false)).is_err());
        }
        assert!(EpisodeCounts::verify(&FilterFixture::preview(vec![retained], true)).is_ok());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn filter_counts_must_be_integers_reconcile_and_agree_between_rows() {
        for invalid in [
            Value::Null,
            json!(-1),
            json!(1.5),
            json!(u64::MAX),
            json!(7),
        ] {
            let mut row = FilterFixture::row("one", "retained");
            row.insert("excluded_count".into(), invalid);
            assert!(EpisodeCounts::verify(&FilterFixture::preview(vec![row], true)).is_err());
        }
        let retained = FilterFixture::row("one", "retained");
        let mut unknown = FilterFixture::row("two", "unknown");
        unknown.insert("excluded_count".into(), json!(3));
        unknown.insert("total_episodes".into(), json!(5));
        assert!(
            EpisodeCounts::verify(&FilterFixture::preview(vec![retained, unknown], false)).is_err()
        );
        assert!(
            EpisodeCounts::verify(&FilterFixture::preview(Vec::new(), false))
                .unwrap()
                .is_none(),
            "zero returned rows do not establish population/excluded counts"
        );
    }
}
