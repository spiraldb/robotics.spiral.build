//! Studio's datasets: the tables under `datasets.<id>` that Studio writes
//! itself when it imports episodes, and reads instead of any source file
//! ([datasets](../../../../docs/datasets.md)).
mod answers;
mod catalog;
mod episodes;
mod files;
#[cfg(test)]
pub(crate) mod fixtures;
mod mcap_episode;
mod recording;
mod streams;
mod topics;
mod writer;
pub use answers::{ColumnAnswers, EpisodeAnswer, StatementAnswers, StoredAnswers};
pub(crate) use catalog::DatasetCatalogClient;
#[cfg(test)]
pub(crate) use catalog::TestJournal;
pub(crate) use files::{BundledFile, BundledSource, McapFile, McapSources};
#[cfg(test)]
pub(crate) use streams::ArrowTypeText;
pub(crate) use streams::{MessagePositions, RecordedStream, StreamMessages};
pub(crate) use writer::{
    DatasetImport, DatasetWrite, DecodedEpisode, EpisodeOrigin, LeRobotEpisode,
};

#[cfg(test)]
use crate::{SpiralAccess, SpiralConnection};
use spiral_robotics_common::datasets::EpisodeId;

pub(crate) struct EpisodeTopicQuery {
    statement: crate::db::SqlStatement,
}

impl EpisodeTopicQuery {
    /// `sql` over one episode, whose number `$1` binds.
    pub(crate) fn for_episode(episode_id: &EpisodeId, sql: String) -> Result<Self, String> {
        Self::for_episodes(std::slice::from_ref(episode_id), sql)
    }

    /// `sql` over the listed episodes: `$1` through `$N` bind their
    /// numbers, in order.
    pub(crate) fn for_episodes(episodes: &[EpisodeId], sql: String) -> Result<Self, String> {
        if episodes.is_empty() {
            return Err("A statement over listed episodes lists at least one".to_owned());
        }
        let fields = (1..=episodes.len())
            .map(|place| EpisodeIds::field(&format!("id_{place}")))
            .collect::<Vec<_>>();
        let columns = episodes
            .iter()
            .map(|episode| EpisodeIds::array(std::slice::from_ref(episode)))
            .collect::<Vec<_>>();
        let parameters = arrow_array::RecordBatch::try_new(
            std::sync::Arc::new(arrow_schema::Schema::new(fields)),
            columns,
        )
        .map_err(|error| error.to_string())?;
        crate::db::SqlStatement::from(sql)
            .with_parameters(parameters)
            .map(|statement| Self { statement })
            .map_err(|error| error.to_string())
    }
    pub(crate) fn into_statement(self) -> crate::db::SqlStatement {
        self.statement
    }
}

/// Episode ids as every dataset table holds them: `UInt64`, never null.
pub(crate) struct EpisodeIds;

impl EpisodeIds {
    pub(crate) fn field(name: &str) -> arrow_schema::Field {
        arrow_schema::Field::new(name, arrow_schema::DataType::UInt64, false)
    }

    pub(crate) fn array(ids: &[EpisodeId]) -> arrow_array::ArrayRef {
        std::sync::Arc::new(arrow_array::UInt64Array::from_iter_values(
            ids.iter().map(EpisodeId::number),
        ))
    }

    /// The ids a column of episode numbers holds, in order.
    pub(crate) fn read(array: &dyn arrow_array::Array) -> Result<Vec<EpisodeId>, String> {
        use arrow_array::Array as _;
        let numbers = array
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .filter(|numbers| numbers.null_count() == 0)
            .ok_or("Episode ids must be episode numbers")?;
        Ok(numbers
            .values()
            .iter()
            .copied()
            .map(EpisodeId::new)
            .collect())
    }
}

/// The Spiral project hosted tests run in, named by `SPIRAL_MCAP_TEST_PROJECT`
/// and reached with `SPIRAL_ACCESS_TOKEN` at `SPIRAL__SERVER__URL` and
/// `SPIRAL__SPFS__URL`. Each test creates and drops its own scratch
/// relations there.
#[cfg(test)]
pub(crate) struct HostedProject;

