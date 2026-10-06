//! Where the model answers curations ask for are kept: one answers table per
//! dataset, `dataset_answers_<id>` beside `dataset_<id>` and
//! `dataset_files_<id>`, holding one row per episode keyed by
//! `episode_id`, and one column per exact call a node makes.
//!
//! A column is named for what it measures and a short hash of the call, so
//! identical calls across curations share it. Each value is wrapped as
//! `STRUCT(value <answer>)`: a NULL struct is an episode not asked yet, and a
//! struct whose value is NULL one the model could not answer, which is never
//! asked again.
use crate::datasets::DatasetSelection;
use crate::sql::UnquotedSqlIdentifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::{self, Display, Formatter};

/// One dataset's answers table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnswerTable {
    dataset_id: String,
}

impl AnswerTable {
    /// The answers table of the dataset Studio imported as `dataset_<id>` at
    /// the catalog root; no other dataset keeps answers.
    pub fn of(dataset: &DatasetSelection) -> Result<Self, String> {
        match dataset.path() {
            [name] => name
                .strip_prefix("dataset_")
                .filter(|id| !id.is_empty())
                .map(Self::for_dataset_id),
            _ => None,
        }
        .ok_or_else(|| {
            format!(
                "{} keeps no model answers; only datasets Studio imported do.",
                dataset.display_name()
            )
        })
    }
    /// The answers table of the dataset Studio names `dataset_<dataset_id>`.
    pub fn for_dataset_id(dataset_id: &str) -> Self {
        Self {
            dataset_id: dataset_id.to_owned(),
        }
    }
    pub fn name(&self) -> String {
        format!("dataset_answers_{}", self.dataset_id)
    }
    /// The name as a statement spells it: quoted, since a dataset id may
    /// hold a hyphen.
    pub fn quoted(&self) -> String {
        format!("\"{}\"", self.name().replace('"', "\"\""))
    }
}

/// One column of an answers table: what one exact call answered for each
/// episode.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AnswerColumn(String);

impl AnswerColumn {
    /// How many hex digits of the call's hash name a column.
    const HASH_DIGITS: usize = 6;

    /// The column `measure` writes for `call`: everything that changes the
    /// answer — the function or service, the model revision, the arguments,
    /// the camera or instruction topic, the column it reads — as JSON, whose
    /// arrays keep their order, so one call always hashes alike.
    pub fn of(measure: &str, call: &impl Serialize) -> Result<Self, String> {
        let call = serde_json::to_string(call)
            .map_err(|error| format!("describing the {measure} call: {error}"))?;
        let digest = format!("{:x}", Sha256::digest(call.as_bytes()));
        Self::try_from(format!("{measure}_{}", &digest[..Self::HASH_DIGITS]))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which episodes of `dataset` one answers column holds an answer for,
/// failed ones included: what a curation opened reads back.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FilledEpisodesRequest {
    dataset: DatasetSelection,
    column: AnswerColumn,
}

impl FilledEpisodesRequest {
    pub fn new(dataset: DatasetSelection, column: AnswerColumn) -> Self {
        Self { dataset, column }
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn column(&self) -> &AnswerColumn {
        &self.column
    }
}

impl TryFrom<String> for AnswerColumn {
    type Error = String;

    /// A measure, then `_` and the hash digits: one plain identifier.
    fn try_from(name: String) -> Result<Self, Self::Error> {
        let hashed = name.rsplit_once('_').is_some_and(|(measure, hash)| {
            !measure.is_empty()
                && hash.len() == Self::HASH_DIGITS
                && hash
                    .bytes()
                    .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit))
        });
        if hashed && UnquotedSqlIdentifier::try_from(name.as_str()).is_ok() {
            Ok(Self(name))
        } else {
            Err(format!("{name:?} is not an answer column"))
        }
    }
}

impl From<AnswerColumn> for String {
    fn from(column: AnswerColumn) -> Self {
        column.0
    }
}

impl Display for AnswerColumn {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_is_its_measure_and_a_hash_of_the_exact_call() {
        let call = ["modal.score_progress", "warp@6417be0", "/cameras/top"];
        let column = AnswerColumn::of("score_progress", &call).unwrap();
        assert!(column.as_str().starts_with("score_progress_"));
        assert_eq!(column.as_str().len(), "score_progress_".len() + 6);
        assert_eq!(AnswerColumn::of("score_progress", &call).unwrap(), column);
        let other = ["modal.score_progress", "warp@6417be0", "/cameras/wrist"];
        assert_ne!(AnswerColumn::of("score_progress", &other).unwrap(), column);
        assert_eq!(
            serde_json::from_value::<AnswerColumn>(serde_json::to_value(&column).unwrap()).unwrap(),
            column
        );
        for invalid in [
            "score_progress",
            "score_progress_3F9A2C",
            "_3f9a2c",
            "a b_3f9a2c",
        ] {
            assert!(
                AnswerColumn::try_from(invalid.to_owned()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn only_a_dataset_studio_imported_keeps_answers() {
        let imported = DatasetSelection::new(vec!["dataset_pick-place".into()]).unwrap();
        let table = AnswerTable::of(&imported).unwrap();
        assert_eq!(table.name(), "dataset_answers_pick-place");
        assert_eq!(table.quoted(), "\"dataset_answers_pick-place\"");
        for foreign in [
            vec!["recordings"],
            vec!["dataset_"],
            vec!["lab", "dataset_x"],
        ] {
            let dataset =
                DatasetSelection::new(foreign.into_iter().map(String::from).collect()).unwrap();
            assert!(AnswerTable::of(&dataset).is_err());
        }
    }
}
