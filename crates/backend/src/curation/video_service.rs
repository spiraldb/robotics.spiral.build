//! A Modal service that answers one episode's video: the backend reads the
//! episode's camera packets from Spiral, posts them, and reads the rows the
//! service answers back as the episode's list answer. Score progress and
//! Track objects each ask one through this.
//!
//! `POST {base_url}/{route}`, with the deployment's token as a bearer, takes
//! one Arrow IPC record batch: a row per packet in decode order — `payload`,
//! the Annex-B access unit as recorded; `decode_index`; `pts_ns` from the
//! episode's first message — with the codec, the framing, and the question's
//! arguments (the checkpoint, the service revision, …) in its schema
//! metadata. It answers an Arrow IPC stream of the rows its contract types.
//! A video it rejects (422) is that episode's failed answer; a service on
//! another revision or a model it can't run (409), a model it doesn't know
//! (404), or a refused token (401) ends the run; anything
//! that may pass (408, 429, 5xx, a timeout, no connection) is tried again
//! after a pause: Modal answers 408 when it drops an upload part way.
use arrow_array::{Array, ArrayRef, RecordBatch, StructArray};
use arrow_schema::{ArrowError, DataType, Field, Fields, Schema};
use reqwest::StatusCode;
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
/// How long one answer may take, a cold start and a long recording included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// What a video service made of one episode's video.
pub(super) enum VideoAnswer {
    Answered(EpisodeAnswer),
    Rejected(String),
}

/// What sets one video service apart: the node that asks it, as its errors
/// name it; the variable its address is set in; the route that answers; and
/// the fields of each row it answers.
pub(super) struct VideoContract {
    node: &'static str,
    url_variable: &'static str,
    route: &'static str,
    row_fields: Fields,
}

impl VideoContract {
    /// A field nested within a row is best declared nullable: the service may
    /// then declare it either way, and its answer column keeps every nested
    /// field nullable regardless.
    pub(super) fn new(
        node: &'static str,
        url_variable: &'static str,
        route: &'static str,
        row_fields: Fields,
    ) -> Self {
        Self {
            node,
            url_variable,
            route,
            row_fields,
        }
    }

    /// The type of one episode's answer: a list of its rows.
    fn answer_type(&self) -> DataType {
        DataType::new_list(DataType::Struct(self.row_fields.clone()), true)
    }

