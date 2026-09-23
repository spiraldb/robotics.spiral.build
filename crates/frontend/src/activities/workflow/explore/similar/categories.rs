//! The category pass: what the LLM is asked, what it must answer with, and
//! how jev's Choice answers become saved assignments.
//!
//! The categories come from the descriptions themselves, so the list is
//! complete by construction: every row joins its highest-probability
//! category, with no "other" option and no confidence gate. The probability
//! and confidence are kept to order members and to let ambiguous rows show.
use super::questions::QuestionBatch;
use crate::activities::workflow::explore::document::{
    SubtaskAssignment, SubtaskCategory, SubtaskRow,
};
use crate::client::decisions::DecisionResponse;
use serde::Deserialize;
use std::collections::BTreeMap;

/// How many categories the pass accepts, as the design doc fixes them.
const CATEGORY_RANGE: std::ops::RangeInclusive<usize> = 4..=8;
/// The description list is one prompt; a large table sends its most common
/// descriptions rather than an unbounded request.
const MAX_PROMPT_CHARS: usize = 60_000;

/// The distinct subtask descriptions with how many rows share each — what
/// the category prompt reads. Most common first, so a table too large for
/// one prompt still describes the operations that dominate it.
pub(super) struct DescriptionCounts {
    entries: Vec<(String, usize)>,
}
impl DescriptionCounts {
    pub(super) fn of(descriptions: &[&str]) -> Self {
        let mut counts = BTreeMap::<&str, usize>::new();
        for description in descriptions {
            *counts.entry(description.trim()).or_default() += 1;
        }
        counts.remove("");
        let mut entries = counts
            .into_iter()
            .map(|(description, count)| (description.to_owned(), count))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        Self { entries }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The instruction the annotation model answers, JSON and nothing else.
    pub(super) fn prompt(&self) -> String {
        let mut listed = String::new();
        let mut shown = 0;
        for (description, count) in &self.entries {
            if shown > 0 && listed.len() + description.len() > MAX_PROMPT_CHARS {
                break;
            }
            listed.push_str(&format!("{count} × {description}\n"));
            shown += 1;
        }
        let scope = if shown == self.entries.len() {
            format!("all {shown} distinct subtask descriptions")
        } else {
            format!(
                "the {shown} most common of {} distinct subtask descriptions",
                self.entries.len()
            )
        };
        format!(
            "These are {scope} annotated in one robot dataset, each with the number of \
             subtask intervals that share it.\n\n{listed}\nPropose between {} and {} \
             categories that cover these operations. Give each category a short name and \
             one sentence of definition that distinguishes it from every other category. \
             Categories must be about the manipulation operation, not about an episode or \
             a moment in time.\n\nAnswer with only this JSON, with no prose and no code \
             fence:\n{{\"categories\":[{{\"name\":\"…\",\"definition\":\"…\"}}]}}",
            CATEGORY_RANGE.start(),
            CATEGORY_RANGE.end()
        )
    }
}

#[derive(Deserialize)]
struct ProposedCategory {
    name: String,
    definition: String,
}
#[derive(Deserialize)]
struct CategoryReply {
    categories: Vec<ProposedCategory>,
}

/// The categories the model proposed, once they are usable: 4 to 8 of them,
/// each named and defined, no two names alike.
pub(super) struct ProposedCategories {
    categories: Vec<SubtaskCategory>,
}
impl ProposedCategories {
    /// Read the model's reply. The prompt asks for bare JSON; a code fence
    /// is common enough in practice to unwrap rather than reject.
    pub(super) fn parse(reply: &str) -> Result<Self, String> {
        let reply = Self::unfenced(reply.trim());
        let parsed = serde_json::from_str::<CategoryReply>(reply)
            .map_err(|error| format!("The model did not answer with category JSON: {error}"))?;
        if !CATEGORY_RANGE.contains(&parsed.categories.len()) {
            return Err(format!(
                "The model proposed {} categories; {} to {} are required.",
                parsed.categories.len(),
                CATEGORY_RANGE.start(),
                CATEGORY_RANGE.end()
            ));
        }
        let categories = parsed
            .categories
            .into_iter()
            .map(|category| SubtaskCategory::new(category.name, category.definition))
            .collect::<Result<Vec<_>, _>>()?;
        // jev reads the names as the options of one Choice, so two names
        // that differ only in case would be two indistinguishable options.
        let mut names = categories
            .iter()
            .map(|category| category.name().to_lowercase())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        if names.len() != categories.len() {
            return Err("The model proposed two categories with the same name.".into());
        }
        Ok(Self { categories })
    }
    pub(super) fn into_categories(self) -> Vec<SubtaskCategory> {
        self.categories
    }
    fn unfenced(reply: &str) -> &str {
        let Some(fenced) = reply.strip_prefix("```") else {
            return reply;
        };
        fenced
            .split_once('\n')
            .map_or(fenced, |(_language, body)| body)
            .trim_end()
            .trim_end_matches("```")
            .trim()
    }
}

/// One category as the operator is editing it: what it was called, and what
/// it should now be called and mean.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CategoryEdit {
    original: String,
    name: String,
    definition: String,
}
impl CategoryEdit {
    pub(super) fn of(category: &SubtaskCategory) -> Self {
        Self {
            original: category.name().to_owned(),
            name: category.name().to_owned(),
            definition: category.definition().to_owned(),
        }
    }
    pub(super) fn original(&self) -> &str {
        &self.original
    }
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn set_name(&mut self, name: String) {
        self.name = name;
    }
    pub(super) fn definition(&self) -> &str {
        &self.definition
    }
    pub(super) fn set_definition(&mut self, definition: String) {
        self.definition = definition;
    }
}

