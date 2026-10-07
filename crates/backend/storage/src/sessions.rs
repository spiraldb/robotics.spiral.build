//! Each browser's Spiral sign-in, saved so a restart signs nobody out.

use crate::StudioStore;

/// One browser's saved sign-in: its Spiral OAuth tokens and the project it works on,
/// empty before one is chosen.
#[derive(Clone, Eq, PartialEq)]
pub struct StoredSession {
    access_token: String,
    refresh_token: String,
    project_id: String,
}

impl StoredSession {
    pub fn new(access_token: String, refresh_token: String, project_id: String) -> Self {
        Self {
            access_token,
            refresh_token,
            project_id,
        }
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredSession")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

/// Every browser's saved sign-in, by the session ID its cookie holds.
#[derive(Clone)]
pub struct SessionStore {
    studio: StudioStore,
}

impl SessionStore {
    pub fn new(studio: StudioStore) -> Self {
        Self { studio }
    }

    pub async fn load(&self, id: &str) -> Result<Option<StoredSession>, String> {
        use crate::studio::sessions as table;
        use sea_orm::EntityTrait;
        let row = table::Entity::find_by_id(id.to_owned())
            .one(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(row.map(|row| StoredSession::new(row.access_token, row.refresh_token, row.project_id)))
    }

    /// Save `session` under `id`, replacing what was saved there.
    pub async fn save(&self, id: &str, session: &StoredSession) -> Result<(), String> {
        use crate::studio::sessions as table;
        use sea_orm::{EntityTrait, Set, sea_query::OnConflict};
        let row = table::ActiveModel {
            id: Set(id.to_owned()),
            access_token: Set(session.access_token.clone()),
            refresh_token: Set(session.refresh_token.clone()),
            project_id: Set(session.project_id.clone()),
        };
        table::Entity::insert(row)
            .on_conflict(
                OnConflict::column(table::Column::Id)
                    .update_columns([
                        table::Column::AccessToken,
                        table::Column::RefreshToken,
                        table::Column::ProjectId,
                    ])
                    .to_owned(),
            )
            .exec(self.studio.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Forget the session `id`; one already gone is forgotten too.
    pub async fn remove(&self, id: &str) -> Result<(), String> {
        use crate::studio::sessions as table;
        use sea_orm::EntityTrait;
        table::Entity::delete_by_id(id.to_owned())
            .exec(self.studio.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sessions_round_trip_and_never_print_their_tokens() {
        let store = SessionStore::new(StudioStore::in_memory());
        assert!(store.load("browser").await.unwrap().is_none());
        let first = StoredSession::new("access-1".into(), "refresh-1".into(), String::new());
        store.save("browser", &first).await.unwrap();
        assert_eq!(store.load("browser").await.unwrap(), Some(first));
        let chosen = StoredSession::new("access-2".into(), "refresh-2".into(), "arms".into());
        store.save("browser", &chosen).await.unwrap();
        assert_eq!(store.load("browser").await.unwrap(), Some(chosen.clone()));
        assert!(!format!("{chosen:?}").contains("refresh-2"));
        store.remove("browser").await.unwrap();
        store.remove("browser").await.unwrap();
        assert!(store.load("browser").await.unwrap().is_none());
    }
}
