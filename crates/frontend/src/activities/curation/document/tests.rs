use super::*;
use spiral_robotics_common::datasets::EpisodeId;

fn annotated_graph() -> (CurationGraph, String) {
    let mut payload = CurationGraph::new(None);
    let annotate = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    payload
        .replace_episode_subtasks(
            &annotate,
            &EpisodeId::fixture(1),
            vec![
                SubtaskRow::new(
                    &annotate,
                    EpisodeId::fixture(1),
                    8.0,
                    13.0,
                    "make the first fold",
                )
                .unwrap(),
                SubtaskRow::new(
                    &annotate,
                    EpisodeId::fixture(1),
                    0.0,
                    8.0,
                    " grasp the corners ",
                )
                .unwrap(),
            ],
        )
        .unwrap();
    (payload, annotate)
}

#[test]
fn a_fresh_graph_is_one_dataset_node_that_validates() {
    let payload = CurationGraph::new(None);
    assert_eq!(payload.validate(), Ok(()));
    assert_eq!(payload.nodes().len(), 1);
    assert_eq!(
        payload.node(DATASET_NODE_ID).unwrap().kind().label(),
        "Dataset"
    );
    assert_eq!(payload.parent_of(DATASET_NODE_ID), None);
}

#[test]
fn children_follow_the_menu_and_get_unique_ids() {
    let mut payload = CurationGraph::new(None);
    let first = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    let second = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    assert_eq!(
        (first.as_str(), second.as_str()),
        ("break_down_subtasks-1", "break_down_subtasks-2")
    );
    let search = payload
        .add_child(
            &first,
            CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
        )
        .unwrap();
    assert_eq!(payload.parent_of(&search), Some(first.as_str()));
    assert_eq!(payload.subtask_source(&search), Some(first.as_str()));
    assert!(
        payload
            .add_child(DATASET_NODE_ID, CurationNodeKind::GroupSimilarSubtasks)
            .is_err()
    );
    assert!(
        payload
            .add_child(&search, CurationNodeKind::GroupSimilarSubtasks)
            .is_err()
    );
    assert!(
        payload
            .add_child("missing", CurationNodeKind::GroupSimilarSubtasks)
            .is_err()
    );
    assert_eq!(payload.children_of(DATASET_NODE_ID).len(), 2);
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn episode_rows_are_sorted_trimmed_and_recorded_as_completed() {
    let (payload, annotate) = annotated_graph();
    let rows = payload
        .subtasks_for(&annotate)
        .filter(|row| row.episode() == &EpisodeId::fixture(1))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].description(), "grasp the corners");
    assert_eq!(rows[1].start_time(), 8.0);
    assert_eq!(rows[1].duration(), 5.0);
    let CurationNodeKind::BreakDownSubtasks(annotation) = payload.node(&annotate).unwrap().kind()
    else {
        panic!("annotation node");
    };
    assert!(annotation.is_completed(&EpisodeId::fixture(1)));
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn rows_that_overlap_or_belong_elsewhere_are_rejected_before_any_change() {
    let (mut payload, annotate) = annotated_graph();
    let before = payload.clone();
    let overlapping = vec![
        SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 5.0, "a").unwrap(),
        SubtaskRow::new(&annotate, EpisodeId::fixture(2), 4.0, 9.0, "b").unwrap(),
    ];
    assert!(
        payload
            .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), overlapping)
            .is_err()
    );
    let foreign = vec![SubtaskRow::new("other", EpisodeId::fixture(2), 0.0, 5.0, "a").unwrap()];
    assert!(
        payload
            .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), foreign)
            .is_err()
    );
    assert!(
        payload
            .replace_episode_subtasks(DATASET_NODE_ID, &EpisodeId::fixture(2), Vec::new())
            .is_err()
    );
    assert_eq!(payload, before);
    assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), 5.0, 5.0, "a").is_err());
    assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), -1.0, 5.0, "a").is_err());
    assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 5.0, "  ").is_err());
}

#[test]
fn an_empty_episode_and_a_failure_both_record_progress() {
    let (mut payload, annotate) = annotated_graph();
    payload
        .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), Vec::new())
        .unwrap();
    payload
        .record_episode_failure(&annotate, &EpisodeId::fixture(3), "timed out")
        .unwrap();
    let CurationNodeKind::BreakDownSubtasks(annotation) = payload.node(&annotate).unwrap().kind()
    else {
        panic!("annotation node");
    };
    assert_eq!(
        annotation.completed_episodes(),
        [EpisodeId::fixture(1), EpisodeId::fixture(2)]
    );
    assert_eq!(
        annotation.failed_episodes()[0].episode(),
        &EpisodeId::fixture(3)
    );
    assert_eq!(annotation.failed_episodes()[0].reason(), "timed out");
    payload
        .replace_episode_subtasks(&annotate, &EpisodeId::fixture(3), Vec::new())
        .unwrap();
    let CurationNodeKind::BreakDownSubtasks(annotation) = payload.node(&annotate).unwrap().kind()
    else {
        panic!("annotation node");
    };
    assert!(annotation.failed_episodes().is_empty());
}

