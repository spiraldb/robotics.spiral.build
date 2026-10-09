//! The finished episodes the workspaces' robot connectors uploaded, kept until a
//! signed-in browser stores each in its dataset: a connector's token links it
//! to a workspace, but writing to Spiral takes a user's access.

use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, ConnectionTrait, EntityTrait as _, QueryFilter as _,
    QueryOrder as _, Set, TransactionTrait as _,
};
use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::recording::SavedRecording;
use spiral_robotics_common::robot_connector::{EpisodePart, MAX_EPISODE_BYTES};

use crate::StudioStore;
use crate::studio::{staged_episode_parts as parts, staged_episodes as episodes};

/// An episode's dataset and episode, valid as names of a dataset and its file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpisodeName {
    dataset: String,
    episode: String,
}

impl EpisodeName {
    pub fn new(dataset: &str, episode: &str) -> Result<Self, String> {
        let dataset = DatasetId::try_from(dataset).map_err(|error| error.to_string())?;
        if episode.is_empty()
            || episode.len() > 64
            || !episode
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("episode ID must use 1–64 letters, numbers, '_' or '-' characters".into());
        }
        Ok(Self {
            dataset: String::from(dataset),
            episode: episode.to_owned(),
        })
    }

    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    pub fn episode(&self) -> &str {
        &self.episode
    }
}

/// What Studio did with a part of an episode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpisodeReceipt {
    /// Kept it; Studio holds this many bytes of the episode.
    Kept(u64),
    /// Left it: it starts elsewhere than where the episode's bytes end, at
    /// this many.
    Misplaced(u64),
}

/// Why Studio could not keep a part of an episode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StagedEpisodeError {
    /// The part can never be kept: sending it again fails again.
    Refused(String),
    /// Studio could not reach its database.
    Storage(String),
}

/// The staged episodes of the workspaces on one Spiral endpoint.
#[derive(Clone)]
pub struct StagedEpisodes {
    studio: StudioStore,
    endpoint: String,
}

impl StagedEpisodes {
    pub fn new(studio: StudioStore, endpoint: impl Into<String>) -> Self {
        Self {
            studio,
            endpoint: endpoint.into(),
        }
    }

    /// Keep `bytes` as the part of the workspace on `project_id`'s episode
    /// `name` that `part` places, if it starts where the bytes Studio holds
    /// end. The episode is whole once they add up to its size.
    pub async fn receive(
        &self,
        project_id: &str,
        name: &EpisodeName,
        part: EpisodePart,
        bytes: &[u8],
    ) -> Result<EpisodeReceipt, StagedEpisodeError> {
        if part.size() == 0 || part.size() > MAX_EPISODE_BYTES {
            return Err(StagedEpisodeError::Refused(format!(
                "an episode must have 1 to {MAX_EPISODE_BYTES} bytes, not {}",
                part.size()
            )));
        }
        let transaction = self
            .studio
            .connection()
            .await
            .map_err(StagedEpisodeError::Storage)?
            .begin()
            .await
            .map_err(Self::storage)?;
        let key = self.key(project_id, name);
        let stored = match episodes::Entity::find_by_id(key.clone())
            .one(&transaction)
            .await
            .map_err(Self::storage)?
        {
            Some(stored) => stored,
            None => episodes::ActiveModel {
                endpoint: Set(key.0.clone()),
                project_id: Set(key.1.clone()),
                dataset: Set(key.2.clone()),
                episode: Set(key.3.clone()),
                size: Set(Self::column(part.size())),
                received: Set(0),
            }
            .insert(&transaction)
            .await
            .map_err(Self::storage)?,
        };
        if stored.size != Self::column(part.size()) {
            return Err(StagedEpisodeError::Refused(format!(
                "Studio holds an episode {} of {} bytes, not {}",
                name.episode(),
                stored.size,
                part.size()
            )));
        }
        let received = stored.received as u64;
        if part.offset() != received {
            return Ok(EpisodeReceipt::Misplaced(received));
        }
        let length = bytes.len() as u64;
        if length == 0 || received + length > part.size() {
            return Err(StagedEpisodeError::Refused(format!(
                "a part of {length} bytes at {received} does not fit an episode of {} bytes",
                part.size()
            )));
        }
        parts::ActiveModel {
            endpoint: Set(key.0.clone()),
            project_id: Set(key.1.clone()),
            dataset: Set(key.2.clone()),
            episode: Set(key.3.clone()),
            start: Set(stored.received),
            bytes: Set(bytes.to_vec()),
        }
        .insert(&transaction)
        .await
        .map_err(Self::storage)?;
        let received = received + length;
        episodes::ActiveModel {
            received: Set(Self::column(received)),
            ..stored.into()
        }
        .update(&transaction)
        .await
        .map_err(Self::storage)?;
        transaction.commit().await.map_err(Self::storage)?;
        Ok(EpisodeReceipt::Kept(received))
    }

