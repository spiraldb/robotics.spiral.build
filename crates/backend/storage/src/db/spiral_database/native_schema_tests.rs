//! Opt-in checks against a local Spiral API and SpFS, using only unique test tables.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use serde::Deserialize;

use super::{
    BatchIngestion, ConfiguredSpiralSession, DatabaseCommand, DatabaseError, DatabaseResult,
    IngestMode, RecordBatchStream, RelationName as DatabaseRelationName, SpiralConnection,
};
use crate::settings::{SpiralConnectionSettings, SpiralCredentials};

#[derive(Deserialize)]
struct LocalSpiralConfig {
    api_url: String,
    spfs_url: String,
    project_id: String,
    client_id: String,
    client_secret: String,
}

impl LocalSpiralConfig {
    fn load() -> SpiralConnectionSettings {
        let path = std::env::var("SPIRAL_SCHEMA_TEST_CONFIG")
            .expect("set SPIRAL_SCHEMA_TEST_CONFIG to a local Spiral client JSON file");
        let config: Self = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for endpoint in [&config.api_url, &config.spfs_url] {
            let url = url::Url::parse(endpoint).unwrap();
            assert!(matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]")
            ));
        }
        SpiralConnectionSettings::new(config.api_url, config.spfs_url)
            .unwrap()
            .with_project(&config.project_id)
            .unwrap()
            .with_credentials(Some(SpiralCredentials::new(
                config.client_id,
                config.client_secret,
            )))
    }
}

struct NativeSchemaFixture {
    configured: ConfiguredSpiralSession,
    connection: SpiralConnection,
    committed: DatabaseRelationName,
    pending: DatabaseRelationName,
}

impl NativeSchemaFixture {
    fn new() -> Self {
        let configured = ConfiguredSpiralSession::new(&LocalSpiralConfig::load()).unwrap();
        let connection = SpiralConnection::new(configured.open_connection().unwrap());
        let unique = format!(
            "{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        Self {
            configured,
            connection,
            committed: DatabaseRelationName::parse(format!("schema_inspection_{unique}")).unwrap(),
            pending: DatabaseRelationName::parse(format!("schema_inspection_pending_{unique}"))
                .unwrap(),
        }
    }

    fn ingestion(&self, mode: IngestMode) -> BatchIngestion {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![1]))])
            .unwrap();
        BatchIngestion::new(
            self.committed.clone(),
            mode,
            RecordBatchStream::new(
                schema,
                futures::stream::once(async move { Ok(batch) }).boxed(),
            ),
        )
    }

    async fn verify(&mut self) -> DatabaseResult<()> {
        assert!(
            self.connection
                .fetch_table_schema(self.committed.clone())
                .await?
                .is_none()
        );
        self.connection
            .ingest(self.ingestion(IngestMode::CreateOrAppend))
            .await?;
        let schema = self
            .connection
            .fetch_table_schema(self.committed.clone())
            .await?
            .unwrap();
        assert_eq!(
            schema.field_with_name("id").unwrap().data_type(),
            &DataType::Int64
        );
        self.connection
            .ingest(self.ingestion(IngestMode::Append))
            .await?;
        let mut observer = SpiralConnection::new(self.configured.open_connection()?);
        assert!(
            observer
                .fetch_table_schema(self.committed.clone())
                .await?
                .is_some()
        );
        let rows = observer
            .execute_command(DatabaseCommand::statement(format!(
                "SELECT id FROM {}",
                self.committed.as_str()
            )))
            .await?
            .into_records()?
            .try_collect::<Vec<_>>()
            .await?;
        assert_eq!(rows.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);

        self.connection
            .context
            .set_autocommit(false)
            .await
            .map_err(DatabaseError::from_external)?;
        self.connection
            .execute_command(DatabaseCommand::statement(format!(
                "CREATE TABLE {} ENGINE 'arbor' AS SELECT 2 AS id",
                self.pending.as_str()
            )))
            .await?;
        let transaction = self.connection.context.active_transaction().unwrap();
        assert!(
            self.connection
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_some()
        );
        assert!(
            observer
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_none()
        );
        assert!(
            self.connection
                .context
                .active_transaction()
                .unwrap()
                .is_same_transaction(&transaction)
        );
        self.connection
            .context
            .commit()
            .await
            .map_err(DatabaseError::from_external)?;
        assert!(
            observer
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_some()
        );

        self.connection
            .execute_command(DatabaseCommand::statement(format!(
                "DROP TABLE {}",
                self.pending.as_str()
            )))
            .await?;
        assert!(
            self.connection
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_none()
        );
        assert!(
            observer
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_some()
        );
        self.connection
            .context
            .rollback()
            .await
            .map_err(DatabaseError::from_external)?;
        assert!(
            self.connection
                .fetch_table_schema(self.pending.clone())
                .await?
                .is_some()
        );
        Ok(())
    }

    async fn cleanup(&mut self) -> DatabaseResult<()> {
        if !self.connection.context.autocommit() {
            self.connection
                .context
                .rollback()
                .await
                .map_err(DatabaseError::from_external)?;
        }
        let mut cleanup = SpiralConnection::new(self.configured.open_connection()?);
        for name in [&self.committed, &self.pending] {
            cleanup
                .execute_command(DatabaseCommand::statement(format!(
                    "DROP TABLE IF EXISTS {}",
                    name.as_str()
                )))
                .await?;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires SPIRAL_SCHEMA_TEST_CONFIG pointing to local Spiral API/SpFS client JSON"]
async fn native_schema_inspection_supports_ingestion_and_transaction_visibility() {
    let mut fixture = NativeSchemaFixture::new();
    let outcome = AssertUnwindSafe(fixture.verify()).catch_unwind().await;
    fixture
        .cleanup()
        .await
        .expect("remove only this test's tables");
    outcome.unwrap().unwrap();
}