    /// The rows the service answered, as one list answer. Each column must
    /// be its field's type, up to the nullability the field allows within
    /// it, and is cast to exactly that type.
    fn listing(&self, stream: &[u8]) -> Result<EpisodeAnswer, String> {
        let unreadable = |error: ArrowError| format!("Reading the Modal service's answer: {error}");
        let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(stream), None)
            .map_err(unreadable)?;
        let batches = reader.collect::<Result<Vec<_>, _>>().map_err(unreadable)?;
        let mut columns = Vec::with_capacity(self.row_fields.len());
        for field in self.row_fields.iter() {
            let parts = batches
                .iter()
                .map(|batch| {
                    batch
                        .column_by_name(field.name())
                        .filter(|column| field.data_type().contains(column.data_type()))
                        .ok_or_else(|| {
                            format!(
                                "The Modal service's answer has no {} of type {}.",
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
                let joined =
                    arrow_select::concat::concat(&parts).map_err(|error| error.to_string())?;
                arrow_cast::cast(&joined, field.data_type()).map_err(|error| error.to_string())?
            };
            if !field.is_nullable() && column.null_count() > 0 {
                return Err(format!("An answered row has no {}.", field.name()));
            }
            columns.push(column);
        }
        let rows = StructArray::try_new(self.row_fields.clone(), columns, None)
            .map_err(|error| error.to_string())?;
        Ok(EpisodeAnswer::listing(Arc::new(rows)))
    }
}

/// A video service as the provider keys reach it, sharing its node's permits
/// so no more than a few requests are in flight across the project's runs.
pub(super) struct VideoService {
    contract: VideoContract,
    client: reqwest::Client,
    endpoint: Url,
    token: String,
    permits: Arc<Semaphore>,
}

impl VideoService {
    /// `contract`'s service at `base_url`, reached with `token`, as the
    /// provider keys give them; a missing or malformed one is said plainly.
    pub(super) fn of(
        contract: VideoContract,
        base_url: Option<&str>,
        token: Option<&str>,
        permits: Arc<Semaphore>,
    ) -> Result<Self, String> {
        let VideoContract {
            node,
            url_variable,
            route,
            ..
        } = contract;
        let (Some(base_url), Some(token)) = (base_url, token) else {
            return Err(format!(
                "{node} needs the Modal service, which this Studio was started without: set {url_variable} and MODAL_SERVICE_TOKEN."
            ));
        };
        let invalid = |error: url::ParseError| format!("{url_variable} is invalid: {error}");
        let base = Url::parse(base_url).map_err(invalid)?;
        let local = matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base.scheme() != "https" && !(base.scheme() == "http" && local) {
            return Err(format!(
                "{url_variable} must use HTTPS (HTTP is allowed only for localhost)."
            ));
        }
        let endpoint = Url::parse(&format!("{}/{route}", base.as_str().trim_end_matches('/')))
            .map_err(invalid)?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            contract,
            client,
            endpoint,
            token: token.to_owned(),
            permits,
        })
    }

    /// A question to this service, its request carrying `arguments` in its
    /// schema metadata.
    pub(super) fn question(&self, arguments: Vec<(&'static str, String)>) -> VideoQuestion<'_> {
        VideoQuestion {
            service: self,
            arguments,
        }
    }

    /// Post `body` until the service answers it, and read what it answered.
    async fn answer(&self, body: Vec<u8>) -> Result<VideoAnswer, String> {
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
                .post(self.endpoint.clone())
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
                StatusCode::OK => return self.contract.listing(&bytes).map(VideoAnswer::Answered),
                StatusCode::UNPROCESSABLE_ENTITY => {
                    return Ok(VideoAnswer::Rejected(Self::rejection(&bytes)));
                }
                StatusCode::CONFLICT => {
                    return Err(format!(
                        "The Modal service can't answer with this model or revision: {}",
                        Self::rejection(&bytes)
                    ));
                }
                StatusCode::UNAUTHORIZED => {
                    return Err("The Modal service refused MODAL_SERVICE_TOKEN.".into());
                }
                status
                    if status == StatusCode::REQUEST_TIMEOUT
                        || status == StatusCode::TOO_MANY_REQUESTS
                        || status.is_server_error() =>
                {
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
            "The Modal service did not answer the episode after {ATTEMPTS} attempts: {last_failure}"
        ))
    }

    /// Why the service rejected a video: its `error`, or what it said.
    fn rejection(body: &[u8]) -> String {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
    }
}

/// One question to a video service: the arguments its request carries
/// beside the packets.
pub(super) struct VideoQuestion<'a> {
    service: &'a VideoService,
    arguments: Vec<(&'static str, String)>,
}

impl VideoQuestion<'_> {
    /// Ask about one episode's `packets` — `payload`, `decode_index`,
    /// `pts_ns`, in decode order.
    pub(super) async fn ask(&self, packets: &RecordBatch) -> Result<VideoAnswer, String> {
        let body = self.request_body(packets)?;
        self.service.answer(body).await
    }

    /// The type of the answer it asks for.
    pub(super) fn answer_type(&self) -> DataType {
        self.service.contract.answer_type()
    }

    /// The packets as the one record batch the service reads.
    fn request_body(&self, packets: &RecordBatch) -> Result<Vec<u8>, String> {
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
        let metadata = [("codec", "h264"), ("framing", "annex_b")]
            .into_iter()
            .chain(
                self.arguments
                    .iter()
                    .map(|(key, value)| (*key, value.as_str())),
            )
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect::<HashMap<_, _>>();
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
}

#[cfg(test)]
pub(super) mod tests;