/// Changes to a grouping's category list. Each returns a new list to assign
/// rows against: an edited category only reruns the jev pass, never the LLM
/// one, because the categories themselves are the operator's now.
pub(super) struct CategoryEdits<'a> {
    categories: &'a [SubtaskCategory],
}
impl<'a> CategoryEdits<'a> {
    pub(super) fn new(categories: &'a [SubtaskCategory]) -> Self {
        Self { categories }
    }
    pub(super) fn renamed(&self, edit: &CategoryEdit) -> Result<Vec<SubtaskCategory>, String> {
        let renamed = SubtaskCategory::new(edit.name(), edit.definition())?;
        if self
            .categories
            .iter()
            .any(|category| category.name() != edit.original() && category.name() == renamed.name())
        {
            return Err(format!(
                "A category is already called “{}”.",
                renamed.name()
            ));
        }
        let mut categories = self.categories.to_vec();
        let position = categories
            .iter()
            .position(|category| category.name() == edit.original())
            .ok_or("That category is no longer part of this grouping.")?;
        categories[position] = renamed;
        Ok(categories)
    }
    /// `from` disappears into `into`, which keeps its name and gains the
    /// other definition: the union of two groups is not described by either
    /// sentence alone, and the operator can trim it afterwards.
    pub(super) fn merged(&self, into: &str, from: &str) -> Result<Vec<SubtaskCategory>, String> {
        if into == from {
            return Err("Merge a category into a different one.".into());
        }
        let absorbed = self
            .categories
            .iter()
            .find(|category| category.name() == from)
            .ok_or("That category is no longer part of this grouping.")?;
        let mut categories = Vec::with_capacity(self.categories.len() - 1);
        for category in self.categories {
            if category.name() == from {
                continue;
            }
            if category.name() == into {
                categories.push(SubtaskCategory::new(
                    category.name(),
                    format!("{} {}", category.definition(), absorbed.definition()),
                )?);
            } else {
                categories.push(category.clone());
            }
        }
        if categories.len() != self.categories.len() - 1 {
            return Err("That category is no longer part of this grouping.".into());
        }
        Ok(categories)
    }
}