    /// The workspace on `project_id`'s whole episodes, by dataset and episode.
    pub async fn whole(&self, project_id: &str) -> Result<Vec<SavedRecording>, String> {
        let rows = episodes::Entity::find()
            .filter(episodes::Column::Endpoint.eq(&self.endpoint))
            .filter(episodes::Column::ProjectId.eq(project_id))
            .filter(
                sea_orm::sea_query::Expr::col(episodes::Column::Received)
                    .eq(sea_orm::sea_query::Expr::col(episodes::Column::Size)),
            )
            .order_by_asc(episodes::Column::Dataset)
            .order_by_asc(episodes::Column::Episode)
            .all(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| SavedRecording::new(row.dataset, row.episode, row.size as u64))
            .collect())
    }

    /// The bytes of the workspace on `project_id`'s episode `name`, once it is
    /// whole.
    pub async fn read(
        &self,
        project_id: &str,
        name: &EpisodeName,
    ) -> Result<Option<Vec<u8>>, String> {
        let connection = self.studio.connection().await?;
        let key = self.key(project_id, name);
        let whole = episodes::Entity::find_by_id(key.clone())
            .one(connection)
            .await
            .map_err(|error| error.to_string())?
            .is_some_and(|row| row.received == row.size);
        if !whole {
            return Ok(None);
        }
        let parts = Self::parts(connection, key).await?;
        Ok(Some(
            parts.into_iter().flat_map(|part| part.bytes).collect(),
        ))
    }

    /// Forget the workspace on `project_id`'s episode `name`, whole or not.
    pub async fn remove(&self, project_id: &str, name: &EpisodeName) -> Result<(), String> {
        episodes::Entity::delete_by_id(self.key(project_id, name))
            .exec(self.studio.connection().await?)
            .await
            .map(drop)
            .map_err(|error| error.to_string())
    }

    async fn parts(
        connection: &impl ConnectionTrait,
        key: (String, String, String, String),
    ) -> Result<Vec<parts::Model>, String> {
        parts::Entity::find()
            .filter(parts::Column::Endpoint.eq(key.0))
            .filter(parts::Column::ProjectId.eq(key.1))
            .filter(parts::Column::Dataset.eq(key.2))
            .filter(parts::Column::Episode.eq(key.3))
            .order_by_asc(parts::Column::Start)
            .all(connection)
            .await
            .map_err(|error| error.to_string())
    }

    fn key(&self, project_id: &str, name: &EpisodeName) -> (String, String, String, String) {
        (
            self.endpoint.clone(),
            project_id.to_owned(),
            name.dataset.clone(),
            name.episode.clone(),
        )
    }

    /// A byte count as SQLite keeps it. Episodes never exceed
    /// [`MAX_EPISODE_BYTES`], so every count fits.
    fn column(count: u64) -> i64 {
        i64::try_from(count).expect("an episode's byte count fits SQLite's integers")
    }

    fn storage(error: sea_orm::DbErr) -> StagedEpisodeError {
        StagedEpisodeError::Storage(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceStore;
    use spiral_robotics_common::workspace::Workspace;

    const ENDPOINT: &str = "https://api.spiraldb.dev";
    const PROJECT: &str = "arms-000001";

    async fn staged() -> (StagedEpisodes, WorkspaceStore) {
        let studio = StudioStore::in_memory();
        let workspaces = WorkspaceStore::new(studio.clone(), ENDPOINT);
        assert!(
            workspaces
                .insert(&Workspace::new(PROJECT, "Arms"))
                .await
                .unwrap()
        );
        (StagedEpisodes::new(studio, ENDPOINT), workspaces)
    }

    fn take() -> EpisodeName {
        EpisodeName::new("pick_place", "episode-1").unwrap()
    }

    #[tokio::test]
    async fn an_episode_arrives_in_order_and_is_whole_once_every_byte_has() {
        let (staged, _) = staged().await;
        let first = staged
            .receive(PROJECT, &take(), EpisodePart::new(0, 6), b"abc")
            .await
            .unwrap();
        assert_eq!(first, EpisodeReceipt::Kept(3));
        assert!(staged.whole(PROJECT).await.unwrap().is_empty());
        assert_eq!(staged.read(PROJECT, &take()).await.unwrap(), None);

        // A part sent again, or one that skips ahead, says where to resume.
        for offset in [0, 5] {
            assert_eq!(
                staged
                    .receive(PROJECT, &take(), EpisodePart::new(offset, 6), b"def")
                    .await
                    .unwrap(),
                EpisodeReceipt::Misplaced(3)
            );
        }
        assert_eq!(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(3, 6), b"def")
                .await
                .unwrap(),
            EpisodeReceipt::Kept(6)
        );
        assert_eq!(
            staged.whole(PROJECT).await.unwrap(),
            [SavedRecording::new("pick_place", "episode-1", 6)]
        );
        assert_eq!(
            staged.read(PROJECT, &take()).await.unwrap().as_deref(),
            Some(&b"abcdef"[..])
        );
        // An upload that resumes after the last part learns it is done.
        assert_eq!(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(0, 6), b"abc")
                .await
                .unwrap(),
            EpisodeReceipt::Misplaced(6)
        );

        staged.remove(PROJECT, &take()).await.unwrap();
        assert!(staged.whole(PROJECT).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn parts_that_cannot_belong_to_the_episode_are_refused() {
        let (staged, _) = staged().await;
        let refused = |result: Result<EpisodeReceipt, StagedEpisodeError>| {
            matches!(result, Err(StagedEpisodeError::Refused(_)))
        };
        assert!(refused(
            staged
                .receive(
                    PROJECT,
                    &take(),
                    EpisodePart::new(0, MAX_EPISODE_BYTES + 1),
                    b"a"
                )
                .await
        ));
        assert!(refused(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(0, 2), b"abc")
                .await
        ));
        staged
            .receive(PROJECT, &take(), EpisodePart::new(0, 4), b"ab")
            .await
            .unwrap();
        assert!(refused(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(2, 5), b"cd")
                .await
        ));
        assert!(refused(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(2, 4), b"")
                .await
        ));
        assert!(EpisodeName::new("pick_place", "../episode").is_err());
        assert!(EpisodeName::new("pick place", "episode-1").is_err());
    }

    #[tokio::test]
    async fn a_deleted_workspace_takes_its_episodes_with_it() {
        let (staged, workspaces) = staged().await;
        staged
            .receive(PROJECT, &take(), EpisodePart::new(0, 2), b"ab")
            .await
            .unwrap();
        assert!(workspaces.delete(PROJECT).await.unwrap());
        assert!(staged.whole(PROJECT).await.unwrap().is_empty());
        // A connector of a workspace that no longer exists keeps nothing.
        assert!(matches!(
            staged
                .receive(PROJECT, &take(), EpisodePart::new(0, 2), b"ab")
                .await,
            Err(StagedEpisodeError::Storage(_))
        ));
    }
}
