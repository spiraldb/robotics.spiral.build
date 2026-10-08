use super::*;
use crate::curation::video_service::VideoAnswer;
use crate::curation::video_service::tests::MockService;
use arrow_array::builder::{Float32Builder, ListBuilder, StructBuilder, UInt32Builder};
use arrow_array::{Array, Float32Array, ListArray, RecordBatch, StructArray, UInt32Array};
use arrow_schema::Schema;

impl MockService {
    /// The object tracking service: every packet it is sent one sampled
    /// picture, tracking object 7 in every other one. As a service may, it
    /// declares an object and its fields non-nullable.
    pub(in crate::curation) fn tracking() -> Self {
        Self::new("/track_objects", Self::tracks)
    }

    fn tracks(packets: &RecordBatch) -> RecordBatch {
        let object = Fields::from(vec![
            Field::new("id", DataType::UInt32, false),
            Field::new("score", DataType::Float32, false),
            Field::new("x", DataType::Float32, false),
            Field::new("y", DataType::Float32, false),
            Field::new("width", DataType::Float32, false),
            Field::new("height", DataType::Float32, false),
        ]);
        let item = Arc::new(Field::new_list_field(
            DataType::Struct(object.clone()),
            false,
        ));
        let mut objects =
            ListBuilder::new(StructBuilder::from_fields(object, 0)).with_field(item.clone());
        for picture in 0..packets.num_rows() {
            if picture % 2 == 0 {
                let tracked = objects.values();
                tracked
                    .field_builder::<UInt32Builder>(0)
                    .unwrap()
                    .append_value(7);
                for (position, value) in [0.9, 0.25, 0.5, 0.125, 0.25].into_iter().enumerate() {
                    tracked
                        .field_builder::<Float32Builder>(position + 1)
                        .unwrap()
                        .append_value(value);
                }
                tracked.append(true);
            }
            objects.append(true);
        }
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("timestamp_ns", DataType::UInt64, false),
                Field::new("objects", DataType::List(item), false),
            ])),
            vec![Arc::new(Self::times(packets)), Arc::new(objects.finish())],
        )
        .unwrap()
    }
}

fn keys(base_url: &str) -> ProviderKeys {
    ProviderKeys::from_variables(|name| match name {
        "OBJECT_TRACKING_URL" => Some(base_url.to_owned()),
        "MODAL_SERVICE_TOKEN" => Some("good".to_owned()),
        _ => None,
    })
}

#[test]
fn the_service_is_where_object_tracking_url_says() {
    let progress_only = ProviderKeys::from_variables(|name| match name {
        "WARP_PROGRESS_URL" => Some("https://scores.modal.run".to_owned()),
        "MODAL_SERVICE_TOKEN" => Some("good".to_owned()),
        _ => None,
    });
    let missing = ObjectTrackingService::of(&progress_only, Arc::new(Semaphore::new(1)));
    assert_eq!(
        missing.err().unwrap(),
        "Track objects needs the Modal service, which this Studio was started without: set OBJECT_TRACKING_URL and MODAL_SERVICE_TOKEN."
    );
    assert!(
        ObjectTrackingService::of(
            &keys("https://tracks.modal.run"),
            Arc::new(Semaphore::new(1))
        )
        .is_ok()
    );
}

#[tokio::test]
async fn the_objects_a_prompt_names_are_tracked_into_one_listing_of_pictures() {
    let mock = MockService::tracking();
    let base = mock.clone().serve().await;
    let service = ObjectTrackingService::of(&keys(&base), Arc::new(Semaphore::new(1))).unwrap();
    let tracking = service.tracking(("the red cup", 2), ("sam3@5f1c2d0", "rev"));
    let Ok(VideoAnswer::Answered(answer)) = tracking.ask(&MockService::packets(3)).await else {
        panic!("the video was tracked")
    };
    let metadata = mock.metadata();
    assert_eq!(
        ["prompt", "rate", "model", "implementation"].map(|key| metadata[key].as_str()),
        ["the red cup", "2", "sam3@5f1c2d0", "rev"]
    );

    // The answer is the contract's type, whatever the service declared
    // nullable within a row.
    let object = Fields::from(vec![
        Field::new("id", DataType::UInt32, true),
        Field::new("score", DataType::Float32, true),
        Field::new("x", DataType::Float32, true),
        Field::new("y", DataType::Float32, true),
        Field::new("width", DataType::Float32, true),
        Field::new("height", DataType::Float32, true),
    ]);
    let picture = Fields::from(vec![
        Field::new("timestamp_ns", DataType::UInt64, false),
        Field::new(
            "objects",
            DataType::new_list(DataType::Struct(object), true),
            false,
        ),
    ]);
    let answer_type = DataType::new_list(DataType::Struct(picture), true);
    assert_eq!(answer.value().data_type(), &answer_type);
    assert_eq!(tracking.answer_type(), answer_type);

    let list = answer.value().as_any().downcast_ref::<ListArray>().unwrap();
    let pictures = list.value(0);
    let pictures = pictures.as_any().downcast_ref::<StructArray>().unwrap();
    let objects = pictures
        .column_by_name("objects")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    assert_eq!(
        (0..pictures.len())
            .map(|picture| objects.value_length(picture))
            .collect::<Vec<_>>(),
        [1, 0, 1]
    );
    let first = objects.value(0);
    let first = first.as_any().downcast_ref::<StructArray>().unwrap();
    let id = first.column_by_name("id").unwrap();
    let x = first.column_by_name("x").unwrap();
    assert_eq!(
        id.as_any().downcast_ref::<UInt32Array>().unwrap().value(0),
        7
    );
    assert_eq!(
        x.as_any().downcast_ref::<Float32Array>().unwrap().value(0),
        0.25
    );
}
