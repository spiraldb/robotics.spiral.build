//! The relations Studio keeps for each dataset it imports, named after the
//! dataset `<id>`: `dataset_<id>`, the Dataset; `dataset_files_<id>`, the
//! files it is made from — a table of MCAP files, or a Location over a
//! LeRobot repository —; and `dataset_answers_<id>`, the model answers about
//! its episodes.
use crate::ProjectConnection;
use spiral_robotics_common::curation::AnswerTable;
use spiral_robotics_common::datasets::ProjectResource;
use spiral_robotics_common::sql::SqlLiteral;

pub(crate) struct StudioDataset {
    dataset: String,
    files: String,
    answers: AnswerTable,
}

impl StudioDataset {
    pub(crate) fn new(dataset_id: &str) -> Self {
        Self {
            dataset: format!("dataset_{dataset_id}"),
            files: format!("dataset_files_{dataset_id}"),
            answers: AnswerTable::for_dataset_id(dataset_id),
        }
    }
    /// The Dataset's SQL name.
    pub(crate) fn dataset(&self) -> &str {
        &self.dataset
    }
    /// The SQL name of the relation of files the Dataset is made from.
    pub(crate) fn files(&self) -> &str {
        &self.files
    }
    pub(crate) fn answers(&self) -> &AnswerTable {
        &self.answers
    }
    /// `name` as one quoted identifier per dotted part.
    pub(crate) fn quoted(name: &str) -> String {
        name.split('.')
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".")
    }
    /// Every relation an import creates, the Dataset first.
    pub(crate) fn planned(&self) -> Vec<String> {
        vec![
            self.dataset.clone(),
            self.files.clone(),
            self.answers.name(),
        ]
    }
    /// Refuse an import over any relation `resources` already holds: an
    /// import creates its relations, never adopts them.
    pub(crate) fn refuse_existing(&self, resources: &[ProjectResource]) -> Result<(), String> {
        if resources
            .iter()
            .any(|resource| self.planned().contains(&resource.selection().label()))
        {
            return Err(
                "The dataset or its source already exists; choose another dataset name".to_owned(),
            );
        }
        Ok(())
    }
    /// Run the Dataset's producer over its files, publishing a new revision.
    pub(crate) async fn refresh(&self, connection: &mut ProjectConnection) -> Result<(), String> {
        connection
            .execute_update_sql(&format!("REFRESH DATASET {}", Self::quoted(&self.dataset)))
            .await
    }
    /// How many episodes the Dataset holds.
    pub(crate) fn members_sql(&self) -> String {
        format!(
            "SELECT COUNT(*) AS members FROM spql.mcap.episodes({})",
            SqlLiteral::text(&self.dataset)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::DatasetSelection;

    #[test]
    fn an_import_names_its_relations_after_the_dataset_and_never_adopts_one() {
        let names = StudioDataset::new("pick-place");
        assert_eq!(
            names.planned(),
            [
                "dataset_pick-place",
                "dataset_files_pick-place",
                "dataset_answers_pick-place"
            ]
        );
        assert_eq!(
            StudioDataset::quoted("dataset_pick-place"),
            "\"dataset_pick-place\""
        );
        let resource = |name: &str| {
            ProjectResource::new(
                DatasetSelection::new(vec![name.into()]).unwrap(),
                "location".into(),
                ("r".into(), "v".into()),
            )
        };
        assert!(
            names
                .refuse_existing(&[resource("dataset_files_pick-place")])
                .is_err()
        );
        assert!(names.refuse_existing(&[resource("dataset_other")]).is_ok());
    }
}
