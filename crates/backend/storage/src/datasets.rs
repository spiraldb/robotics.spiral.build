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
pub use answers::{ColumnAnswers, DatasetAnswers, EpisodeAnswer, StatementAnswers};
pub(crate) use catalog::DatasetCatalogClient;
#[cfg(test)]
pub(crate) use catalog::TestJournal;
pub(crate) use files::{McapFile, McapSources};
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
    /// `sql` over one episode, whose UUID `$1` binds.
    pub(crate) fn for_episode(episode_id: &EpisodeId, sql: String) -> Result<Self, String> {
        Self::for_episodes(std::slice::from_ref(episode_id), sql)
    }

    /// `sql` over the listed episodes: `$1` through `$N` bind their UUIDs,
    /// in order.
    pub(crate) fn for_episodes(episodes: &[EpisodeId], sql: String) -> Result<Self, String> {
        if episodes.is_empty() {
            return Err("A statement over listed episodes lists at least one".to_owned());
        }
        let fields = (1..=episodes.len())
            .map(|place| EpisodeUuids::field(&format!("id_{place}")))
            .collect::<Vec<_>>();
        let columns = episodes
            .iter()
            .map(|episode| EpisodeUuids::array(std::slice::from_ref(episode)))
            .collect::<Result<Vec<_>, _>>()?;
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

/// Dataset member ids as Spiral types them: `arrow.uuid`, sixteen bytes each.
pub(crate) struct EpisodeUuids;

impl EpisodeUuids {
    pub(crate) fn field(name: &str) -> arrow_schema::Field {
        arrow_schema::Field::new(name, arrow_schema::DataType::FixedSizeBinary(16), false)
            .with_extension_type(arrow_schema::extension::Uuid)
    }

    pub(crate) fn array(ids: &[EpisodeId]) -> Result<arrow_array::ArrayRef, String> {
        let uuids = ids
            .iter()
            .map(|id| {
                uuid::Uuid::parse_str(id.as_str()).map_err(|_| "Invalid recording UUID".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let values = arrow_array::FixedSizeBinaryArray::try_from_iter(
            uuids.iter().map(|uuid| uuid.as_bytes()),
        )
        .map_err(|error| error.to_string())?;
        Ok(std::sync::Arc::new(values))
    }

    /// The ids a column of member UUIDs holds, in order.
    pub(crate) fn read(array: &dyn arrow_array::Array) -> Result<Vec<EpisodeId>, String> {
        use arrow_array::Array as _;
        let uuids = array
            .as_any()
            .downcast_ref::<arrow_array::FixedSizeBinaryArray>()
            .filter(|uuids| uuids.value_length() == 16 && uuids.null_count() == 0)
            .ok_or("Episode ids must be member UUIDs")?;
        (0..uuids.len())
            .map(|row| {
                let bytes = <[u8; 16]>::try_from(uuids.value(row)).map_err(|e| e.to_string())?;
                Ok(EpisodeId::from(bytes))
            })
            .collect()
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
        let connection = SpiralConnection::new(settings, access);
        let store = std::sync::Arc::new(
            crate::ProjectStore::new(
                connection.clone(),
                std::sync::Arc::new(crate::AdbcDatabase::new(connection)),
            )
            .unwrap(),
        );
        let storage = crate::StorageManager::new(
            store.clone(),
            crate::StudioStore::in_memory().project(&store.connection_settings()),
        );
        (store, storage)
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

    /// Drop the probe dataset `name`: every table under its namespace.
    pub async fn drop_probe_dataset(&self, name: &str) -> Result<(), String> {
        let dataset = spiral_robotics_common::datasets::DatasetSelection::new(name)?;
        let tables = self
            .project_store()
            .dataset_catalog()
            .datasets()
            .await?
            .tables(&dataset)?;
        let mut connection = self.project_store().connect_exclusive().await?;
        for table in tables {
            connection
                .execute_update_sql(&format!("DROP TABLE IF EXISTS {table}"))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_reads_bind_uuid_bytes_and_reject_invalid_ids() {
        let id = uuid::Uuid::parse_str("01900000-0000-7000-8000-000000000001").unwrap();
        let statement = EpisodeTopicQuery::for_episode(
            &EpisodeId::try_from(id.simple().to_string()).unwrap(),
            "SELECT value FROM source WHERE id = $1".into(),
        )
        .unwrap()
        .statement;
        assert!(statement.as_str().contains("WHERE id = $1"));
        assert!(!statement.as_str().contains(&id.to_string()));
        let parameters = statement.parameters().unwrap();
        assert_eq!(
            parameters
                .schema()
                .field(0)
                .metadata()
                .get("ARROW:extension:name")
                .unwrap(),
            "arrow.uuid"
        );
        let values = parameters
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::FixedSizeBinaryArray>()
            .unwrap();
        assert_eq!(values.value(0), id.as_bytes());
        // An id that is not a member UUID never reaches a statement at all.
        assert!(EpisodeId::try_from("' OR true --").is_err());
    }

    #[test]
    fn listed_episodes_bind_one_uuid_each_in_their_order() {
        let ids = [
            EpisodeId::try_from("01900000000070008000000000000001").unwrap(),
            EpisodeId::try_from("01a0b63c7b7f779b9237d4e0206c08ff").unwrap(),
        ];
        let statement = EpisodeTopicQuery::for_episodes(
            &ids,
            "SELECT value FROM source WHERE id IN ($1, $2)".into(),
        )
        .unwrap()
        .statement;
        let parameters = statement.parameters().unwrap();
        assert_eq!(parameters.num_rows(), 1);
        let bound = (0..parameters.num_columns())
            .map(|place| EpisodeUuids::read(parameters.column(place).as_ref()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(bound, [vec![ids[0].clone()], vec![ids[1].clone()]]);
        assert!(EpisodeTopicQuery::for_episodes(&[], "SELECT 1".into()).is_err());
    }

    #[test]
    fn member_ids_travel_as_uuids_and_read_back_as_they_went() {
        let ids = [
            EpisodeId::try_from("01900000000070008000000000000001").unwrap(),
            EpisodeId::try_from("01a0b63c7b7f779b9237d4e0206c08ff").unwrap(),
        ];
        let array = EpisodeUuids::array(&ids).unwrap();
        assert_eq!(EpisodeUuids::read(array.as_ref()).unwrap(), ids);
        let text = arrow_array::StringArray::from(vec!["01900000000070008000000000000001"]);
        assert!(EpisodeUuids::read(&text).is_err());
    }
}