/// Turns jev's Choice answers into the assignments a grouping saves.
pub(super) struct AssignmentReader<'a> {
    rows: &'a [SubtaskRow],
    categories: &'a [SubtaskCategory],
}
impl<'a> AssignmentReader<'a> {
    pub(super) fn new(rows: &'a [SubtaskRow], categories: &'a [SubtaskCategory]) -> Self {
        Self { rows, categories }
    }
    /// One batch's assignments, in row order. A row without a usable answer
    /// is left unassigned rather than guessed at; the caller counts them.
    pub(super) fn read(
        &self,
        batch: &QuestionBatch,
        answers: &DecisionResponse,
    ) -> Vec<SubtaskAssignment> {
        batch
            .rows()
            .filter_map(|row| self.assignment(row, answers))
            .collect()
    }
    fn assignment(&self, row: usize, answers: &DecisionResponse) -> Option<SubtaskAssignment> {
        let subtask = self.rows.get(row)?;
        let distribution = answers.choice_distribution(&QuestionBatch::row_key(row))?;
        let category = self
            .categories
            .iter()
            .find(|category| category.name() == distribution.choice())?;
        let assignment = SubtaskAssignment::new(
            subtask.episode().clone(),
            subtask.start_time(),
            category.name(),
            distribution.probability(category.name()),
            distribution.confidence().unwrap_or(0.0),
        )
        .ok()?;
        // A runner-up jev named that is not one of these categories is a
        // stray option, not a second choice worth keeping.
        let runner_up = distribution.runner_up().and_then(|(name, probability)| {
            let known = self
                .categories
                .iter()
                .find(|category| category.name() == name)?;
            Some((known.name(), probability))
        });
        match runner_up {
            Some((name, probability)) => assignment.with_runner_up(name, probability).ok(),
            None => Some(assignment),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::workflow::explore::document::RunnerUp;
    use crate::activities::workflow::explore::similar::questions::AssignmentQuestions;
    use spiral_robotics_common::datasets::EpisodeId;

    fn categories(names: &[&str]) -> Vec<SubtaskCategory> {
        names
            .iter()
            .map(|name| SubtaskCategory::new(*name, format!("Everything that {name}s.")).unwrap())
            .collect()
    }
    fn reply(categories: usize) -> String {
        let entries = (0..categories)
            .map(|index| format!(r#"{{"name":"c{index}","definition":"Defines c{index}."}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"categories":[{entries}]}}"#)
    }

    #[test]
    fn counts_dedupe_descriptions_and_lead_with_the_most_common() {
        let counts = DescriptionCounts::of(&["fold", "grasp", "fold", "  fold  ", "  "]);
        let prompt = counts.prompt();
        assert!(!counts.is_empty());
        assert!(prompt.contains("3 × fold\n1 × grasp"), "{prompt}");
        assert!(prompt.contains("all 2 distinct subtask descriptions"));
        assert!(prompt.contains("between 4 and 8"));
        assert!(prompt.contains("only this JSON"));
        assert!(DescriptionCounts::of(&[]).is_empty());
    }

    #[test]
    fn an_oversized_table_sends_its_most_common_descriptions() {
        let table = (0..4_000)
            .map(|index| format!("subtask number {index} of a long annotated table"))
            .collect::<Vec<_>>();
        let borrowed = table.iter().map(String::as_str).collect::<Vec<_>>();
        let prompt = DescriptionCounts::of(&borrowed).prompt();
        assert!(
            prompt.contains("of 4000 distinct subtask descriptions"),
            "{prompt}"
        );
        assert!(prompt.len() < MAX_PROMPT_CHARS * 2);
    }

    #[test]
    fn a_plain_json_answer_becomes_categories() {
        let proposed = ProposedCategories::parse(&reply(5))
            .unwrap()
            .into_categories();
        assert_eq!(proposed.len(), 5);
        assert_eq!(proposed[0].name(), "c0");
        assert_eq!(proposed[0].definition(), "Defines c0.");
    }

    #[test]
    fn a_fenced_answer_is_unwrapped() {
        let fenced = format!("```json\n{}\n```", reply(4));
        assert_eq!(
            ProposedCategories::parse(&fenced)
                .unwrap()
                .into_categories()
                .len(),
            4
        );
        let bare_fence = format!("```\n{}\n```", reply(8));
        assert!(ProposedCategories::parse(&bare_fence).is_ok());
    }

    #[test]
    fn too_few_categories_empty_definitions_and_repeated_names_are_refused() {
        assert!(ProposedCategories::parse(&reply(3)).is_err());
        assert!(ProposedCategories::parse(&reply(9)).is_err());
        assert!(ProposedCategories::parse("sure! here they are").is_err());
        assert!(
            ProposedCategories::parse(
                r#"{"categories":[{"name":"a","definition":"A."},{"name":"b","definition":" "},
                   {"name":"c","definition":"C."},{"name":"d","definition":"D."}]}"#
            )
            .is_err()
        );
        assert!(
            ProposedCategories::parse(
                r#"{"categories":[{"name":"Fold","definition":"A."},{"name":"fold","definition":"B."},
                   {"name":"c","definition":"C."},{"name":"d","definition":"D."}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn renaming_keeps_the_list_and_refuses_a_name_another_category_holds() {
        let categories = categories(&["fold", "grasp"]);
        let mut edit = CategoryEdit::of(&categories[0]);
        edit.set_name("fold in half".into());
        edit.set_definition("Folding the cloth over itself.".into());
        let renamed = CategoryEdits::new(&categories).renamed(&edit).unwrap();
        assert_eq!(renamed[0].name(), "fold in half");
        assert_eq!(renamed[0].definition(), "Folding the cloth over itself.");
        assert_eq!(renamed[1], categories[1]);

        let mut clash = CategoryEdit::of(&categories[0]);
        clash.set_name("grasp".into());
        assert!(CategoryEdits::new(&categories).renamed(&clash).is_err());
        let mut blank = CategoryEdit::of(&categories[0]);
        blank.set_definition("  ".into());
        assert!(CategoryEdits::new(&categories).renamed(&blank).is_err());
    }

    #[test]
    fn merging_drops_one_category_and_keeps_both_definitions() {
        let categories = categories(&["fold", "grasp", "smooth"]);
        let merged = CategoryEdits::new(&categories)
            .merged("fold", "grasp")
            .unwrap();
        assert_eq!(
            merged.iter().map(SubtaskCategory::name).collect::<Vec<_>>(),
            ["fold", "smooth"]
        );
        assert_eq!(
            merged[0].definition(),
            "Everything that folds. Everything that grasps."
        );
        assert!(
            CategoryEdits::new(&categories)
                .merged("fold", "fold")
                .is_err()
        );
        assert!(
            CategoryEdits::new(&categories)
                .merged("fold", "gone")
                .is_err()
        );
    }

    #[test]
    fn every_answered_row_joins_its_argmax_category_without_a_confidence_gate() {
        let categories = categories(&["fold", "grasp", "smooth"]);
        let rows = vec![
            SubtaskRow::new("annotate-1", EpisodeId::fixture(0xa), 0.0, 1.0, "fold it").unwrap(),
            SubtaskRow::new("annotate-1", EpisodeId::fixture(0xa), 2.0, 3.0, "grab it").unwrap(),
            SubtaskRow::new("annotate-1", EpisodeId::fixture(0xb), 0.5, 1.5, "wipe it").unwrap(),
        ];
        let descriptions = rows.iter().map(SubtaskRow::description).collect::<Vec<_>>();
        let batch = &AssignmentQuestions::new(&categories)
            .unwrap()
            .batches(&descriptions)[0];
        let answers = serde_json::from_str::<DecisionResponse>(
            r#"{"answers":{
                "r0":{"type":"choice","choice":"fold","confidence":0.2,
                      "probabilities":{"fold":0.7,"grasp":0.2,"smooth":0.1}},
                "r1":{"type":"choice","choice":"grasp",
                      "probabilities":{"fold":0.1,"grasp":0.8,"smooth":0.1}},
                "r2":{"type":"choice","choice":"polish","confidence":0.9,
                      "probabilities":{"polish":1.0}}}}"#,
        )
        .unwrap();
        let assignments = AssignmentReader::new(&rows, &categories).read(batch, &answers);

        assert_eq!(assignments.len(), 2, "an unknown category is not an answer");
        assert_eq!(assignments[0].episode(), &EpisodeId::fixture(0xa));
        assert_eq!(assignments[0].start_time(), 0.0);
        assert_eq!(assignments[0].category(), "fold");
        assert_eq!(assignments[0].probability(), 0.7);
        // Low confidence still assigns; the doc keeps confidence to show
        // ambiguity, not to withhold a row.
        assert_eq!(assignments[0].confidence(), 0.2);
        assert_eq!(
            assignments[0].runner_up().map(RunnerUp::category),
            Some("grasp")
        );
        assert_eq!(assignments[1].category(), "grasp");
        assert_eq!(assignments[1].confidence(), 0.0);
    }
}
