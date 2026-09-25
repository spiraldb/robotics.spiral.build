use super::*;
use crate::activities::curation::annotate::SubtaskRow;
use spiral_robotics_common::datasets::EpisodeId;

fn annotated_graph() -> (CurationGraph, String) {
    let mut payload = CurationGraph::new(None);
    let annotate = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    (payload, annotate)
}

fn categories(names: &[&str]) -> Vec<SubtaskCategory> {
    names
        .iter()
        .map(|name| SubtaskCategory::new(*name, format!("Everything that {name}s.")).unwrap())
        .collect()
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
    let group = || CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default());
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
    assert!(payload.add_child(DATASET_NODE_ID, group()).is_err());
    assert!(payload.add_child(&search, group()).is_err());
    assert!(payload.add_child("missing", group()).is_err());
    assert_eq!(payload.children_of(DATASET_NODE_ID).len(), 2);
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn a_grouping_holds_its_categories_and_refuses_a_list_the_classifier_would() {
    let mut grouping = SubtaskGrouping::default();
    assert!(
        grouping.categories().is_empty(),
        "none before any are proposed"
    );
    grouping
        .set_categories(categories(&["fold", "grasp"]))
        .unwrap();
    assert_eq!(grouping.categories()[1].name(), "grasp");
    // The same name twice would be two indistinguishable options.
    assert!(
        grouping
            .set_categories(categories(&["fold", "fold"]))
            .unwrap_err()
            .contains("appears twice")
    );
    let too_many = (0..256)
        .map(|index| format!("c{index}"))
        .collect::<Vec<_>>();
    let too_many = too_many.iter().map(String::as_str).collect::<Vec<_>>();
    assert!(grouping.set_categories(categories(&too_many)).is_err());
    // A refused list leaves the categories as they were.
    assert_eq!(grouping.categories().len(), 2);
    assert!(SubtaskCategory::new("fold", " ").is_err());

    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks(grouping))
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][2]["kind"],
        serde_json::json!({
            "type": "group_similar_subtasks",
            "categories": [
                {"name": "fold", "definition": "Everything that folds."},
                {"name": "grasp", "definition": "Everything that grasps."},
            ],
        })
    );
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &payload.encode().unwrap()).unwrap(),
        payload
    );
    // A document naming a category twice is refused on the way in too.
    let twice = payload.encode().unwrap().replace("\"grasp\"", "\"fold\"");
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, &twice).is_err());
    assert!(payload.node(&group).is_some());
}

#[test]
fn asking_about_a_row_reuses_the_first_search_node_and_remembers_the_row() {
    let (mut payload, annotate) = annotated_graph();
    let rows = [
        SubtaskRow::new(EpisodeId::fixture(1), (0.0, 8.0), " grasp the corners ").unwrap(),
        SubtaskRow::new(EpisodeId::fixture(1), (8.0, 13.0), "make the first fold").unwrap(),
    ];
    let first = payload.search_for(&annotate, &rows[0]).unwrap();
    let again = payload.search_for(&annotate, &rows[1]).unwrap();
    assert_eq!(first, again);
    assert_eq!(payload.children_of(&annotate).len(), 1);
    let CurationNodeKind::FindSimilarSubtasks(search) = payload.node(&first).unwrap().kind() else {
        panic!("a search node");
    };
    assert_eq!(search.query(), "make the first fold");
    assert_eq!(search.source(), Some(&rows[1].key()));
    assert!(search.source().unwrap().is(&rows[1]));

    let mut typed = search.clone();
    typed.set_query("make the first fold");
    assert_eq!(typed.source(), Some(&rows[1].key()));
    typed.set_query("smooth the cloth");
    assert_eq!(typed.source(), None);

    // Only a Break down node is asked about its rows.
    assert!(payload.search_for(DATASET_NODE_ID, &rows[0]).is_err());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn removing_a_node_takes_its_subtree_with_it() {
    let (mut payload, annotate) = annotated_graph();
    payload
        .add_child(
            &annotate,
            CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
        )
        .unwrap();
    assert!(payload.remove_node(DATASET_NODE_ID).is_err());
    payload.remove_node(&annotate).unwrap();
    assert_eq!(payload.nodes().len(), 1);
    assert!(payload.edges().is_empty());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn encoding_round_trips_and_rejects_other_versions_and_broken_graphs() {
    let (mut payload, annotate) = annotated_graph();
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        payload.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/top-camera".into()));
        annotation.set_task_topic(Some("/instruction".into()));
    }
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
    assert_eq!(json["nodes"][1]["kind"]["task_topic"], "/instruction");
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

#[test]
fn every_node_saves_its_recipe_and_nothing_it_read() {
    let mut payload = CurationGraph::new(None);
    payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][1]["kind"],
        serde_json::json!({"type": "identify_unproductive_moments"})
    );
    assert!(json.get("moments").is_none());
    // What encoding 2 used to store for the nodes that did not compile yet is
    // refused rather than carried: the preview reads it again.
    for stored in [
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"m","kind":{"type":"identify_unproductive_moments","completed_episodes":[]}}],"edges":[{"from":"dataset","to":"m"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5,"completed_episodes":[]}}],"edges":[{"from":"dataset","to":"a"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"subtasks":[]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"groupings":[]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"rankings":[]}"#,
    ] {
        assert!(
            CurationGraph::decode(CURATION_ENCODING_VERSION, stored).is_err(),
            "{stored}"
        );
    }
    // A grouping saved before categories were its configuration decodes as
    // one with none yet.
    let unconfigured = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}},{"id":"g","kind":{"type":"group_similar_subtasks"}}],"edges":[{"from":"dataset","to":"a"},{"from":"a","to":"g"}]}"#;
    let decoded = CurationGraph::decode(CURATION_ENCODING_VERSION, unconfigured).unwrap();
    assert_eq!(
        decoded.node("g").unwrap().kind(),
        &CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default())
    );
}

