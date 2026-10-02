//! The Modal service that scores progress: the backend reads one episode's
//! camera packets from Spiral, posts them, and reads the scores back as the
//! answer the Score progress node consumes, `spql.ai.warp_rm`'s shape.
//!
//! `POST {base_url}/score_progress`, with the saved token as a bearer, takes
//! one Arrow IPC record batch: a row per packet in decode order — `payload`,
//! the Annex-B access unit as recorded; `decode_index`; `pts_ns` from the
//! episode's first message — with the codec, framing, and checkpoint in its
//! schema metadata. It answers a row per picture in display order. A video it
//! rejects (422) is that episode's failed answer; a refused token (401) ends
//! the run; anything that may pass (429, 5xx, a timeout, no connection) is
//! tried again after a pause.
use arrow_array::{Array, ArrayRef, RecordBatch, StructArray};
use arrow_schema::{DataType, Field, Fields, Schema};
use reqwest::StatusCode;
use spiral_robotics_storage::ProviderKeys;
use spiral_robotics_storage::answers::EpisodeAnswer;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use url::Url;

/// How many times one episode is posted before the run gives up on it.
const ATTEMPTS: u32 = 5;
/// The pause before the second attempt; each later one waits twice as long.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
/// How long one scoring may take, a cold start and a long recording
/// included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// What the service made of one episode's video.
pub(super) enum ProgressScores {
    Scored(EpisodeAnswer),
    Rejected(String),
}

/// The Modal service as the saved keys reach it, sharing the project's
/// permits so no more than a few requests are in flight across its runs.
pub(super) struct ProgressService {
    client: reqwest::Client,
    scoring: Url,
    token: String,
    permits: Arc<Semaphore>,
}