#[test]
fn a_rows_poster_round_trips_and_an_older_row_decodes_without_one() {
    let (mut payload, annotate) = annotated_graph();
    let poster = format!("data:image/jpeg;base64,{}", "A".repeat(32));
    payload
        .replace_episode_subtasks(
            &annotate,
            &EpisodeId::fixture(2),
            vec![
                SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 6.0, "unfold")
                    .unwrap()
                    .with_poster(poster.clone()),
            ],
        )
        .unwrap();
    let encoded = payload.encode().unwrap();
    let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    // Only the row that has one carries the field at all.
    assert_eq!(json["subtasks"][0].get("poster"), None);
    assert_eq!(json["subtasks"][2]["poster"], poster);
    let decoded = CurationGraph::decode(CURATION_ENCODING_VERSION, &encoded).unwrap();
    assert_eq!(decoded, payload);
    assert_eq!(
        decoded
            .subtasks_for(&annotate)
            .find(|row| row.episode() == &EpisodeId::fixture(2))
            .unwrap()
            .poster(),
        Some(poster.as_str())
    );
    // A curation saved before posters existed still decodes under
    // `deny_unknown_fields`, and its rows simply have none.
    let older = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"}],"subtasks":[{"node":"a","episode":"00000000000000000000000000000001","start_time":0.0,"end_time":4.0,"description":"grasp"}]}"#;
    let older = CurationGraph::decode(CURATION_ENCODING_VERSION, older).unwrap();
    let row = older.subtasks_for("a").next().unwrap();
    assert_eq!(row.poster(), None);
    assert_eq!(row.midpoint(), 2.0);
    // An empty picture is no picture, so nothing is stored for it.
    assert_eq!(row.clone().with_poster(String::new()).poster(), None);
}

#[test]
fn a_whole_datasets_posters_fit_in_one_curation() {
    // What the sample dataset actually produced: 39 episodes, 311 rows.
    // A poster is about 5.5 kB at 192 pixels wide and JPEG quality 0.7.
    let poster = format!("data:image/jpeg;base64,{}", "A".repeat(5_500));
    let mut payload = CurationGraph::new(None);
    let annotate = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    let rows = (0..311)
        .map(|index| {
            SubtaskRow::new(
                &annotate,
                EpisodeId::fixture(1),
                f64::from(index),
                f64::from(index) + 1.0,
                "fold",
            )
            .unwrap()
            .with_poster(poster.clone())
        })
        .collect();
    payload
        .replace_episode_subtasks(&annotate, &EpisodeId::fixture(1), rows)
        .unwrap();
    let encoded = payload.encode().unwrap().len();
    assert!(
        encoded * 10 < spiral_robotics_common::curation::MAX_CURATION_PAYLOAD_BYTES,
        "a whole dataset's posters are {encoded} bytes, more than a tenth of what an \
         curation may carry"
    );
}

