use super::*;
use arrow_array::{BinaryViewArray, Int64Array, ListArray, UInt64Array};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode as Status};
use axum::response::{IntoResponse as _, Response};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A stand-in Modal video service on `route`: it answers what `answer` makes
/// of the packets it is sent, refuses any token but `good`, rejects the
/// checkpoint `reject`, is unavailable once to the checkpoint `flaky`, runs
/// the revision `rev` only, and keeps the metadata it was last sent and how
/// many requests it holds at once.
#[derive(Clone)]
pub(in crate::curation) struct MockService {
    route: &'static str,
    answer: fn(&RecordBatch) -> RecordBatch,
    requests: Arc<AtomicUsize>,
    holding: Arc<AtomicUsize>,
    most_held: Arc<AtomicUsize>,
    metadata: Arc<Mutex<HashMap<String, String>>>,
}

impl MockService {
    pub(in crate::curation) fn new(
        route: &'static str,
        answer: fn(&RecordBatch) -> RecordBatch,
    ) -> Self {
        Self {
            route,
            answer,
            requests: Arc::default(),
            holding: Arc::default(),
            most_held: Arc::default(),
            metadata: Arc::default(),
        }
    }

    /// Serve on a free local port, and say where.
    pub(in crate::curation) async fn serve(self) -> String {
        let router = axum::Router::new()
            .route(self.route, axum::routing::post(Self::respond))
            .with_state(self);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/")
    }

    /// How many requests it has answered.
    pub(in crate::curation) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// The schema metadata of the last request it was sent.
    pub(in crate::curation) fn metadata(&self) -> HashMap<String, String> {
        self.metadata.lock().unwrap().clone()
    }

    /// One episode's packets as the read of its camera answers them.
    pub(in crate::curation) fn packets(count: i64) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("payload", DataType::BinaryView, false),
            Field::new("decode_index", DataType::Int64, false),
            Field::new("pts_ns", DataType::Int64, false),
        ]));
        let payloads = (0..count)
            .map(|index| vec![0, 0, 0, 1, index as u8])
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(BinaryViewArray::from_iter_values(
                    payloads.iter().map(Vec::as_slice),
                )),
                Arc::new(Int64Array::from_iter_values(0..count)),
                Arc::new(Int64Array::from_iter_values(
                    (0..count).map(|index| index * 33_333_333),
                )),
            ],
        )
        .unwrap()
    }

    /// The packets' times, as unsigned nanoseconds.
    pub(in crate::curation) fn times(packets: &RecordBatch) -> UInt64Array {
        let times = packets
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        UInt64Array::from_iter_values(times.values().iter().map(|time| *time as u64))
    }

    /// Answer `rows` as the Arrow IPC stream a service answers.
    pub(in crate::curation) fn stream(rows: &RecordBatch) -> Response {
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(Vec::new(), &rows.schema()).unwrap();
        writer.write(rows).unwrap();
        writer.finish().unwrap();
        writer.into_inner().unwrap().into_response()
    }

    async fn respond(State(service): State<Self>, headers: HeaderMap, body: Bytes) -> Response {
        let attempt = service.requests.fetch_add(1, Ordering::SeqCst);
        let held = service.holding.fetch_add(1, Ordering::SeqCst) + 1;
        service.most_held.fetch_max(held, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        service.holding.fetch_sub(1, Ordering::SeqCst);
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer good") {
            return Status::UNAUTHORIZED.into_response();
        }
        assert_eq!(headers.get("content-type").unwrap(), ARROW_STREAM);
        let reader =
            arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(body), None).unwrap();
        let schema = reader.schema();
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches.len(), 1, "one record batch");
        let metadata = schema.metadata();
        *service.metadata.lock().unwrap() = metadata.clone();
        assert_eq!(
            (metadata["codec"].as_str(), metadata["framing"].as_str()),
            ("h264", "annex_b")
        );
        let types = schema
            .fields()
            .iter()
            .map(|field| field.data_type().clone())
            .collect::<Vec<_>>();
        assert_eq!(types, [DataType::Binary, DataType::Int64, DataType::Int64]);
        let order = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(
            order.values().to_vec(),
            (0..order.len() as i64).collect::<Vec<_>>()
        );
        if metadata["implementation"] != "rev" {
            return (
                Status::CONFLICT,
                axum::Json(serde_json::json!({"error": "this service runs rev"})),
            )
                .into_response();
        }
        match metadata["model"].as_str() {
            "reject" => {
                return (
                    Status::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({"error": "the video is not 30 Hz"})),
                )
                    .into_response();
            }
            "flaky" if attempt == 0 => return Status::SERVICE_UNAVAILABLE.into_response(),
            _ => {}
        }
        Self::stream(&(service.answer)(&batches[0]))
    }

    /// A probe service's answer: each packet's time, as one row.
    fn timed(packets: &RecordBatch) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(VideoContract::probe().row_fields.to_vec())),
            vec![Arc::new(Self::times(packets))],
        )
        .unwrap()
    }

    /// A probe service's answer with its times typed as signed.
    fn mistimed(packets: &RecordBatch) -> RecordBatch {
        let times = arrow_cast::cast(&Self::times(packets), &DataType::Int64).unwrap();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "timestamp_ns",
                DataType::Int64,
                false,
            )])),
            vec![times],
        )
        .unwrap()
    }
}

