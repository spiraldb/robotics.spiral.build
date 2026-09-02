//! Framework-neutral data settings, query, and remote replay actions.

pub(crate) mod catalog;
pub(crate) mod catalog_lease;
pub(crate) mod connection;
pub(crate) mod episode_arms;
pub(crate) mod queries;
pub(crate) mod seek;

use std::collections::BTreeMap;

use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use url::Url;

use crate::OperationError;

const MAX_QUERY_ROWS: usize = 1_000;

struct HttpEndpoint(String);

impl HttpEndpoint {
    fn parse(endpoint: &str, label: &str) -> Result<Self, OperationError> {
        let parsed = Url::parse(endpoint.trim()).map_err(|error| {
            OperationError::invalid_input(format!(
                "{label} must be an http:// or https:// URL: {error}"
            ))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(OperationError::invalid_input(format!(
                "{label} must be an http:// or https:// URL with a host"
            )));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(OperationError::invalid_input(format!(
                "{label} must not contain embedded credentials"
            )));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(OperationError::invalid_input(format!(
                "{label} must not contain a query string or fragment"
            )));
        }
        Ok(Self(parsed.as_str().trim_end_matches('/').to_owned()))
    }

    fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SpiralDestinationSettings {
    endpoint: String,
    spfs_endpoint: String,
    project_id: String,
    client_id: String,
    client_secret_configured: bool,
    namespace: String,
    endpoint_restart_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    spiral_endpoint: String,
    spiral_spfs_endpoint: String,
    project_id: String,
    client_id: String,
    /// `None` preserves the saved secret; `Some("")` clears it.
    client_secret: Option<String>,
    training_endpoint: String,
    video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsSnapshot {
    spiral: SpiralDestinationSettings,
    training_endpoint: String,
    video_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsUpdate {
    settings: DataSettingsSnapshot,
    project_state_rehydrated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionCheck {
    ok: bool,
    message: String,
    project_state_rehydrated: bool,
}

pub(crate) struct TestConnectionCommand;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryValue {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Array(Vec<QueryValue>),
    Object(BTreeMap<String, QueryValue>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryColumn {
    name: String,
    data_type: String,
    nullable: bool,
    semantic_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryResult {
    columns: Vec<QueryColumn>,
    rows: Vec<BTreeMap<String, QueryValue>>,
    truncated: bool,
    elapsed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryCommand {
    statement: String,
    max_rows: usize,
}

impl SpiralDestinationSettings {
    pub(crate) fn new(
        endpoint: impl Into<String>,
        spfs_endpoint: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            spfs_endpoint: spfs_endpoint.into(),
            project_id: project_id.into(),
            client_id: String::new(),
            client_secret_configured: false,
            namespace: String::new(),
            endpoint_restart_required: false,
        }
    }

    pub(crate) fn with_client(
        mut self,
        client_id: impl Into<String>,
        secret_configured: bool,
    ) -> Self {
        self.client_id = client_id.into();
        self.client_secret_configured = secret_configured;
        self
    }

    pub(crate) fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub(crate) fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }
    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    pub(crate) fn client_secret_configured(&self) -> bool {
        self.client_secret_configured
    }
    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }
    pub(crate) fn endpoint_restart_required(&self) -> bool {
        self.endpoint_restart_required
    }
}

impl UpdateDataSettingsCommand {
    pub(crate) fn new(
        spiral_endpoint: impl Into<String>,
        project_id: impl Into<String>,
        training_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            spiral_endpoint: spiral_endpoint.into(),
            spiral_spfs_endpoint: String::new(),
            project_id: project_id.into(),
            client_id: String::new(),
            client_secret: None,
            training_endpoint: training_endpoint.into(),
            video_enabled: false,
        }
    }

    pub(crate) fn with_spfs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.spiral_spfs_endpoint = endpoint.into();
        self
    }

    pub(crate) fn with_client(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = client_id.into();
        self
    }

    pub(crate) fn with_client_secret(mut self, secret: Option<String>) -> Self {
        self.client_secret = secret;
        self
    }

    pub(crate) fn with_video_enabled(mut self, enabled: bool) -> Self {
        self.video_enabled = enabled;
        self
    }

    pub(crate) fn spiral_endpoint(&self) -> &str {
        &self.spiral_endpoint
    }
    pub(crate) fn spiral_spfs_endpoint(&self) -> &str {
        &self.spiral_spfs_endpoint
    }
    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    pub(crate) fn client_secret(&self) -> Option<&str> {
        self.client_secret.as_deref()
    }
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl DataSettingsSnapshot {
    pub(crate) fn new(
        spiral: SpiralDestinationSettings,
        training_endpoint: impl Into<String>,
        video_enabled: bool,
    ) -> Self {
        Self {
            spiral,
            training_endpoint: training_endpoint.into(),
            video_enabled,
        }
    }

    pub(crate) fn spiral(&self) -> &SpiralDestinationSettings {
        &self.spiral
    }
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl DataSettingsUpdate {
    pub(crate) fn new(settings: DataSettingsSnapshot, project_state_rehydrated: bool) -> Self {
        Self {
            settings,
            project_state_rehydrated,
        }
    }

    pub(crate) fn into_parts(self) -> (DataSettingsSnapshot, bool) {
        (self.settings, self.project_state_rehydrated)
    }
}

impl ConnectionCheck {
    pub(crate) fn new(ok: bool, message: impl Into<String>) -> Self {
        Self {
            ok,
            message: message.into(),
            project_state_rehydrated: false,
        }
    }

    fn with_rehydrated_project_state(mut self) -> Self {
        self.project_state_rehydrated = true;
        self
    }

    pub(crate) fn into_parts(self) -> (bool, String, bool) {
        (self.ok, self.message, self.project_state_rehydrated)
    }
}

impl QueryColumn {
    pub(crate) fn new(
        name: impl Into<String>,
        data_type: impl Into<String>,
        nullable: bool,
    ) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            semantic_type: None,
        }
    }

    pub(crate) fn with_semantic_type(mut self, semantic_type: Option<String>) -> Self {
        self.semantic_type = semantic_type;
        self
    }

    pub(crate) fn into_parts(self) -> (String, String, bool, Option<String>) {
        (self.name, self.data_type, self.nullable, self.semantic_type)
    }
}

impl QueryResult {
    pub(crate) fn new(columns: Vec<QueryColumn>, rows: Vec<BTreeMap<String, QueryValue>>) -> Self {
        Self {
            columns,
            rows,
            truncated: false,
            elapsed_ms: 0,
        }
    }

    pub(crate) fn with_status(mut self, truncated: bool, elapsed_ms: u64) -> Self {
        self.truncated = truncated;
        self.elapsed_ms = elapsed_ms;
        self
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Vec<QueryColumn>,
        Vec<BTreeMap<String, QueryValue>>,
        bool,
        u64,
    ) {
        (self.columns, self.rows, self.truncated, self.elapsed_ms)
    }
}

impl QueryCommand {
    pub(crate) fn new(statement: impl Into<String>, max_rows: usize) -> Self {
        Self {
            statement: statement.into(),
            max_rows,
        }
    }

    pub(crate) fn statement(&self) -> &str {
        &self.statement
    }
    pub(crate) fn max_rows(&self) -> usize {
        self.max_rows
    }
}

/// Data-catalog consequences expressed only in capability-owned models.
pub(crate) trait DataCatalogPort: Send + Sync {
    fn settings(&self) -> DataSettingsSnapshot;
    async fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsUpdate, OperationError>;
    async fn refresh_training_readiness(&self);

    async fn test_connection(&self) -> ConnectionCheck;
    async fn recover_project_state(&self) -> Result<(), OperationError>;
    async fn query(&self, command: QueryCommand) -> Result<QueryResult, OperationError>;
}

impl UpdateDataSettingsCommand {
    pub(crate) async fn apply<P: DataCatalogPort + ?Sized>(
        self,
        port: &P,
    ) -> Result<DataSettingsUpdate, OperationError> {
        let command = self.validated()?;
        let update = port.update_settings(command).await?;
        // Training readiness depends on the selected runtime endpoint, so publish
        // preferences before rebuilding it.
        port.refresh_training_readiness().await;
        Ok(update)
    }
}

impl TestConnectionCommand {
    pub(crate) async fn execute<P: DataCatalogPort + ?Sized>(port: &P) -> ConnectionCheck {
        let check = port.test_connection().await;
        if !check.ok {
            return check;
        }
        match port.recover_project_state().await {
            Ok(()) => check.with_rehydrated_project_state(),
            Err(error) => ConnectionCheck::new(
                false,
                format!(
                    "SpiralDB provisioning succeeded, but Studio could not reload the project state: {error}"
                ),
            ),
        }
    }
}

impl QueryCommand {
    pub(crate) async fn execute<P: DataCatalogPort + ?Sized>(
        mut self,
        port: &P,
    ) -> Result<QueryResult, OperationError> {
        ReadOnlyQuery::new(&self.statement, ReadOnlyQueryMode::Preview)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.max_rows = self.max_rows.clamp(1, MAX_QUERY_ROWS);
        port.query(self).await
    }
}

impl UpdateDataSettingsCommand {
    fn validated(mut self) -> Result<Self, OperationError> {
        self.spiral_endpoint =
            HttpEndpoint::parse(&self.spiral_endpoint, "Spiral API endpoint")?.into_string();
        self.spiral_spfs_endpoint =
            HttpEndpoint::parse(&self.spiral_spfs_endpoint, "Spiral SpFS endpoint")?.into_string();
        self.project_id = self.project_id.trim().to_owned();
        self.client_id = self.client_id.trim().to_owned();
        self.client_secret = self.client_secret.map(|secret| secret.trim().to_owned());
        self.training_endpoint = self.training_endpoint.trim().to_owned();

        if self.project_id.is_empty()
            || !self
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(OperationError::InvalidInput(
                "Spiral project must contain only letters, numbers, '_' or '-'".to_owned(),
            ));
        }

        if self.client_id.is_empty()
            && self
                .client_secret
                .as_ref()
                .is_some_and(|secret| !secret.is_empty())
        {
            return Err(OperationError::InvalidInput(
                "Spiral client secret requires a client ID".to_owned(),
            ));
        }

        self.training_endpoint =
            HttpEndpoint::parse(&self.training_endpoint, "Training runtime endpoint")?
                .into_string();

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeData {
        events: Mutex<Vec<String>>,
        update_error: Option<OperationError>,
        update_rehydrated: bool,
        connection_check: ConnectionCheck,
        recovery_error: Option<OperationError>,
        query_error: Option<OperationError>,
    }

    impl FakeData {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                update_error: None,
                update_rehydrated: false,
                connection_check: ConnectionCheck::new(true, "connected"),
                recovery_error: None,
                query_error: None,
            }
        }
    }

    impl DataCatalogPort for FakeData {
        fn settings(&self) -> DataSettingsSnapshot {
            settings_view("settings")
        }

        async fn update_settings(
            &self,
            command: UpdateDataSettingsCommand,
        ) -> Result<DataSettingsUpdate, OperationError> {
            self.events.lock().unwrap().push(format!(
                "update:{}:{}:{}:{}:{}",
                command.spiral_endpoint,
                command.spiral_spfs_endpoint,
                command.project_id,
                command.training_endpoint,
                command.video_enabled
            ));
            self.update_error.clone().map_or_else(
                || {
                    Ok(DataSettingsUpdate::new(
                        settings_view("saved"),
                        self.update_rehydrated,
                    ))
                },
                Err,
            )
        }

        async fn refresh_training_readiness(&self) {
            self.events.lock().unwrap().push("refresh".to_owned());
        }

        async fn test_connection(&self) -> ConnectionCheck {
            self.events.lock().unwrap().push("test".to_owned());
            self.connection_check.clone()
        }

        async fn recover_project_state(&self) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("recover".to_owned());
            self.recovery_error.clone().map_or(Ok(()), Err)
        }

        async fn query(&self, command: QueryCommand) -> Result<QueryResult, OperationError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("query:{}:{}", command.statement, command.max_rows));
            self.query_error.clone().map_or_else(
                || {
                    Ok(QueryResult {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        truncated: false,
                        elapsed_ms: 1,
                    })
                },
                Err,
            )
        }
    }

    fn settings_view(project: &str) -> DataSettingsSnapshot {
        DataSettingsSnapshot {
            spiral: SpiralDestinationSettings {
                endpoint: "https://api.spiraldb.dev".to_owned(),
                spfs_endpoint: "https://spfs.spiraldb.dev".to_owned(),
                project_id: project.to_owned(),
                client_id: String::new(),
                client_secret_configured: false,
                namespace: "dataset".to_owned(),
                endpoint_restart_required: false,
            },
            training_endpoint: "http://127.0.0.1:50051".to_owned(),
            video_enabled: true,
        }
    }

    fn settings() -> UpdateDataSettingsCommand {
        UpdateDataSettingsCommand {
            spiral_endpoint: " https://api.spiraldb.dev/ ".to_owned(),
            spiral_spfs_endpoint: " https://spfs.spiraldb.dev/ ".to_owned(),
            project_id: " project ".to_owned(),
            client_id: String::new(),
            client_secret: None,
            training_endpoint: " http://127.0.0.1:50051 ".to_owned(),
            video_enabled: true,
        }
    }

    #[tokio::test]
    async fn settings_are_validated_normalized_saved_then_readiness_is_refreshed() {
        let data = FakeData::new();
        assert_eq!(
            settings().apply(&data).await,
            Ok(DataSettingsUpdate::new(settings_view("saved"), false))
        );
        assert_eq!(
            *data.events.lock().unwrap(),
            [
                "update:https://api.spiraldb.dev:https://spfs.spiraldb.dev:project:http://127.0.0.1:50051:true",
                "refresh"
            ]
        );
    }

    #[tokio::test]
    async fn successful_spiral_test_recovers_project_state() {
        let data = FakeData::new();

        let result = TestConnectionCommand::execute(&data).await;

        assert_eq!(result.into_parts(), (true, "connected".to_owned(), true));
        assert_eq!(*data.events.lock().unwrap(), ["test", "recover"]);
    }

    #[tokio::test]
    async fn failed_spiral_test_does_not_touch_project_state() {
        let mut data = FakeData::new();
        data.connection_check = ConnectionCheck::new(false, "connection refused");

        let result = TestConnectionCommand::execute(&data).await;

        assert_eq!(
            result.into_parts(),
            (false, "connection refused".to_owned(), false)
        );
        assert_eq!(*data.events.lock().unwrap(), ["test"]);
    }

    #[tokio::test]
    async fn recovery_failure_does_not_report_a_successful_spiral_test() {
        let mut data = FakeData::new();
        data.recovery_error = Some(OperationError::External(
            "unknown table rollout_runs".to_owned(),
        ));

        let result = TestConnectionCommand::execute(&data).await;
        let (ok, message, project_state_rehydrated) = result.into_parts();

        assert!(!ok);
        assert!(!project_state_rehydrated);
        assert!(message.contains("provisioning succeeded"), "{message}");
        assert!(message.contains("unknown table rollout_runs"), "{message}");
        assert_eq!(*data.events.lock().unwrap(), ["test", "recover"]);
    }

    #[tokio::test]
    async fn invalid_or_unsaved_settings_never_refresh_training() {
        let data = FakeData::new();
        let mut invalid = settings();
        invalid.project_id = "not/a/project".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_endpoint = "file:///tmp/spiral".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.spiral_spfs_endpoint = "file:///tmp/spfs".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut invalid = settings();
        invalid.training_endpoint = "ftp://runtime.example".to_owned();
        assert!(matches!(
            invalid.apply(&data).await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());

        let mut data = FakeData::new();
        data.update_error = Some(OperationError::Internal("Spiral unavailable".to_owned()));
        assert_eq!(
            settings().apply(&data).await,
            Err(OperationError::Internal("Spiral unavailable".to_owned()))
        );
        assert_eq!(data.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn query_row_limits_are_owned_by_the_capability_action() {
        let data = FakeData::new();
        QueryCommand {
            statement: "SELECT * FROM episodes".to_owned(),
            max_rows: usize::MAX,
        }
        .execute(&data)
        .await
        .unwrap();
        assert_eq!(
            *data.events.lock().unwrap(),
            ["query:SELECT * FROM episodes:1000"]
        );
    }

    #[tokio::test]
    async fn unauthorized_query_is_rejected_before_the_adapter() {
        let data = FakeData::new();
        assert!(matches!(
            QueryCommand {
                statement: "DELETE FROM episodes".to_owned(),
                max_rows: 100,
            }
            .execute(&data)
            .await,
            Err(OperationError::InvalidInput(_))
        ));
        assert!(data.events.lock().unwrap().is_empty());
    }
}