#[test]
fn groupings_read_their_parent_and_order_members_by_probability() {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks)
        .unwrap();
    let categories = vec![
        SubtaskCategory::new("Grasp", "Take hold of the cloth").unwrap(),
        SubtaskCategory::new("Fold", "Bring one part over another").unwrap(),
    ];
    let assignments = vec![
        SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Grasp", 0.9, 0.8).unwrap(),
        SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.7, 0.4).unwrap(),
        SubtaskAssignment::new(EpisodeId::fixture(9), 1.0, "Fold", 0.95, 0.9).unwrap(),
    ];
    let grouping =
        SubtaskGrouping::new(&group, &annotate, categories.clone(), assignments.clone()).unwrap();
    payload.set_grouping(grouping).unwrap();
    let members = payload.grouping_for(&group).unwrap().members("Fold");
    assert_eq!(
        members
            .iter()
            .map(|member| member.episode().clone())
            .collect::<Vec<_>>(),
        [EpisodeId::fixture(9), EpisodeId::fixture(1)]
    );
    assert!(
        SubtaskGrouping::new(
            &group,
            &annotate,
            categories.clone(),
            vec![SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Smooth", 0.5, 0.5).unwrap()]
        )
        .is_err()
    );
    assert!(SubtaskGrouping::new(&group, "elsewhere", categories.clone(), Vec::new()).is_ok());
    assert!(
        payload
            .set_grouping(
                SubtaskGrouping::new(&group, "elsewhere", categories.clone(), Vec::new()).unwrap()
            )
            .is_err()
    );
    assert!(
        payload
            .set_grouping(
                SubtaskGrouping::new(&annotate, &annotate, categories, Vec::new()).unwrap()
            )
            .is_err()
    );
    assert!(SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Grasp", 1.2, 0.5).is_err());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn a_runner_up_must_be_another_known_category_and_older_assignments_have_none() {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks)
        .unwrap();
    let categories = vec![
        SubtaskCategory::new("Grasp", "Take hold of the cloth").unwrap(),
        SubtaskCategory::new("Fold", "Bring one part over another").unwrap(),
    ];
    let assigned = SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
        .unwrap()
        .with_runner_up("Grasp", 0.35)
        .unwrap();
    assert_eq!(assigned.runner_up().map(RunnerUp::category), Some("Grasp"));
    let unknown = SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
        .unwrap()
        .with_runner_up("Smooth", 0.35)
        .unwrap();
    assert!(SubtaskGrouping::new(&group, &annotate, categories.clone(), vec![unknown]).is_err());
    assert!(
        SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
            .unwrap()
            .with_runner_up("Fold", 0.35)
            .is_err()
    );
    payload
        .set_grouping(SubtaskGrouping::new(&group, &annotate, categories, vec![assigned]).unwrap())
        .unwrap();

    let older: SubtaskAssignment = serde_json::from_str(
        r#"{"episode":"00000000000000000000000000000001","start_time":0,
            "category":"Grasp","probability":0.9,"confidence":0.8}"#,
    )
    .unwrap();
    assert!(older.runner_up().is_none());
    assert!(!serde_json::to_string(&older).unwrap().contains("runner_up"));
}

#[test]
fn asking_about_a_row_reuses_the_first_search_node_and_remembers_the_row() {
    let (mut payload, annotate) = annotated_graph();
    let rows = payload.subtasks_for(&annotate).cloned().collect::<Vec<_>>();
    let first = payload.search_for(&annotate, &rows[0]).unwrap();
    let again = payload.search_for(&annotate, &rows[1]).unwrap();
    assert_eq!(first, again);
    assert_eq!(payload.children_of(&annotate).len(), 1);
    let CurationNodeKind::FindSimilarSubtasks(search) = payload.node(&first).unwrap().kind() else {
        panic!("a search node");
    };
    assert_eq!(search.query(), "make the first fold");
    assert_eq!(search.source(), Some(&rows[1].key()));

    let mut typed = search.clone();
    typed.set_query("make the first fold");
    assert_eq!(typed.source(), Some(&rows[1].key()));
    typed.set_query("smooth the cloth");
    assert_eq!(typed.source(), None);

    let elsewhere = SubtaskRow::new("other", EpisodeId::fixture(1), 0.0, 1.0, "grasp").unwrap();
    assert!(payload.search_for(&annotate, &elsewhere).is_err());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn removing_a_node_takes_its_subtree_and_outputs_with_it() {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks)
        .unwrap();
    payload
        .set_grouping(
            SubtaskGrouping::new(
                &group,
                &annotate,
                vec![SubtaskCategory::new("Fold", "Folds").unwrap()],
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(payload.remove_node(DATASET_NODE_ID).is_err());
    payload.remove_node(&annotate).unwrap();
    assert_eq!(payload.nodes().len(), 1);
    assert!(payload.edges().is_empty());
    assert!(payload.subtasks_for(&annotate).next().is_none());
    assert!(payload.grouping_for(&group).is_none());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn encoding_round_trips_and_rejects_other_versions_and_broken_graphs() {
    let (mut payload, annotate) = annotated_graph();
    let search = payload
        .add_child(
            &annotate,
            CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
        )
        .unwrap();
    if let CurationNodeKind::FindSimilarSubtasks(config) =
        payload.node_mut(&search).unwrap().kind_mut()
    {
        config.set_query("make the first fold");
    }
    let encoded = payload.encode().unwrap();
    let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(json["nodes"][1]["kind"]["type"], "break_down_subtasks");
    assert_eq!(json["nodes"][1]["kind"]["model"], DEFAULT_ANNOTATION_MODEL);
    assert_eq!(json["nodes"][2]["kind"]["query"], "make the first fold");
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &encoded).unwrap(),
        payload
    );
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION + 1, &encoded).is_err());
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, r#"{"nodes":[]}"#).is_err());
    let two_parents = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}},{"id":"b","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"},{"from":"dataset","to":"b"},{"from":"a","to":"b"}]}"#;
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, two_parents).is_err());
    let unknown_field = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"colour":"blue"}"#;
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, unknown_field).is_err());
}
