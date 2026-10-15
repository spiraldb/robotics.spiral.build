use super::*;
use crate::curation::openrouter::tests::MockOpenRouter;
use arrow_array::types::Float64Type;
use arrow_array::{ListArray, UInt64Array};
use arrow_buffer::OffsetBuffer;
use arrow_schema::Schema;
use axum::response::IntoResponse as _;
use serde_json::json;
use spiral_robotics_common::curation::CategoryDefinition;

fn episode() -> EpisodeId {
    EpisodeId::new(0)
}

/// Three subtasks a Break down stored, the second without a description.
fn stored() -> StoredSubtasks {
    StoredSubtasks(Some(vec![
        Some("make the first fold".to_owned()),
        None,
        Some("grasp the corners".to_owned()),
    ]))
}

/// What reading a stored Break down column answers for one episode: its
/// subtasks described as `descriptions`, or NULL.
fn read(descriptions: Option<&[&str]>) -> RecordBatch {
    let fields = Fields::from(vec![
        Field::new("start_ns", DataType::UInt64, true),
        Field::new("end_ns", DataType::UInt64, true),
        Field::new("description", DataType::Utf8View, true),
    ]);
    let listed = descriptions.unwrap_or_default();
    let count = listed.len() as u64;
    let rows = StructArray::try_new(
        fields.clone(),
        vec![
            Arc::new(UInt64Array::from_iter_values(0..count)),
            Arc::new(UInt64Array::from_iter_values(1..=count)),
            Arc::new(arrow_array::StringViewArray::from(listed.to_vec())),
        ],
        None,
    )
    .unwrap();
    let item = Arc::new(Field::new_list_field(DataType::Struct(fields), true));
    let lists = ListArray::new(
        item,
        OffsetBuffer::from_lengths([listed.len()]),
        Arc::new(rows),
        Some(NullBuffer::from(vec![descriptions.is_some()])),
    );
    let schema = Schema::new(vec![Field::new(
        "subtasks",
        lists.data_type().clone(),
        true,
    )]);
    RecordBatch::try_new(Arc::new(schema), vec![Arc::new(lists)]).unwrap()
}

#[test]
fn a_judgment_reads_the_descriptions_a_break_down_stored_in_their_order() {
    let dataset = DatasetSelection::new("folding").unwrap();
    let column = AnswerColumn::of("break_down", &["m"]).unwrap();
    assert_eq!(
        StoredSubtasks::statement(&dataset, &column),
        format!(
            "SELECT a.value AS subtasks FROM \"datasets.folding.answers.{column}\" a WHERE a.episode_id = $1"
        )
    );
    let StoredSubtasks(read_back) =
        StoredSubtasks::read(&[read(Some(&["reach", "grasp"]))]).unwrap();
    assert_eq!(
        read_back,
        Some(vec![Some("reach".into()), Some("grasp".into())])
    );
    for none in [read(None), read(Some(&[]))] {
        assert_eq!(StoredSubtasks::read(&[none]).unwrap().0, None);
    }
    assert_eq!(StoredSubtasks::read(&[]).unwrap().0, None);
}

