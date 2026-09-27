//! The category proposal: what the LLM is asked and what it must answer
//! with, and the category the operator is editing.
//!
//! The categories come from the descriptions themselves, so the list is
//! complete by construction: `spql.ai.classify` puts every subtask in its
//! most probable category, with no "other" option and no confidence gate.
use crate::activities::curation::document::SubtaskCategory;
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
        // The classifier reads the names as the options of one choice, so two
        // names that differ only in case would be two indistinguishable
        // options.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
