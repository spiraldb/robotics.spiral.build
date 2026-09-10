use super::*;
use arrow_array::{Int64Array, new_null_array};
use std::collections::HashMap;

struct ImageFixture;
impl ImageFixture {
    fn null_batch() -> RecordBatch {
        let image_type = DataType::Struct(
            vec![
                Field::new("width", DataType::UInt32, false),
                Field::new("height", DataType::UInt32, false),
                Field::new("pixel_format", DataType::UInt8, false),
                Field::new(
                    "data",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))),
                    false,
                ),
            ]
            .into(),
        );
        let field =
            Field::new("camera", image_type.clone(), true).with_metadata(HashMap::from([(
                EXTENSION_TYPE_NAME_KEY.into(),
                IMAGE_EXTENSION_NAME.into(),
            )]));
        let schema = Arc::new(Schema::new(vec![
            Field::new("arbitrary_id", DataType::Int64, false),
            field,
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![7, 7, 2])),
                new_null_array(&image_type, 3),
            ],
        )
        .unwrap()
    }
    fn operator() -> ModelOperator {
        ModelOperator::new(
            "camera".into(),
            vec![WorkflowModelFeature::new("rainy", "Rain match", "rain")],
        )
    }
}

#[tokio::test]
async fn null_images_preserve_arbitrary_columns_duplicates_order_and_image_metadata() {
    let batch = ImageFixture::null_batch();
    let input = RecordBatchStream::new(
        batch.schema(),
        futures::stream::iter([Ok(batch.clone())]).boxed(),
    );
    let mut output = ImageFixture::operator()
        .execute(input, CancellationToken::new())
        .unwrap();
    assert_eq!(output.schema().field(1), batch.schema().field(1));
    let scored = output.next().await.unwrap().unwrap();
    assert_eq!(scored.column(0), batch.column(0));
    assert_eq!(scored.column(1), batch.column(1));
    assert_eq!(scored.column(2).null_count(), 3);
    assert!(output.next().await.is_none());
}

#[test]
fn schema_errors_precede_model_loading() {
    let batch = ImageFixture::null_batch();
    let mut operator = ImageFixture::operator();
    operator.image_column = "arbitrary_id".into();
    assert!(
        operator
            .plan(&batch.schema())
            .unwrap_err()
            .contains("decoded Spiral image")
    );
    operator.image_column = "camera".into();
    operator.features[0].set_name("arbitrary_id");
    assert!(
        operator
            .plan(&batch.schema())
            .unwrap_err()
            .contains("already exists")
    );
    assert!(operator.engine.is_none());
}

#[tokio::test]
async fn cancellation_before_model_input_does_not_poll_the_parent() {
    let batch = ImageFixture::null_batch();
    let input = RecordBatchStream::new(
        batch.schema(),
        futures::stream::poll_fn(|_| panic!("cancelled model polled its parent")).boxed(),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut output = ImageFixture::operator().execute(input, cancel).unwrap();
    assert!(output.next().await.unwrap().is_err());
}

use arrow_schema::Fields;

#[test]
fn malformed_pixels_fail_without_panicking() {
    let data = Arc::new(ListArray::from_iter_primitive::<
        arrow_array::types::UInt8Type,
        _,
        _,
    >([Some(vec![Some(255), Some(0)])])) as ArrayRef;
    let images = StructArray::new(
        Fields::from(vec![
            Field::new("data", data.data_type().clone(), false),
            Field::new("width", DataType::UInt32, false),
            Field::new("height", DataType::UInt32, false),
            Field::new("pixel_format", DataType::UInt8, false),
        ]),
        vec![
            data,
            Arc::new(UInt32Array::from(vec![1])) as ArrayRef,
            Arc::new(UInt32Array::from(vec![1])) as ArrayRef,
            Arc::new(UInt8Array::from(vec![1])) as ArrayRef,
        ],
        None,
    );
    let processor = SiglipImageProcessor {
        image_size: 224,
        image_mean: [0.5; 3],
        image_std: [0.5; 3],
        rescale_factor: 1.0 / 255.0,
    };
    assert!(images.decode_image_row(0, &processor).is_err());
}

#[test]
fn pinned_processor_contract_requires_bicubic_and_exact_normalization() {
    let source: SiglipPreprocessorFile = serde_json::from_value(serde_json::json!({
        "do_normalize": true,
        "do_rescale": true,
        "do_resize": true,
        "image_mean": [0.5, 0.5, 0.5],
        "image_std": [0.5, 0.5, 0.5],
        "resample": 3,
        "rescale_factor": 0.00392156862745098,
        "size": {"height": 224, "width": 224}
    }))
    .unwrap();
    let processor = SiglipImageProcessor::from_source(source, 224).unwrap();
    assert_eq!(processor.image_size, 224);
    assert!((processor.rescale_factor - 1.0 / 255.0).abs() < f32::EPSILON);

    let unsupported: SiglipPreprocessorFile = serde_json::from_value(serde_json::json!({
        "do_normalize": true,
        "do_rescale": true,
        "do_resize": true,
        "image_mean": [0.5, 0.5, 0.5],
        "image_std": [0.5, 0.5, 0.5],
        "resample": 2,
        "rescale_factor": 0.00392156862745098,
        "size": {"height": 224, "width": 224}
    }))
    .unwrap();
    assert!(SiglipImageProcessor::from_source(unsupported, 224).is_err());
}

#[test]
fn chunk_rows_are_derived_from_budget_and_model_dimensions() {
    let bytes_per_image = 224 * 224 * 3 * std::mem::size_of::<f32>();
    assert_eq!(
        SiglipImageProcessor::chunk_rows_for_budget(224, bytes_per_image * 7),
        7
    );
    assert_eq!(
        SiglipImageProcessor::chunk_rows_for_budget(224, 1),
        MIN_MODEL_CHUNK_ROWS
    );
    assert_eq!(
        SiglipImageProcessor::chunk_rows_for_budget(224, usize::MAX),
        MAX_MODEL_CHUNK_ROWS
    );
}
