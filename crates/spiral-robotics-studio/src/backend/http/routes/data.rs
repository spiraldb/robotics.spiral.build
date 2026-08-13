use super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/settings/data",
            get(get_data_settings).patch(update_data_settings),
        )
        .route("/settings/data/test/spiral", post(test_spiral))
        .route("/publications", get(list_publications))
        .route(
            "/spiral/publications/{episode}",
            post(sync_existing_episode_to_spiral),
        )
        .route("/spiral/query", post(spiral_query))
        .route("/spiral/replay", post(start_spiral_replay))
        .route("/spiral/video.mp4", get(spiral_video))
}

pub(super) struct DataHttpAdapter<'a> {
    state: &'a AppState,
}

impl<'a> DataHttpAdapter<'a> {
    pub(super) fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl DataCatalogPort for DataHttpAdapter<'_> {
    fn settings(&self) -> DataSettingsView {
        application_data_settings(self.state.data_settings.public())
    }

    fn update_settings(
        &self,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsView, ApplicationError> {
        self.state
            .data_settings
            .update(UpdateDataSettingsRequest {
                spiral: ContractSpiralSettings {
                    enabled: false,
                    uri: command.spiral.uri,
                    project_id: command.spiral.project_id,
                    dataset: command.spiral.namespace,
                },
                video: VideoSettings {
                    enabled: command.video_enabled,
                },
            })
            .map(application_data_settings)
            .map_err(ApplicationError::InvalidInput)
    }

    async fn refresh_training_readiness(&self) {
        self.state.training.refresh_readiness().await;
    }

    async fn test_connection(&self) -> ConnectionCheck {
        let response = self.state.storage.test_spiral().await;
        ConnectionCheck {
            ok: response.ok,
            message: response.message,
        }
    }

    fn publications(&self) -> Vec<PublicationView> {
        self.state
            .storage
            .statuses()
            .into_iter()
            .map(|status| PublicationView {
                episode_id: status.episode_id,
                state: status.state,
                spiral: status.spiral,
                updated_ms: status.updated_ms,
            })
            .collect()
    }

    fn sync_existing_episode(&self, episode: String) -> Result<(), ApplicationError> {
        self.state
            .storage
            .sync_spiral_existing(episode)
            .map_err(ApplicationError::InvalidInput)
    }

    async fn query(
        &self,
        command: QueryCommand,
    ) -> Result<ApplicationQueryResult, ApplicationError> {
        self.state
            .storage
            .query(&command.statement, command.max_rows)
            .await
            .map(application_query_result)
            .map_err(ApplicationError::External)
    }

    async fn load_replay_trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<SpiralReplayTrajectory, ApplicationError> {
        let (sample_hz, frames) = self
            .state
            .storage
            .trajectory(episode, stream)
            .await
            .map_err(ApplicationError::External)?;
        Ok(SpiralReplayTrajectory { sample_hz, frames })
    }

    async fn video_asset(&self, query: VideoAssetQuery) -> Result<Vec<u8>, ApplicationError> {
        self.state
            .storage
            .video_asset(&query.episode, &query.stream, query.segment)
            .await
            .map_err(ApplicationError::External)
    }
}

pub(super) fn application_data_settings(settings: DataSettings) -> DataSettingsView {
    DataSettingsView {
        spiral: SpiralDestinationSettings {
            uri: settings.spiral.uri,
            project_id: settings.spiral.project_id,
            namespace: settings.spiral.dataset,
        },
        video_enabled: settings.video.enabled,
    }
}

pub(super) fn contract_data_settings(settings: DataSettingsView) -> DataSettings {
    DataSettings {
        spiral: ContractSpiralSettings {
            enabled: false,
            uri: settings.spiral.uri,
            project_id: settings.spiral.project_id,
            dataset: settings.spiral.namespace,
        },
        video: VideoSettings {
            enabled: settings.video_enabled,
        },
    }
}

pub(super) fn application_query_result(result: SpiralQueryResponse) -> ApplicationQueryResult {
    ApplicationQueryResult {
        columns: result
            .columns
            .into_iter()
            .map(|column| ApplicationQueryColumn {
                name: column.name,
                data_type: column.data_type,
                nullable: column.nullable,
            })
            .collect(),
        rows: result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| (name, application_query_value(value)))
                    .collect()
            })
            .collect(),
        truncated: result.truncated,
        elapsed_ms: result.elapsed_ms,
    }
}

fn application_query_value(value: Value) -> QueryValue {
    match value {
        Value::Null => QueryValue::Null,
        Value::Bool(value) => QueryValue::Bool(value),
        Value::Number(value) => value.as_i64().map_or_else(
            || {
                value.as_u64().map_or_else(
                    || QueryValue::Float(value.as_f64().unwrap_or(f64::NAN)),
                    QueryValue::Unsigned,
                )
            },
            QueryValue::Signed,
        ),
        Value::String(value) => QueryValue::String(value),
        Value::Array(values) => {
            QueryValue::Array(values.into_iter().map(application_query_value).collect())
        }
        Value::Object(values) => QueryValue::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, application_query_value(value)))
                .collect(),
        ),
    }
}

