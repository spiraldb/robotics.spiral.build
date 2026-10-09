//! Studio's LeRobot datasets: each one's files are a Location over its
//! Hugging Face repository at one commit, listed by Spiral and never copied,
//! and its Dataset is made from them by `spql.lerobot.dataset`, which copies
//! every episode into Spiral at its refresh and encodes its cameras again.
use super::StudioDataset;
use crate::ProjectConnection;
use spiral_robotics_common::datasets::LeRobotImportPhase;
use spiral_robotics_common::sql::SqlLiteral;

pub(crate) struct LeRobotDatasetDefinition {
    relations: StudioDataset,
    /// The repository's root at its commit: `hf://datasets/<org>/<repo>@<sha>/`.
    root: String,
}

impl LeRobotDatasetDefinition {
    pub(crate) fn new(dataset_id: &str, root: String) -> Self {
        Self {
            relations: StudioDataset::new(dataset_id),
            root,
        }
    }

    pub(crate) fn relations(&self) -> &StudioDataset {
        &self.relations
    }

    /// The Location naming the repository, whose rows are its files.
    fn create_location_sql(&self) -> String {
        format!(
            "CREATE LOCATION {} WITH (uri = {})",
            StudioDataset::quoted(self.relations.files()),
            SqlLiteral::text(&self.root)
        )
    }

    /// List the repository's files into the Location's inventory.
    fn refresh_location_sql(&self) -> String {
        format!(
            "REFRESH LOCATION {}",
            StudioDataset::quoted(self.relations.files())
        )
    }

    /// The Dataset over the Location, each file keyed by its path in the
    /// repository; LeRobot's producer always copies every topic.
    fn create_sql(&self) -> String {
        format!(
            "CREATE DATASET {} USING spql.lerobot.dataset FROM {} WITH (key = 'path', blob = 'file', materialization = 'all')",
            StudioDataset::quoted(self.relations.dataset()),
            StudioDataset::quoted(self.relations.files())
        )
    }

    /// Create the Location and list it, create the Dataset over it and
    /// refresh it, then create its answers table, saying each phase as it
    /// starts. The refresh runs in this process and reads every episode.
    pub(crate) async fn import(
        &self,
        connection: &mut ProjectConnection,
        phase: &(dyn Fn(LeRobotImportPhase) + Send + Sync),
    ) -> Result<(), String> {
        phase(LeRobotImportPhase::ListingFiles);
        connection
            .execute_update_sql(&self.create_location_sql())
            .await?;
        connection
            .execute_update_sql(&self.refresh_location_sql())
            .await?;
        phase(LeRobotImportPhase::CreatingDataset);
        connection.execute_update_sql(&self.create_sql()).await?;
        phase(LeRobotImportPhase::CopyingEpisodes);
        self.relations.refresh(connection).await?;
        phase(LeRobotImportPhase::CreatingAnswers);
        self.relations.create_answers(connection).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dataset_is_made_from_a_location_named_as_its_files() {
        let definition = LeRobotDatasetDefinition::new(
            "pick",
            "hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/"
                .into(),
        );
        assert_eq!(
            definition.create_location_sql(),
            "CREATE LOCATION \"dataset_files_pick\" WITH (uri = 'hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/')"
        );
        assert_eq!(
            definition.refresh_location_sql(),
            "REFRESH LOCATION \"dataset_files_pick\""
        );
        assert_eq!(
            definition.create_sql(),
            "CREATE DATASET \"dataset_pick\" USING spql.lerobot.dataset FROM \"dataset_files_pick\" WITH (key = 'path', blob = 'file', materialization = 'all')"
        );
    }
}