impl VideoContract {
    /// A service answering each packet's time on `/probe`.
    fn probe() -> Self {
        Self::new(
            "Probe",
            "PROBE_URL",
            "probe",
            Fields::from(vec![Field::new("timestamp_ns", DataType::UInt64, false)]),
        )
    }
}

fn service(base_url: &str, token: &str) -> Result<VideoService, String> {
    VideoService::of(
        VideoContract::probe(),
        Some(base_url),
        Some(token),
        Arc::new(Semaphore::new(8)),
    )
}

async fn ask(service: &VideoService, model: &str, revision: &str) -> Result<VideoAnswer, String> {
    service
        .question(vec![("model", model), ("implementation", revision)])
        .ask(&MockService::packets(3))
        .await
}

#[test]
fn the_service_needs_its_url_and_token_and_https_away_from_localhost() {
    let permits = || Arc::new(Semaphore::new(1));
    let missing = VideoService::of(VideoContract::probe(), None, Some("good"), permits());
    assert_eq!(
        missing.err().unwrap(),
        "Probe needs the Modal service, which this Studio was started without: set PROBE_URL and MODAL_SERVICE_TOKEN."
    );
    let tokenless = VideoService::of(
        VideoContract::probe(),
        Some("https://answers.modal.run"),
        None,
        permits(),
    );
    assert!(tokenless.err().unwrap().contains("MODAL_SERVICE_TOKEN"));
    let insecure = service("http://answers.example.com", "good");
    assert!(insecure.err().unwrap().contains("PROBE_URL must use HTTPS"));
    let endpoint = service("https://answers.modal.run/", "good")
        .unwrap()
        .endpoint;
    assert_eq!(endpoint.as_str(), "https://answers.modal.run/probe");
    assert!(service("http://localhost:8000", "good").is_ok());
}

#[tokio::test]
async fn a_video_is_answered_rejected_or_refused_as_the_contract_says() {
    let base = MockService::new("/probe", MockService::timed).serve().await;
    let VideoAnswer::Answered(answer) = ask(&service(&base, "good").unwrap(), "m", "rev")
        .await
        .unwrap()
    else {
        panic!("the video was answered")
    };
    assert_eq!(
        answer.value().data_type(),
        &VideoContract::probe().answer_type()
    );
    let list = answer.value().as_any().downcast_ref::<ListArray>().unwrap();
    assert_eq!(list.len(), 1, "one episode's answer");
    let rows = list.value(0);
    let rows = rows.as_any().downcast_ref::<StructArray>().unwrap();
    let times = rows
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(times.values().to_vec(), [0, 33_333_333, 66_666_666]);

    let rejected = ask(&service(&base, "good").unwrap(), "reject", "rev").await;
    assert!(
        matches!(rejected, Ok(VideoAnswer::Rejected(reason)) if reason == "the video is not 30 Hz")
    );

    let refused = ask(&service(&base, "stale").unwrap(), "m", "rev").await;
    assert!(
        refused
            .err()
            .unwrap()
            .contains("refused MODAL_SERVICE_TOKEN")
    );
}

#[tokio::test]
async fn an_answer_of_another_type_fails_the_run() {
    let base = MockService::new("/probe", MockService::mistimed)
        .serve()
        .await;
    let answered = ask(&service(&base, "good").unwrap(), "m", "rev").await;
    assert_eq!(
        answered.err().unwrap(),
        "The Modal service's answer has no timestamp_ns of type UInt64."
    );
}

#[tokio::test]
async fn a_service_on_another_revision_fails_the_run_and_is_not_asked_again() {
    let mock = MockService::new("/probe", MockService::timed);
    let base = mock.clone().serve().await;
    let answered = ask(&service(&base, "good").unwrap(), "m", "stale").await;
    assert!(answered.err().unwrap().contains("this service runs rev"));
    assert_eq!(mock.requests(), 1);
}

#[tokio::test]
async fn an_unavailable_service_is_asked_again_after_a_pause() {
    let mock = MockService::new("/probe", MockService::timed);
    let base = mock.clone().serve().await;
    let answered = ask(&service(&base, "good").unwrap(), "flaky", "rev").await;
    assert!(matches!(answered, Ok(VideoAnswer::Answered(_))));
    assert_eq!(mock.requests(), 2);
}

#[tokio::test]
async fn no_more_than_the_permits_are_in_flight() {
    let mock = MockService::new("/probe", MockService::timed);
    let base = mock.clone().serve().await;
    let service = Arc::new(service(&base, "good").unwrap());
    let asked = (0..20).map(|_| {
        let service = service.clone();
        tokio::spawn(async move { ask(&service, "m", "rev").await })
    });
    for answered in futures_util::future::join_all(asked).await {
        assert!(matches!(answered.unwrap(), Ok(VideoAnswer::Answered(_))));
    }
    assert_eq!(mock.requests(), 20);
    assert!(mock.most_held.load(Ordering::SeqCst) <= 8);
}
