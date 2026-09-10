use super::*;
use crate::DatabaseSession as _;
use arrow_array::{Array as _, ArrayRef, BinaryArray, Int64Array, StructArray, UInt32Array};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use futures::TryStreamExt as _;
use std::sync::Arc;

#[tokio::test]
async fn workflow_sql_nodes_plan_and_execute_an_arrow_join() {
    let store = ProjectStore::workflow_test();
    let source =
        WorkflowSqlNode::new("SELECT CAST(3 AS BIGINT) AS value, 'retain me' AS label").unwrap();
    let enrichment =
        WorkflowSqlNode::new("SELECT *, value * 2 AS score FROM {{node:source}}").unwrap();
    let joined = WorkflowSqlNode::new("SELECT rows.label, rows.score FROM {{node:enriched}} rows JOIN {{node:threshold}} threshold ON rows.score > threshold.minimum").unwrap();
    let threshold = WorkflowSqlNode::new("SELECT CAST(5 AS BIGINT) AS minimum").unwrap();

    let source_schema = source.plan(&store, BTreeMap::new()).await.unwrap();
    let enriched_schema = enrichment
        .plan(&store, BTreeMap::from([("source".into(), source_schema)]))
        .await
        .unwrap();
    assert_eq!(
        enriched_schema.fields().len(),
        3,
        "an operator can preserve unrelated columns"
    );
    let threshold_schema = threshold.plan(&store, BTreeMap::new()).await.unwrap();
    let planned = joined
        .plan(
            &store,
            BTreeMap::from([
                ("enriched".into(), enriched_schema),
                ("threshold".into(), threshold_schema),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(planned.field(1).name(), "score");

    let source_stream = source.execute(&store, BTreeMap::new()).await.unwrap();
    let enriched_stream = enrichment
        .execute(&store, BTreeMap::from([("source".into(), source_stream)]))
        .await
        .unwrap();
    let threshold_stream = threshold.execute(&store, BTreeMap::new()).await.unwrap();
    let result = joined
        .execute(
            &store,
            BTreeMap::from([
                ("enriched".into(), enriched_stream),
                ("threshold".into(), threshold_stream),
            ]),
        )
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(
        result[0]
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0),
        6
    );
    assert!(
        store
            .connect()
            .await
            .unwrap()
            .relations()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workflow_sql_decodes_packet_frames_through_the_native_driver() {
    // Share the playback fixture: eight access units, with a keyframe every four.
    let bytes = include_bytes!("../../../../frontend/src/activities/data/fixtures/seek.h264");
    let mut offsets = bytes
        .windows(5)
        .enumerate()
        .filter_map(|(index, prefix)| (prefix == [0, 0, 0, 1, 9]).then_some(index))
        .collect::<Vec<_>>();
    offsets.push(bytes.len());
    let payloads = offsets
        .windows(2)
        .map(|range| &bytes[range[0]..range[1]])
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), 8);
    let packets = RecordBatch::try_from_iter([
        ("payload", Arc::new(BinaryArray::from(payloads)) as ArrayRef),
        (
            "decode_index",
            Arc::new(Int64Array::from_iter_values(0..8)) as ArrayRef,
        ),
    ])
    .unwrap();
    let store = ProjectStore::workflow_test();
    let node = WorkflowSqlNode::new(
        "SELECT f.request_index, spql.video.decode(f.frame) AS image
         FROM (SELECT spql.video.from_packets(
             spql.list.collect(struct_pack(payload := payload, decode_index := decode_index)),
             'h264', 'annex_b') AS source FROM {{node:packets}}) s
         CROSS JOIN LATERAL spql.video.frames(s.source, spql.video.frame_indices([6, 2, 6])) f
         ORDER BY f.request_index",
    )
    .unwrap();
    let planned = node
        .plan(
            &store,
            BTreeMap::from([("packets".into(), packets.schema())]),
        )
        .await
        .unwrap();
    assert_eq!(
        planned
            .field_with_name("image")
            .unwrap()
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .map(String::as_str),
        Some("spql.image.Image")
    );
    let stream = RecordBatchStream::new(
        packets.schema(),
        futures::stream::iter([Ok(packets)]).boxed(),
    );
    let batches = node
        .execute(&store, BTreeMap::from([("packets".into(), stream)]))
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);
    for batch in &batches {
        let images = batch
            .column_by_name("image")
            .unwrap()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        assert_eq!(images.null_count(), 0);
        let widths = images
            .column_by_name("width")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt32Array>()
            .unwrap();
        assert!(widths.iter().all(|width| width == Some(32)));
    }
}

#[tokio::test]
async fn workflow_preview_drains_and_reports_errors_beyond_its_row_limit() {
    let batch =
        RecordBatch::try_from_iter([("value", Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef)])
            .unwrap();
    let stream = RecordBatchStream::new(
        batch.schema(),
        futures::stream::iter([
            Ok(batch),
            Err(crate::DatabaseError::new(
                crate::DatabaseErrorKind::Internal,
                "late failure",
            )),
        ])
        .boxed(),
    );
    assert!(
        stream
            .into_preview_drained(1)
            .await
            .unwrap_err()
            .contains("late failure")
    );
}
