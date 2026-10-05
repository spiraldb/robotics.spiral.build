use super::*;
use arrow_array::{BinaryViewArray, Float32Array, Int64Array, ListArray, UInt32Array, UInt64Array};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode as Status};
use axum::response::{IntoResponse as _, Response};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A stand-in Modal service: it scores every packet it is sent as one
/// picture, refuses any token but `good`, rejects the checkpoint `reject`,
/// is unavailable once to the checkpoint `flaky`, runs the revision `rev`
/// only, and counts the requests it holds at once.
#[derive(Clone, Default)]
pub(in crate::curation) struct MockService {
    requests: Arc<AtomicUsize>,
    holding: Arc<AtomicUsize>,
    most_held: Arc<AtomicUsize>,
}

impl MockService {
    /// Serve on a free local port, and say where.
    pub(in crate::curation) async fn serve(self) -> String {
        let router = axum::Router::new()
            .route("/score_progress", axum::routing::post(Self::score))
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

    async fn score(State(service): State<Self>, headers: HeaderMap, body: Bytes) -> Response {
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
        let packets = &batches[0];
        let times = packets
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let count = packets.num_rows();
        let scores = RecordBatch::try_new(
            Arc::new(Schema::new(ProgressService::score_fields().to_vec())),
            vec![
                Arc::new(UInt32Array::from_iter_values(0..count as u32)),
                Arc::new(UInt64Array::from_iter_values(
                    times.values().iter().map(|time| *time as u64),
                )),
                Arc::new(Float32Array::from(vec![0.75; count])),
                Arc::new(Float32Array::from(vec![None::<f32>; count])),
                Arc::new(UInt32Array::from(vec![1; count])),
                Arc::new(UInt32Array::from(vec![4; count])),
            ],
        )
        .unwrap();
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(Vec::new(), &scores.schema()).unwrap();
        writer.write(&scores).unwrap();
        writer.finish().unwrap();
        writer.into_inner().unwrap().into_response()
    }
}

/// One episode's packets as the read of its camera answers them.
fn packets(count: i64) -> RecordBatch {
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

fn service(base_url: &str, token: &str) -> Result<ProgressService, String> {
    let keys = ProviderKeys::from_variables(|name| match name {
        "WARP_PROGRESS_URL" => Some(base_url.to_owned()),
        "WARP_PROGRESS_TOKEN" => Some(token.to_owned()),
        _ => None,
    });
    ProgressService::of(&keys, Arc::new(Semaphore::new(8)))
}

#[test]
fn the_service_needs_its_url_and_token() {
    let missing = ProgressService::of(&ProviderKeys::default(), Arc::new(Semaphore::new(1)));
    assert!(missing.err().unwrap().contains("WARP_PROGRESS_TOKEN"));
    let insecure = service("http://scores.example.com", "good");
    assert!(insecure.err().unwrap().contains("HTTPS"));
    assert!(service("https://scores.modal.run", "good").is_ok());
}

#[tokio::test]
async fn a_video_is_scored_rejected_or_refused_as_the_contract_says() {
    let base = MockService::default().serve().await;
    let scored = service(&base, "good")
        .unwrap()
        .score(&packets(3), "warp@6417be0", "rev")
        .await
        .unwrap();
    let ProgressScores::Scored(answer) = scored else {
        panic!("the video was scored")
    };
    assert_eq!(answer.value().data_type(), &ProgressService::answer_type());
    let list = answer.value().as_any().downcast_ref::<ListArray>().unwrap();
    let pictures = list.value(0);
    let pictures = pictures.as_any().downcast_ref::<StructArray>().unwrap();
    assert_eq!(pictures.len(), 3);
    let times = pictures
        .column_by_name("timestamp_ns")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(times.values().to_vec(), [0, 33_333_333, 66_666_666]);

    let rejected = service(&base, "good")
        .unwrap()
        .score(&packets(3), "reject", "rev")
        .await
        .unwrap();
    assert!(
        matches!(rejected, ProgressScores::Rejected(reason) if reason == "the video is not 30 Hz")
    );

    let refused = service(&base, "stale")
        .unwrap()
        .score(&packets(3), "warp@6417be0", "rev")
        .await;
    assert!(
        refused
            .err()
            .unwrap()
            .contains("refused WARP_PROGRESS_TOKEN")
    );
}

#[tokio::test]
async fn a_service_on_another_revision_fails_the_run_and_is_not_asked_again() {
    let mock = MockService::default();
    let base = mock.clone().serve().await;
    let scored = service(&base, "good")
        .unwrap()
        .score(&packets(2), "warp@6417be0", "stale")
        .await;
    assert!(scored.err().unwrap().contains("this service runs rev"));
    assert_eq!(mock.requests(), 1);
}

#[tokio::test]
async fn an_unavailable_service_is_asked_again_after_a_pause() {
    let mock = MockService::default();
    let base = mock.clone().serve().await;
    let scored = service(&base, "good")
        .unwrap()
        .score(&packets(2), "flaky", "rev")
        .await
        .unwrap();
    assert!(matches!(scored, ProgressScores::Scored(_)));
    assert_eq!(mock.requests(), 2);
}

#[tokio::test]
async fn no_more_than_the_permits_are_in_flight() {
    let mock = MockService::default();
    let base = mock.clone().serve().await;
    let scoring = Arc::new(service(&base, "good").unwrap());
    let scorings = (0..20).map(|_| {
        let scoring = scoring.clone();
        tokio::spawn(async move { scoring.score(&packets(1), "warp", "rev").await })
    });
    for scored in futures_util::future::join_all(scorings).await {
        assert!(matches!(scored.unwrap(), Ok(ProgressScores::Scored(_))));
    }
    assert_eq!(mock.requests(), 20);
    assert!(mock.most_held.load(Ordering::SeqCst) <= 8);
}