impl ProgressService {
    /// The service the saved keys name; missing or malformed settings are
    /// said plainly.
    pub(super) fn of(keys: &ProviderKeys, permits: Arc<Semaphore>) -> Result<Self, String> {
        let (Some(base_url), Some(token)) = (keys.modal_base_url(), keys.modal_token()) else {
            return Err(
                "Score progress needs the Modal service: add its base URL and token in Settings → Keys."
                    .into(),
            );
        };
        let base = Url::parse(base_url)
            .map_err(|error| format!("The Modal base URL is invalid: {error}"))?;
        let local = matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base.scheme() != "https" && !(base.scheme() == "http" && local) {
            return Err(
                "The Modal base URL must use HTTPS (HTTP is allowed only for localhost).".into(),
            );
        }
        let scoring = Url::parse(&format!(
            "{}/score_progress",
            base.as_str().trim_end_matches('/')
        ))
        .map_err(|error| format!("The Modal base URL is invalid: {error}"))?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            scoring,
            token: token.to_owned(),
            permits,
        })
    }

    /// The type of one episode's scores: a list of every picture's.
    pub(super) fn answer_type() -> DataType {
        DataType::List(Arc::new(Field::new_list_field(
            DataType::Struct(Self::score_fields()),
            true,
        )))
    }

    /// Score one episode's `packets` — `payload`, `decode_index`, `pts_ns`,
    /// in decode order — with the checkpoint `model` on the service
    /// `revision`.
    pub(super) async fn score(
        &self,
        packets: &RecordBatch,
        model: &str,
        revision: &str,
    ) -> Result<ProgressScores, String> {
        let body = Self::request_body(packets, model, revision)?;
        let mut pause = FIRST_PAUSE;
        let mut last_failure = String::new();
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                tokio::time::sleep(pause).await;
                pause *= 2;
            }
            let permit = self
                .permits
                .acquire()
                .await
                .map_err(|error| error.to_string())?;
            let sent = self
                .client
                .post(self.scoring.clone())
                .bearer_auth(&self.token)
                .header(reqwest::header::CONTENT_TYPE, ARROW_STREAM)
                .body(body.clone())
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(error) if error.is_timeout() || error.is_connect() || error.is_request() => {
                    last_failure = error.to_string();
                    continue;
                }
                Err(error) => return Err(format!("Calling the Modal service: {error}")),
            };
            let status = response.status();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) if error.is_timeout() || error.is_body() => {
                    last_failure = error.to_string();
                    continue;
                }
                Err(error) => return Err(format!("Reading the Modal service's answer: {error}")),
            };
            drop(permit);
            match status {
                StatusCode::OK => return Self::scores(&bytes).map(ProgressScores::Scored),
                StatusCode::UNPROCESSABLE_ENTITY => {
                    return Ok(ProgressScores::Rejected(Self::rejection(&bytes)));
                }
                StatusCode::CONFLICT => {
                    return Err(format!(
                        "The Modal service runs another revision than this Studio stores: {}",
                        Self::rejection(&bytes)
                    ));
                }
                StatusCode::UNAUTHORIZED => {
                    return Err(
                        "The Modal service refused the token; check it in Settings → Keys.".into(),
                    );
                }
                status if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() => {
                    last_failure = format!("the service answered {status}");
                }
                status => {
                    return Err(format!(
                        "The Modal service answered {status}: {}",
                        String::from_utf8_lossy(&bytes)
                    ));
                }
            }
        }
        Err(format!(
            "The Modal service did not score the episode after {ATTEMPTS} attempts: {last_failure}"
        ))
    }

    /// The fields of one picture's score, as the contract types them.
    fn score_fields() -> Fields {
        Fields::from(vec![
            Field::new("frame_index", DataType::UInt32, false),
            Field::new("timestamp_ns", DataType::UInt64, false),
            Field::new("velocity", DataType::Float32, false),
            Field::new("progress", DataType::Float32, true),
            Field::new("window_stride", DataType::UInt32, false),
            Field::new("support", DataType::UInt32, false),
        ])
    }

    /// The packets as the one record batch the service reads.
    fn request_body(packets: &RecordBatch, model: &str, revision: &str) -> Result<Vec<u8>, String> {
        let column = |name: &str, kind: &DataType| -> Result<ArrayRef, String> {
            let column = packets
                .column_by_name(name)
                .ok_or_else(|| format!("The packets have no {name}."))?;
            let cast = arrow_cast::cast(column, kind).map_err(|error| error.to_string())?;
            if cast.null_count() > 0 {
                return Err(format!("A packet has no {name}."));
            }
            Ok(cast)
        };
        let metadata = HashMap::from([
            ("codec".to_owned(), "h264".to_owned()),
            ("framing".to_owned(), "annex_b".to_owned()),
            ("model".to_owned(), model.to_owned()),
            ("implementation".to_owned(), revision.to_owned()),
        ]);
        let schema = Arc::new(
            Schema::new(vec![
                Field::new("payload", DataType::Binary, false),
                Field::new("decode_index", DataType::Int64, false),
                Field::new("pts_ns", DataType::Int64, false),
            ])
            .with_metadata(metadata),
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                column("payload", &DataType::Binary)?,
                column("decode_index", &DataType::Int64)?,
                column("pts_ns", &DataType::Int64)?,
            ],
        )
        .map_err(|error| error.to_string())?;
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(Vec::new(), &schema)
            .map_err(|error| error.to_string())?;
        writer.write(&batch).map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
        writer.into_inner().map_err(|error| error.to_string())
    }

    /// The scores the service answered, one per picture, as one list answer.
    fn scores(stream: &[u8]) -> Result<EpisodeAnswer, String> {
        let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(stream), None)
            .map_err(|error| format!("Reading the Modal service's scores: {error}"))?;
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Reading the Modal service's scores: {error}"))?;
        let fields = Self::score_fields();
        let mut columns = Vec::with_capacity(fields.len());
        for field in fields.iter() {
            let parts = batches
                .iter()
                .map(|batch| {
                    batch
                        .column_by_name(field.name())
                        .filter(|column| column.data_type() == field.data_type())
                        .ok_or_else(|| {
                            format!(
                                "The Modal service's scores have no {} of type {}.",
                                field.name(),
                                field.data_type()
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let parts = parts.iter().map(|part| part.as_ref()).collect::<Vec<_>>();
            let column = if parts.is_empty() {
                arrow_array::new_empty_array(field.data_type())
            } else {
                arrow_select::concat::concat(&parts).map_err(|error| error.to_string())?
            };
            if !field.is_nullable() && column.null_count() > 0 {
                return Err(format!("A score has no {}.", field.name()));
            }
            columns.push(column);
        }
        let scores =
            StructArray::try_new(fields, columns, None).map_err(|error| error.to_string())?;
        Ok(EpisodeAnswer::listing(Arc::new(scores)))
    }

    /// Why the service rejected a video: its `error`, or what it said.
    fn rejection(body: &[u8]) -> String {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
    }
}

#[cfg(test)]
pub(super) mod tests;