/// A graph with two outputs over whole episodes: longer and shorter ones.
fn two_outputs() -> (CurationGraph, String, String) {
    let mut payload = CurationGraph::new(None);
    let rule = |comparison| {
        CurationNodeKind::Select(Selection::Episodes {
            fact: EpisodeFact::DurationSeconds,
            comparison,
            value: 10.0,
        })
    };
    let long = payload
        .add_child(DATASET_NODE_ID, rule(Comparison::AtLeast))
        .unwrap();
    let short = payload
        .add_child(DATASET_NODE_ID, rule(Comparison::AtMost))
        .unwrap();
    (payload, long, short)
}

#[test]
fn a_view_is_current_until_a_later_save_changes_the_recipe() {
    let (mut payload, long, short) = two_outputs();
    // Published from the recipe saved at 3; the save recording it is 4.
    payload
        .record_publication(&long, "long_episodes", 3)
        .unwrap();
    let published = payload.publication(&long).unwrap().clone();
    assert_eq!(published.view(), "long_episodes");
    assert!(published.is_current(3) && published.is_current(4));
    // An edit saved as 5 moves the recipe past the view.
    assert!(!published.is_current(5));
    // Publishing the other output records nothing new about the first: a
    // view current before stays current, and one out of date stays so.
    let mut current = payload.clone();
    current
        .record_publication(&short, "short_episodes", 4)
        .unwrap();
    assert!(current.publication(&long).unwrap().is_current(5));
    payload
        .record_publication(&short, "short_episodes", 5)
        .unwrap();
    assert!(!payload.publication(&long).unwrap().is_current(6));
    assert!(payload.publication(&short).unwrap().is_current(6));
    // Unpublishing forgets the output, and only a published one.
    payload.remove_publication(&short, 6).unwrap();
    assert!(payload.publication(&short).is_none());
    assert!(payload.remove_publication(&short, 7).is_err());
}

#[test]
fn a_publication_names_one_output_by_one_view_name() {
    let (mut payload, long, short) = two_outputs();
    assert!(
        payload
            .record_publication(DATASET_NODE_ID, "episodes", 1)
            .is_err()
    );
    assert!(
        payload
            .record_publication(&long, "long episodes", 1)
            .is_err()
    );
    payload.record_publication(&long, "episodes", 1).unwrap();
    assert!(payload.record_publication(&short, "episodes", 2).is_err());
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn a_published_output_stays_an_output_until_it_is_unpublished() {
    let (mut payload, long, _) = two_outputs();
    payload
        .record_publication(&long, "long_episodes", 1)
        .unwrap();
    // A step after it would make it no longer an output.
    let longer = CurationNodeKind::Select(Selection::Episodes {
        fact: EpisodeFact::DurationSeconds,
        comparison: Comparison::AtLeast,
        value: 20.0,
    });
    let mut attempt = payload.clone();
    attempt.add_child(&long, longer).unwrap();
    assert!(attempt.validate().unwrap_err().contains("unpublish it"));
    // Nor does its branch go while the view is there.
    assert!(
        payload
            .remove_node(&long)
            .unwrap_err()
            .contains("long_episodes")
    );
    payload.remove_publication(&long, 2).unwrap();
    payload.remove_node(&long).unwrap();
}