#[cfg(test)]
impl HostedProject {
    pub(crate) fn storage() -> (std::sync::Arc<crate::ProjectStore>, crate::StorageManager) {
        let connection = Self::connection();
        let store = std::sync::Arc::new(
            crate::ProjectStore::new(
                connection.clone(),
                std::sync::Arc::new(crate::SpiralSdkDatabase::new(connection)),
            )
            .unwrap(),
        );
        let storage = crate::StorageManager::new(
            store.clone(),
            crate::StudioStore::in_memory().project(&store.connection_settings()),
        );
        (store, storage)
    }

    /// The project as `SPIRAL_ACCESS_TOKEN`'s user reaches it.
    pub(crate) fn connection() -> SpiralConnection {
        use crate::settings::{ServiceEndpoint, SpiralConnectionSettings};
        let settings = SpiralConnectionSettings::new(
            ServiceEndpoint::environment_spiral().into_string(),
            ServiceEndpoint::environment_spiral_spfs().into_string(),
        )
        .unwrap()
        .with_project(
            &std::env::var("SPIRAL_MCAP_TEST_PROJECT").expect("the Spiral project to test in"),
        )
        .unwrap();
        let access = SpiralAccess::fixed(
            std::env::var("SPIRAL_ACCESS_TOKEN").expect("a user access token for the test"),
        );
        SpiralConnection::new(settings, access)
    }
}

/// Probe datasets for hosted tests, here and in the crates above storage.
#[cfg(any(test, feature = "test-support"))]
impl crate::StorageManager {
    /// Record the short episode `episode` into the MCAP dataset `dataset`,
    /// creating it with the first: eight frames of `/camera/top/video`, 33 ms
    /// apart.
    pub async fn record_probe_episode(&self, dataset: &str, episode: &str) -> Result<(), String> {
        use spiral_robotics_recording::{CameraRecording, EpisodeFixture, McapRecording};
        let camera = CameraRecording::from_annex_b(
            "top".into(),
            (32, 32, 30),
            (
                include_bytes!("../../../frontend/src/activities/episode/fixtures/seek.h264")
                    .to_vec(),
                (0..8).map(|index| index * 33_333_333).collect(),
            ),
        )?;
        let recording = EpisodeFixture::new(episode, dataset)
            .with_timing(1_700_000_000_000, 500.0)
            .with_duration_seconds(0.3)
            .into_episode();
        let bytes = McapRecording::encode(&recording, &[camera])?;
        self.store_recording(&crate::EpisodeName::new(dataset, episode)?, &bytes)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_reads_bind_the_episode_number_without_metadata() {
        let statement = EpisodeTopicQuery::for_episode(
            &EpisodeId::new(7),
            "SELECT value FROM source WHERE id = $1".into(),
        )
        .unwrap()
        .statement;
        assert!(statement.as_str().contains("WHERE id = $1"));
        let parameters = statement.parameters().unwrap();
        let field = parameters.schema().field(0).clone();
        assert_eq!(field.data_type(), &arrow_schema::DataType::UInt64);
        assert!(
            field.metadata().is_empty(),
            "a plain number filters Spiral's scan"
        );
        let values = parameters
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .unwrap();
        assert_eq!(values.value(0), 7);
    }

    #[test]
    fn listed_episodes_bind_one_number_each_in_their_order() {
        let ids = [EpisodeId::new(1), EpisodeId::new(2)];
        let statement = EpisodeTopicQuery::for_episodes(
            &ids,
            "SELECT value FROM source WHERE id IN ($1, $2)".into(),
        )
        .unwrap()
        .statement;
        let parameters = statement.parameters().unwrap();
        assert_eq!(parameters.num_rows(), 1);
        let bound = (0..parameters.num_columns())
            .map(|place| EpisodeIds::read(parameters.column(place).as_ref()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(bound, [vec![ids[0].clone()], vec![ids[1].clone()]]);
        assert!(EpisodeTopicQuery::for_episodes(&[], "SELECT 1".into()).is_err());
    }

    #[test]
    fn episode_ids_travel_as_numbers_and_read_back_as_they_went() {
        let ids = [EpisodeId::new(1), EpisodeId::new(2)];
        let array = EpisodeIds::array(&ids);
        assert_eq!(EpisodeIds::read(array.as_ref()).unwrap(), ids);
        let text = arrow_array::StringArray::from(vec!["01900000000070008000000000000001"]);
        assert!(EpisodeIds::read(&text).is_err());
    }
}
