//! Where the model answers curations ask for are kept: one table per exact
//! call a node makes, `datasets.<id>.answers.<column>` beside the dataset's
//! `episodes` ([`DatasetSelection::answers`]).
//!
//! A column is named for what it measures and a short hash of the call, so
//! identical calls across curations share it. Its table holds `episode_id`
//! and `value`, one row per episode asked: a NULL `value` is one the model
//! could not answer, which is never asked again, and an episode with no row
//! is not asked yet. A fill run only appends rows, and makes the table,
//! empty, when it starts filling the column, so whatever reads it once the
//! run is on it finds it.
use crate::datasets::DatasetSelection;
use crate::sql::UnquotedSqlIdentifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::{self, Display, Formatter};

/// One answer column: what one exact call answered for each episode, in a
/// table of its own.
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

/// Which episodes of `dataset` one answer column holds an answer for, failed
/// ones included: what a curation opened reads back.
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
}
