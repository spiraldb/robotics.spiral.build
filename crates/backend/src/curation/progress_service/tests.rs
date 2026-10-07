use super::*;
use crate::curation::video_service::VideoAnswer;
use crate::curation::video_service::tests::MockService;
use arrow_array::{Float32Array, ListArray, RecordBatch, StructArray, UInt32Array, UInt64Array};
use arrow_schema::Schema;

impl MockService {
    /// The progress service: every packet it is sent scored as one picture.
    pub(in crate::curation) fn scoring() -> Self {
        Self::new("/score_progress", Self::scores)
    }

    fn scores(packets: &RecordBatch) -> RecordBatch {
        let count = packets.num_rows();
        RecordBatch::try_new(
            Arc::new(Schema::new(ProgressService::score_fields().to_vec())),
            vec![
                Arc::new(UInt32Array::from_iter_values(0..count as u32)),
                Arc::new(Self::times(packets)),
                Arc::new(Float32Array::from(vec![0.75; count])),
                Arc::new(Float32Array::from(vec![None::<f32>; count])),
                Arc::new(UInt32Array::from(vec![1; count])),
                Arc::new(UInt32Array::from(vec![4; count])),
            ],
        )
        .unwrap()
    }
}

fn keys(base_url: &str) -> ProviderKeys {
    ProviderKeys::from_variables(|name| match name {
        "WARP_PROGRESS_URL" => Some(base_url.to_owned()),
        "MODAL_SERVICE_TOKEN" => Some("good".to_owned()),
        _ => None,
    })
}

#[test]
fn the_service_is_where_warp_progress_url_says() {
    let missing = ProgressService::of(&ProviderKeys::default(), Arc::new(Semaphore::new(1)));
    assert!(
        missing
            .err()
            .unwrap()
            .starts_with("Score progress needs the Modal service")
    );
    let insecure = ProgressService::of(
        &keys("http://scores.example.com"),
        Arc::new(Semaphore::new(1)),
    );
    assert!(insecure.err().unwrap().contains("WARP_PROGRESS_URL"));
}

#[tokio::test]
async fn a_video_is_scored_with_its_checkpoint_on_its_revision_as_a_score_per_picture() {
    let mock = MockService::scoring();
    let base = mock.clone().serve().await;
    let service = ProgressService::of(&keys(&base), Arc::new(Semaphore::new(1))).unwrap();
    let scoring = service.scoring("warp@6417be0", "rev");
    let Ok(VideoAnswer::Answered(answer)) = scoring.ask(&MockService::packets(3)).await else {
        panic!("the video was scored")
    };
    let metadata = mock.metadata();
    assert_eq!(
        (
            metadata["model"].as_str(),
            metadata["implementation"].as_str()
        ),
        ("warp@6417be0", "rev")
    );
    let answer_type = DataType::new_list(DataType::Struct(ProgressService::score_fields()), true);
    assert_eq!(answer.value().data_type(), &answer_type);
    assert_eq!(scoring.answer_type(), answer_type);
    let list = answer.value().as_any().downcast_ref::<ListArray>().unwrap();
    let pictures = list.value(0);
    let pictures = pictures.as_any().downcast_ref::<StructArray>().unwrap();
    let times = pictures
        .column_by_name("timestamp_ns")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap();
    assert_eq!(times.values().to_vec(), [0, 33_333_333, 66_666_666]);
    assert_eq!(pictures.column_by_name("progress").unwrap().null_count(), 3);
}