fn contract_query_value(value: QueryValue) -> Value {
    match value {
        QueryValue::Null => Value::Null,
        QueryValue::Bool(value) => Value::Bool(value),
        QueryValue::Signed(value) => Value::from(value),
        QueryValue::Unsigned(value) => Value::from(value),
        QueryValue::Float(value) => Value::from(value),
        QueryValue::String(value) => Value::String(value),
        QueryValue::Array(values) => {
            Value::Array(values.into_iter().map(contract_query_value).collect())
        }
        QueryValue::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, contract_query_value(value)))
                .collect(),
        ),
    }
}

pub(super) fn contract_query_result(result: ApplicationQueryResult) -> SpiralQueryResponse {
    SpiralQueryResponse {
        columns: result
            .columns
            .into_iter()
            .map(|column| spiral_robotics_contracts::api::QueryColumn {
                name: column.name,
                data_type: column.data_type,
                nullable: column.nullable,
            })
            .collect(),
        rows: result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| (name, contract_query_value(value)))
                    .collect()
            })
            .collect(),
        truncated: result.truncated,
        elapsed_ms: result.elapsed_ms,
    }
}

impl SpiralReplayPort for HardwareHandle {
    async fn start_spiral_replay(
        &self,
        command: StartSpiralReplayCommand,
        trajectory: SpiralReplayTrajectory,
    ) -> Result<(), ApplicationError> {
        HardwareHandle::start_replay(
            self,
            &command.robot,
            &command.episode,
            trajectory.frames,
            trajectory.sample_hz,
        )
        .await
        .map_err(ApplicationError::conflict)
    }
}

fn data_settings_command(request: UpdateDataSettingsRequest) -> UpdateDataSettingsCommand {
    UpdateDataSettingsCommand {
        spiral: SpiralDestinationSettings {
            uri: request.spiral.uri,
            project_id: request.spiral.project_id,
            namespace: request.spiral.dataset,
        },
        video_enabled: request.video.enabled,
    }
}

fn spiral_replay_command(request: StartSpiralReplayRequest) -> StartSpiralReplayCommand {
    StartSpiralReplayCommand {
        episode: request.episode,
        robot: request.robot,
        stream: request.stream,
    }
}

async fn get_data_settings(State(state): State<AppState>) -> Json<DataSettings> {
    Json(contract_data_settings(data_application::get_settings(
        &DataHttpAdapter { state: &state },
    )))
}

async fn update_data_settings(
    State(state): State<AppState>,
    Json(request): Json<UpdateDataSettingsRequest>,
) -> Result<Json<DataSettings>, Failure> {
    let settings = data_application::update_settings(
        &DataHttpAdapter { state: &state },
        data_settings_command(request),
    )
    .await?;
    Ok(Json(contract_data_settings(settings)))
}

async fn test_spiral(State(state): State<AppState>) -> Json<ConnectionTestResponse> {
    let result = data_application::test_connection(&DataHttpAdapter { state: &state }).await;
    Json(ConnectionTestResponse {
        ok: result.ok,
        message: result.message,
    })
}

async fn list_publications(State(state): State<AppState>) -> Json<Vec<PublicationStatus>> {
    Json(
        data_application::list_publications(&DataHttpAdapter { state: &state })
            .into_iter()
            .map(|status| PublicationStatus {
                episode_id: status.episode_id,
                state: status.state,
                spiral: status.spiral,
                updated_ms: status.updated_ms,
            })
            .collect(),
    )
}

async fn sync_existing_episode_to_spiral(
    State(state): State<AppState>,
    Path(episode): Path<String>,
) -> Result<StatusCode, Failure> {
    data_application::sync_existing_episode(&DataHttpAdapter { state: &state }, episode)?;
    Ok(StatusCode::ACCEPTED)
}

async fn spiral_query(
    State(state): State<AppState>,
    Json(request): Json<SpiralQueryRequest>,
) -> Result<Json<SpiralQueryResponse>, Failure> {
    let result = data_application::query(
        &DataHttpAdapter { state: &state },
        QueryCommand {
            statement: request.sql,
            max_rows: request.max_rows,
        },
    )
    .await?;
    Ok(Json(contract_query_result(result)))
}

async fn start_spiral_replay(
    State(state): State<AppState>,
    Json(request): Json<StartSpiralReplayRequest>,
) -> Result<StatusCode, Failure> {
    data_application::start_spiral_replay(
        &DataHttpAdapter { state: &state },
        state.hardware.as_ref(),
        spiral_replay_command(request),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct SpiralVideoQuery {
    episode: String,
    stream: String,
    segment: Option<u32>,
}

async fn spiral_video(
    State(state): State<AppState>,
    Query(query): Query<SpiralVideoQuery>,
) -> Result<Response, Failure> {
    let bytes = data_application::video_asset(
        &DataHttpAdapter { state: &state },
        VideoAssetQuery {
            episode: query.episode,
            stream: query.stream,
            segment: query.segment,
        },
    )
    .await
    .map_err(Failure::from)?;
    let mut response = Body::from(bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("video/mp4"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
