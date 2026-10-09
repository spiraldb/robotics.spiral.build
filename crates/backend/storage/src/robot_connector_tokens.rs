//! The token each workspace issued for its robot connector to link with.
//! Studio shows a token once and keeps only its SHA-256, so the database
//! never holds a token that links.

use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _, Set, sea_query::OnConflict};
use sha2::{Digest as _, Sha256};

use crate::StudioStore;
use crate::studio::robot_connector_tokens as table;

/// The robot connector tokens of the workspaces on one Spiral endpoint.
#[derive(Clone)]
pub struct RobotConnectorTokens {
    studio: StudioStore,
    endpoint: String,
}

impl RobotConnectorTokens {
    pub fn new(studio: StudioStore, endpoint: impl Into<String>) -> Self {
        Self {
            studio,
            endpoint: endpoint.into(),
        }
    }

    /// Make `token` the one the workspace on `project_id` links with,
    /// replacing the token it issued before.
    pub async fn replace(&self, project_id: &str, token: &str) -> Result<(), String> {
        let row = table::ActiveModel {
            endpoint: Set(self.endpoint.clone()),
            project_id: Set(project_id.to_owned()),
            token_sha256: Set(Self::digest(token)),
        };
        table::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([table::Column::Endpoint, table::Column::ProjectId])
                    .update_column(table::Column::TokenSha256)
                    .to_owned(),
            )
            .exec_without_returning(self.studio.connection().await?)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Whether the workspace on `project_id` has issued a token.
    pub async fn issued(&self, project_id: &str) -> Result<bool, String> {
        let row = table::Entity::find_by_id((self.endpoint.clone(), project_id.to_owned()))
            .one(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(row.is_some())
    }

    /// The project of the workspace whose current token is `token`.
    pub async fn workspace(&self, token: &str) -> Result<Option<String>, String> {
        let row = table::Entity::find()
            .filter(table::Column::Endpoint.eq(&self.endpoint))
            .filter(table::Column::TokenSha256.eq(Self::digest(token)))
            .one(self.studio.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(row.map(|row| row.project_id))
    }

    fn digest(token: &str) -> Vec<u8> {
        Sha256::digest(token.as_bytes()).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceStore;
    use spiral_robotics_common::workspace::Workspace;

    const ENDPOINT: &str = "https://api.spiraldb.dev";

    #[tokio::test]
    async fn a_workspace_links_with_its_latest_token_until_it_is_deleted() {
        let studio = StudioStore::in_memory();
        let workspaces = WorkspaceStore::new(studio.clone(), ENDPOINT);
        let tokens = RobotConnectorTokens::new(studio.clone(), ENDPOINT);
        for project in ["arms-000001", "bench-000002"] {
            assert!(
                workspaces
                    .insert(&Workspace::new(project, project))
                    .await
                    .unwrap()
            );
        }
        assert!(!tokens.issued("arms-000001").await.unwrap());
        // Only a workspace issues a token.
        assert!(
            tokens
                .replace("missing-000003", "rc_missing")
                .await
                .is_err()
        );

        tokens.replace("arms-000001", "rc_first").await.unwrap();
        tokens.replace("bench-000002", "rc_bench").await.unwrap();
        assert!(tokens.issued("arms-000001").await.unwrap());
        assert_eq!(
            tokens.workspace("rc_first").await.unwrap().as_deref(),
            Some("arms-000001")
        );
        // Another endpoint's workspaces are another backend's.
        assert_eq!(
            RobotConnectorTokens::new(studio.clone(), "https://other.spiraldb.dev")
                .workspace("rc_first")
                .await
                .unwrap(),
            None
        );

        tokens.replace("arms-000001", "rc_second").await.unwrap();
        assert_eq!(tokens.workspace("rc_first").await.unwrap(), None);
        assert_eq!(
            tokens.workspace("rc_second").await.unwrap().as_deref(),
            Some("arms-000001")
        );

        assert!(workspaces.delete("arms-000001").await.unwrap());
        assert!(!tokens.issued("arms-000001").await.unwrap());
        assert_eq!(tokens.workspace("rc_second").await.unwrap(), None);
        assert_eq!(
            tokens.workspace("rc_bench").await.unwrap().as_deref(),
            Some("bench-000002")
        );
    }
}
