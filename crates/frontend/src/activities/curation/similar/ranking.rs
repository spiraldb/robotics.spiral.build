//! Reading a search's Noul answers into one ordered list.
//!
//! Noul probabilities are comparable across requests, so the batches of one
//! search sort into a single ranking. Every row of the table appears in it:
//! the design has no cutoff, and a row jev did not answer for is marked and
//! shown last rather than quietly dropped.
use super::questions::QuestionBatch;
use crate::client::decisions::DecisionResponse;

/// One row's standing in a search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RankedRow {
    row: usize,
    probability: f64,
    answered: bool,
}
impl RankedRow {
    /// The row's index in the source table.
    pub(super) fn row(&self) -> usize {
        self.row
    }
    pub(super) fn probability(&self) -> f64 {
        self.probability
    }
    pub(super) fn answered(&self) -> bool {
        self.answered
    }
}

/// Every row of one search, in table order until asked for the ranking.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SearchRanking {
    rows: Vec<RankedRow>,
}
impl SearchRanking {
    /// A ranking of `rows` rows before any answer has arrived.
    pub(super) fn of(rows: usize) -> Self {
        Self {
            rows: (0..rows)
                .map(|row| RankedRow {
                    row,
                    probability: 0.0,
                    answered: false,
                })
                .collect(),
        }
    }
    /// Take one batch's answers. Answers for rows outside this ranking are
    /// ignored; they belong to a table that has since changed.
    pub(super) fn absorb(&mut self, batch: &QuestionBatch, answers: &DecisionResponse) {
        for row in batch.rows() {
            let Some(entry) = self.rows.get_mut(row) else {
                continue;
            };
            if let Some(probability) = answers.noul(&QuestionBatch::row_key(row)) {
                entry.probability = probability;
                entry.answered = true;
            }
        }
    }
    /// The ranking, best first; rows that tie, and rows still without an
    /// answer, keep their table order.
    pub(super) fn ranked(&self) -> Vec<RankedRow> {
        let mut ranked = self.rows.clone();
        ranked.sort_by(|left, right| right.probability.total_cmp(&left.probability));
        ranked
    }
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }
    pub(super) fn unanswered(&self) -> usize {
        self.rows.iter().filter(|row| !row.answered).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::curation::similar::questions::SearchQuestions;

    fn answers(json: &str) -> DecisionResponse {
        serde_json::from_str(json).expect("a canned Decisions response")
    }
    fn batches(rows: usize) -> Vec<QuestionBatch> {
        let table = (0..rows)
            .map(|row| format!("row {row}"))
            .collect::<Vec<_>>();
        let borrowed = table.iter().map(String::as_str).collect::<Vec<_>>();
        SearchQuestions::new("fold the cloth", None)
            .expect("a nonempty query")
            .batches(&borrowed)
    }

    #[test]
    fn answered_rows_sort_ahead_of_rows_without_an_answer() {
        let batches = batches(4);
        let mut ranking = SearchRanking::of(4);
        ranking.absorb(
            &batches[0],
            &answers(
                r#"{"answers":{"r0":{"type":"noul","noul":0.2},
                    "r1":{"type":"noul","noul":0.91},
                    "r2":{"type":"noul","noul":0.5}}}"#,
            ),
        );
        let ranked = ranking.ranked();
        assert_eq!(
            ranked.iter().map(RankedRow::row).collect::<Vec<_>>(),
            [1, 2, 0, 3]
        );
        assert_eq!(ranked[0].probability(), 0.91);
        assert!(!ranked[3].answered());
        assert_eq!(ranked[3].probability(), 0.0);
        assert_eq!(ranking.unanswered(), 1);
        assert_eq!(ranking.len(), 4);
    }

    #[test]
    fn a_choice_answer_or_an_out_of_range_probability_is_no_answer() {
        let batches = batches(2);
        let mut ranking = SearchRanking::of(2);
        ranking.absorb(
            &batches[0],
            &answers(
                r#"{"answers":{"r0":{"type":"choice","choice":"fold","confidence":0.9},
                    "r1":{"type":"noul","noul":1.4}}}"#,
            ),
        );
        assert_eq!(ranking.unanswered(), 2);
    }

    #[test]
    fn answers_for_a_table_that_has_since_shrunk_are_ignored() {
        let batches = batches(3);
        let mut ranking = SearchRanking::of(1);
        ranking.absorb(
            &batches[0],
            &answers(
                r#"{"answers":{"r0":{"type":"noul","noul":0.4},"r2":{"type":"noul","noul":0.9}}}"#,
            ),
        );
        assert_eq!(ranking.ranked().len(), 1);
        assert_eq!(ranking.ranked()[0].probability(), 0.4);
    }
}
