//! Storing a Rollout episode: its MCAP file decoded and written as one
//! batch of one episode into its MCAP dataset, which its first episode
//! creates. Storing the same file at its key again is an idempotent retry;
//! other bytes at that key are refused.
use super::files::McapFile;
use super::writer::{DatasetWrite, DecodedEpisode};
use crate::db::ArrowColumns;
use crate::{EpisodeName, ProjectConnection, StorageManager};
use spiral_robotics_common::datasets::{DatasetFormat, DatasetSelection};
use spiral_robotics_common::sql::SqlLiteral;

impl StorageManager {
    /// Store one episode the robot connector recorded in its MCAP dataset,
    /// creating the dataset with its first episode.
    pub async fn store_recording(&self, name: &EpisodeName, bytes: &[u8]) -> Result<(), String> {
        let dataset = DatasetSelection::new(name.dataset())?;
        let catalog = self.project_store().dataset_catalog().datasets().await?;
        match catalog.summary(&dataset) {
            Some(summary) if summary.format() != DatasetFormat::Mcap => {
                return Err(format!(
                    "{} is a LeRobot dataset; record into an MCAP dataset",
                    dataset.display_name()
                ));
            }
            None if catalog.holds(&dataset) => {
                return Err(format!(
                    "{} holds tables but no episodes; delete them before recording into it",
                    dataset.label()
                ));
            }
            _ => {}
        }
        let key = format!("{}.mcap", name.episode());
        let file = McapFile::held(&key, bytes)?;
        let sha256 = file.sha256().to_owned();
        let bytes = bytes.to_vec();
        let episode = tokio::task::spawn_blocking(move || DecodedEpisode::of_mcap(file, &bytes))
            .await
            .map_err(|error| format!("decoding the recording failed: {error}"))??;
        let mut connection = self.project_store().connect_exclusive().await?;
        connection
            .transaction("recording store", async |connection| {
                // Whether the dataset exists is read again here, where it
                // cannot change before the commit: another recording may
                // have created it since.
                let exists = connection.table_exists(dataset.episodes().name()).await?;
                let mut write = if !exists {
                    DatasetWrite::create(connection, &dataset, DatasetFormat::Mcap).await?
                } else {
                    if StoredFile::read(connection, &dataset, &key)
                        .await?
                        .holds(&sha256)?
                    {
                        return Ok(());
                    }
                    DatasetWrite::open(connection, &dataset, DatasetFormat::Mcap).await?
                };
                write.write(connection, vec![episode]).await?;
                write.finish(connection).await
            })
            .await
    }
}

/// What a dataset already holds at one key: the SHA-256 of each file stored
/// there, which is at most one.
struct StoredFile {
    sha256: Vec<Option<String>>,
}

impl StoredFile {
    async fn read(
        connection: &mut ProjectConnection,
        dataset: &DatasetSelection,
        key: &str,
    ) -> Result<Self, String> {
        let batches = connection
            .read_batches(&format!(
                "SELECT sha256 FROM {} WHERE key = {} LIMIT 2",
                dataset.episodes(),
                SqlLiteral::text(key)
            ))
            .await?;
        let mut sha256 = Vec::new();
        for batch in &batches {
            let columns = ArrowColumns::new(batch, "episodes");
            for row in 0..batch.num_rows() {
                sha256.push(columns.optional_text("sha256", row)?.map(str::to_owned));
            }
        }
        Ok(Self { sha256 })
    }

    /// Whether the key holds exactly the file whose SHA-256 is `sha256`, so
    /// storing it again changes nothing; another file there is refused.
    fn holds(&self, sha256: &str) -> Result<bool, String> {
        match self.sha256.as_slice() {
            [] => Ok(false),
            [Some(stored)] if stored == sha256 => Ok(true),
            [_] => Err("Refusing to replace an immutable MCAP file with different bytes".into()),
            _ => Err("Duplicate episodes share this key; repair the dataset before storing".into()),
        }
    }
}

#[cfg(test)]
mod tests;