#[tokio::test]
async fn similarities_and_categories_answer_each_episode_from_one_request_each() {
    let mock = MockOpenRouter::new(|_, body, _| {
        if body["state"]["query"].is_string() {
            MockOpenRouter::decided(json!({
                "r0": {"type": "noul", "noul": 0.9},
                "r2": {"type": "noul", "noul": 0.1},
            }))
        } else {
            MockOpenRouter::decided(json!({
                "r0": {"type": "choice", "choice": "fold",
                       "probabilities": {"fold": 0.8, "grasp": 0.2}},
                "r2": {"type": "choice", "choice": "grasp",
                       "probabilities": {"fold": 0.3, "grasp": 0.7}},
            }))
        }
    });
    let openrouter = mock.serve().await;
    let similar = SimilarityQuestions::new("make the first fold").unwrap();
    let (answer, failure) = SubtaskJudgment::new(similar, "typesafe/jev-1.13", &openrouter)
        .answer(&episode(), stored())
        .await
        .unwrap()
        .into_parts();
    assert_eq!(failure, None);
    let list = answer.value().as_list::<i32>();
    assert_eq!(
        list.data_type(),
        &DataType::new_list(DataType::Float64, true)
    );
    let similarities = list.value(0);
    let similarities = similarities.as_primitive::<Float64Type>();
    assert_eq!(
        similarities.iter().collect::<Vec<_>>(),
        [Some(0.9), None, Some(0.1)]
    );
    let (route, request) = &mock.requests()[0];
    assert_eq!(route, "/alpha/decisions");
    assert_eq!(request["model"], "typesafe/jev-1.13");
    assert_eq!(request["state"]["query"], "make the first fold");
    assert_eq!(request["state"]["candidates"]["r2"], "grasp the corners");
    assert!(
        request["questions"].get("r1").is_none(),
        "no description, no question"
    );

    let categories = [
        CategoryDefinition::new("fold", "Folding the cloth."),
        CategoryDefinition::new("grasp", "Taking hold of the cloth."),
    ];
    let grouped = CategoryQuestions::new(&categories).unwrap();
    let (answer, _) = SubtaskJudgment::new(grouped, "typesafe/jev-1.13", &openrouter)
        .answer(&episode(), stored())
        .await
        .unwrap()
        .into_parts();
    let list = answer.value().as_list::<i32>();
    let chosen = list.value(0);
    let chosen = chosen.as_struct();
    assert_eq!(chosen.fields(), &CategoryQuestions::fields());
    assert!(chosen.is_valid(0) && chosen.is_null(1) && chosen.is_valid(2));
    let names = chosen.column(0).as_string::<i32>();
    let probabilities = chosen.column(1).as_primitive::<Float64Type>();
    assert_eq!((names.value(0), probabilities.value(0)), ("fold", 0.8));
    assert_eq!((names.value(2), probabilities.value(2)), ("grasp", 0.7));
    let (_, request) = &mock.requests()[1];
    assert_eq!(request["questions"]["r2"]["type"], "choice");
    assert_eq!(
        request["questions"]["r2"]["criteria"]["grasp"],
        "Taking hold of the cloth."
    );
    assert_eq!(mock.requests().len(), 2, "one request each");
}

#[tokio::test]
async fn a_subtask_jev_cannot_answer_is_null_and_a_refused_key_ends_the_run() {
    let unsure = MockOpenRouter::new(|_, _, _| {
        MockOpenRouter::decided(json!({
            "r0": {"type": "noul", "noul": 0.9},
            "r2": {"type": "noul", "noul": 1.7},
        }))
    });
    let openrouter = unsure.serve().await;
    let judgment = SubtaskJudgment::new(
        SimilarityQuestions::new("fold").unwrap(),
        "typesafe/jev-1.13",
        &openrouter,
    );
    let (answer, failure) = judgment
        .answer(&episode(), stored())
        .await
        .unwrap()
        .into_parts();
    assert_eq!(failure, None, "the episode is answered");
    let list = answer.value().as_list::<i32>().value(0);
    assert_eq!(
        list.as_primitive::<Float64Type>()
            .iter()
            .collect::<Vec<_>>(),
        [Some(0.9), None, None]
    );
    // An episode Break down stored nothing for asks nothing.
    let (answer, failure) = judgment
        .answer(&episode(), StoredSubtasks(None))
        .await
        .unwrap()
        .into_parts();
    assert!(answer.is_failed());
    assert_eq!(
        failure.as_deref(),
        Some("Break down stored no subtasks for this episode.")
    );
    assert_eq!(unsure.requests().len(), 1);
    let refused = MockOpenRouter::new(|_, _, _| {
        let error = json!({"error": {"code": 401, "message": "User not found."}});
        (axum::http::StatusCode::UNAUTHORIZED, axum::Json(error)).into_response()
    });
    let openrouter = refused.serve().await;
    let judgment = SubtaskJudgment::new(
        SimilarityQuestions::new("fold").unwrap(),
        "typesafe/jev-1.13",
        &openrouter,
    );
    assert_eq!(
        judgment.answer(&episode(), stored()).await.err().unwrap(),
        "OpenRouter returned HTTP 401: User not found."
    );
}
