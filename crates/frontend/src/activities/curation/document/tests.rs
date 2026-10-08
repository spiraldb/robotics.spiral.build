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

fn select() -> CurationNodeKind {
    CurationNodeKind::Select(Selection::keeping_everything())
}

fn labels(kinds: &[CurationNodeKind]) -> Vec<&'static str> {
    kinds.iter().map(CurationNodeKind::label).collect()
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
    assert!(payload.outputs().is_empty(), "the dataset never publishes");
}

#[test]
fn anything_follows_anything_but_a_judgment_needs_a_break_down_above() {
    let mut payload = CurationGraph::new(None);
    assert_eq!(
        labels(&payload.child_kinds(DATASET_NODE_ID)),
        [
            "Break down subtasks",
            "Score progress",
            "Track objects",
            "Select"
        ]
    );
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
    // Below a Break down, through anything, a judgment may follow.
    let narrowed = payload.add_child(&first, select()).unwrap();
    let scored = payload
        .add_child(
            &narrowed,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    assert_eq!(
        labels(&payload.child_kinds(&scored)),
        [
            "Break down subtasks",
            "Score progress",
            "Track objects",
            "Find similar subtasks",
            "Group similar subtasks",
            "Select"
        ]
    );
    let search = payload
        .add_child(
            &scored,
            CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
        )
        .unwrap();
    assert_eq!(payload.subtask_source(&search), Some(first.as_str()));
    // A Select may follow a Select, and a model a judgment.
    let again = payload.add_child(&narrowed, select()).unwrap();
    assert!(
        payload
            .add_child(
                &search,
                CurationNodeKind::TrackObjects(ObjectTracking::default())
            )
            .is_ok()
    );
    let group = || CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default());
    assert!(payload.add_child(DATASET_NODE_ID, group()).is_err());
    assert!(payload.add_child(&again, group()).is_ok());
    assert!(payload.add_child("missing", group()).is_err());
    assert_eq!(payload.children_of(DATASET_NODE_ID).len(), 2);
    assert_eq!(payload.validate(), Ok(()));
    // Every node but the dataset may be published.
    assert_eq!(payload.outputs().len(), payload.nodes().len() - 1);
    // A judgment saved without a Break down above it is refused.
    let orphan = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"find_similar_subtasks-1","kind":{"type":"find_similar_subtasks","query":"fold"}}],"edges":[{"from":"dataset","to":"find_similar_subtasks-1"}]}"#;
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, orphan).is_err());
}

/// A graph whose grouping has proposed `names`, a Select below it choosing
/// among them, and both their ids.
fn grouped(names: &[&str]) -> (CurationGraph, String, String) {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(
            &annotate,
            CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
        )
        .unwrap();
    payload
        .propose_categories(&group, categories(names))
        .unwrap();
    let chooser = payload.add_child(&group, select()).unwrap();
    let chosen = payload
        .conditions_for(&chooser)
        .last()
        .unwrap()
        .condition()
        .clone();
    payload
        .set_selection(&chooser, Selection::of(vec![chosen]))
        .unwrap();
    (payload, group, chooser)
}

/// The categories, by name, the Select `node` chooses.
fn chosen(payload: &CurationGraph, node: &str) -> Vec<String> {
    let CurationNodeKind::Select(selection) = payload.node(node).unwrap().kind() else {
        panic!("a Select");
    };
    let [
        SelectCondition::InCategories {
            node: grouping,
            categories,
        },
    ] = selection.conditions()
    else {
        panic!("one category condition, not {selection:?}");
    };
    let CurationNodeKind::GroupSimilarSubtasks(grouping) = payload.node(grouping).unwrap().kind()
    else {
        panic!("a grouping");
    };
    categories
        .iter()
        .map(|id| grouping.category(*id).unwrap().name().to_owned())
        .collect()
}

#[test]
fn a_grouping_numbers_its_categories_and_refuses_a_list_the_classifier_would() {
    let (mut payload, group, chooser) = grouped(&["fold", "grasp"]);
    // The same name twice would be two indistinguishable options.
    assert!(
        payload
            .propose_categories(&group, categories(&["fold", "fold"]))
            .unwrap_err()
            .contains("appears twice")
    );
    let too_many = (0..256)
        .map(|index| format!("c{index}"))
        .collect::<Vec<_>>();
    let too_many = too_many.iter().map(String::as_str).collect::<Vec<_>>();
    assert!(
        payload
            .propose_categories(&group, categories(&too_many))
            .is_err()
    );
    // A refused list leaves the categories, and what is chosen, as they were.
    assert_eq!(chosen(&payload, &chooser), ["fold", "grasp"]);
    assert!(SubtaskCategory::new("fold", " ").is_err());

    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][2]["kind"],
        serde_json::json!({
            "type": "group_similar_subtasks",
            "categories": [
                {"id": 1, "name": "fold", "definition": "Everything that folds."},
                {"id": 2, "name": "grasp", "definition": "Everything that grasps."},
            ],
        })
    );
    assert_eq!(
        json["nodes"][3]["kind"],
        serde_json::json!({
            "type": "select",
            "conditions": [{"keep": "in_categories", "node": group, "categories": [1, 2]}],
        })
    );
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &payload.encode().unwrap()).unwrap(),
        payload
    );
    // A document naming a category twice is refused on the way in too, and
    // so is a Select choosing a category its grouping does not have.
    let twice = payload.encode().unwrap().replace("\"grasp\"", "\"fold\"");
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, &twice).is_err());
    let stray = payload
        .encode()
        .unwrap()
        .replace("\"categories\":[1,2]", "\"categories\":[1,7]");
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, &stray).is_err());
}

#[test]
fn a_select_chooses_the_same_categories_through_every_edit_of_them() {
    let (mut payload, group, chooser) = grouped(&["fold", "grasp", "smooth"]);
    let only = |payload: &mut CurationGraph, ids: &[u32]| {
        let id = |raw: &u32| serde_json::from_value::<CategoryId>(serde_json::json!(raw)).unwrap();
        payload.set_selection(
            &chooser,
            Selection::of(vec![SelectCondition::InCategories {
                node: group.clone(),
                categories: ids.iter().map(id).collect(),
            }]),
        )
    };
    only(&mut payload, &[1, 3]).unwrap();
    assert!(
        only(&mut payload, &[9]).is_err(),
        "a category the grouping does not name"
    );
    // A renamed category keeps its id, so the Select still chooses it, and a
    // name another category holds is refused.
    payload
        .edit_category(
            &group,
            "fold",
            SubtaskCategory::new("fold in half", "Folding the cloth over itself.").unwrap(),
        )
        .unwrap();
    payload
        .edit_category(
            &group,
            "grasp",
            SubtaskCategory::new("reach", "Moving toward the cloth.").unwrap(),
        )
        .unwrap();
    assert!(
        payload
            .edit_category(
                &group,
                "reach",
                SubtaskCategory::new("smooth", "Flat.").unwrap()
            )
            .is_err()
    );
    assert_eq!(chosen(&payload, &chooser), ["fold in half", "smooth"]);
    // Merging drops the merged category; a Select that chose it chooses the
    // one it joined, which keeps both definitions.
    payload.merge_category(&group, "smooth", "reach").unwrap();
    assert_eq!(chosen(&payload, &chooser), ["fold in half", "reach"]);
    let CurationNodeKind::GroupSimilarSubtasks(grouping) = payload.node(&group).unwrap().kind()
    else {
        panic!("a grouping");
    };
    assert_eq!(
        grouping
            .categories()
            .iter()
            .map(|category| (category.name(), category.definition()))
            .collect::<Vec<_>>(),
        [
            ("fold in half", "Folding the cloth over itself."),
            ("reach", "Moving toward the cloth. Everything that smooths.")
        ]
    );
    assert!(payload.merge_category(&group, "reach", "reach").is_err());
    assert!(payload.merge_category(&group, "gone", "reach").is_err());
    // A list proposed afresh is chosen whole.
    payload
        .propose_categories(&group, categories(&["lift", "place"]))
        .unwrap();
    assert_eq!(chosen(&payload, &chooser), ["lift", "place"]);
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn a_select_reads_only_nodes_above_it_of_the_kind_each_condition_reads() {
    let mut payload = CurationGraph::new(None);
    let mut tracking = ObjectTracking::default();
    tracking.set_prompt("bottle");
    let track = payload
        .add_child(DATASET_NODE_ID, CurationNodeKind::TrackObjects(tracking))
        .unwrap();
    let score = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let fastest = payload.add_child(&track, select()).unwrap();
    // The menu offers the facts and what the Track objects above counted.
    assert_eq!(
        payload
            .conditions_for(&fastest)
            .iter()
            .map(|choice| choice.label().to_owned())
            .collect::<Vec<_>>(),
        [
            "Duration",
            "Message count",
            "Channel count",
            "File size",
            "Most bottles at once",
            "Duration per bottle",
            "Bottles visible"
        ]
    );
    let per_bottle = SelectCondition::Episodes(EpisodeCondition::new(
        EpisodeNumber::DurationPer(track.clone()),
        Comparison::Lowest,
        0.25,
    ));
    payload
        .set_selection(&fastest, Selection::of(vec![per_bottle.clone()]))
        .unwrap();
    // A node in another branch, or of another kind, is no column to read.
    for stray in [
        SelectCondition::Progressing {
            node: score.clone(),
            minimum: 0.2,
        },
        SelectCondition::Similar {
            node: track.clone(),
            minimum: 0.5,
        },
    ] {
        assert!(
            payload
                .set_selection(&fastest, Selection::of(vec![stray]))
                .is_err()
        );
    }
    // A share is between none and all, and a Select names a condition.
    let share = |value| {
        Selection::of(vec![SelectCondition::Episodes(EpisodeCondition::new(
            EpisodeNumber::MostAtOnce(track.clone()),
            Comparison::Highest,
            value,
        ))])
    };
    assert!(payload.set_selection(&fastest, share(1.5)).is_err());
    assert!(payload.set_selection(&fastest, share(0.1)).is_ok());
    assert!(
        payload
            .set_selection(&fastest, Selection::of(Vec::new()))
            .is_err()
    );
    payload
        .set_selection(&fastest, Selection::of(vec![per_bottle]))
        .unwrap();
    let CurationNodeKind::Select(selection) = payload.node(&fastest).unwrap().kind() else {
        panic!("a Select");
    };
    assert_eq!(
        selection.sentence(&payload),
        "Duration per bottle in the lowest 25%"
    );
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][3]["kind"]["conditions"],
        serde_json::json!([{"keep": "episodes", "number": {"duration_per": track}, "comparison": "lowest", "value": 0.25}])
    );
    // Removing the Track objects node takes its Select with it, so no
    // condition ever reads a node that is gone.
    payload.remove_node(&track).unwrap();
    assert!(payload.node(&fastest).is_none());
    assert_eq!(payload.validate(), Ok(()));
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
fn the_model_nodes_below_a_node_read_once_it_has_through_any_select_between() {
    let mut payload = CurationGraph::new(None);
    let select = payload.add_child(DATASET_NODE_ID, select()).unwrap();
    let annotate = payload
        .add_child(
            &select,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    let score = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let search = payload
        .add_child(
            &annotate,
            CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
        )
        .unwrap();
    let ids = |node: &str| {
        payload
            .measures_below(node)
            .into_iter()
            .map(|below| below.id().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(DATASET_NODE_ID), [annotate.clone(), score]);
    assert_eq!(ids(&annotate), std::slice::from_ref(&search));
    assert!(ids(&select).is_empty());
    assert!(ids(&search).is_empty());
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
    assert_eq!(json["nodes"][1]["kind"]["task_topic"], "/instruction");
    assert_eq!(json["nodes"][2]["kind"]["query"], "make the first fold");
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &encoded).unwrap(),
        payload
    );
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION - 1, &encoded).is_err());
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, r#"{"nodes":[]}"#).is_err());
    let two_parents = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5}},{"id":"b","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"},{"from":"dataset","to":"b"},{"from":"a","to":"b"}]}"#;
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, two_parents).is_err());
    let unknown_field = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"colour":"blue"}"#;
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, unknown_field).is_err());
    // A node id is what its columns are named by, so it is one identifier
    // once its hyphen is a stroke, and no two ids name the same columns.
    for ids in [("a b", "c"), ("a-1", "a_1")] {
        let clash = format!(
            r#"{{"nodes":[{{"id":"dataset","kind":{{"type":"dataset"}}}},{{"id":"{}","kind":{{"type":"score_progress"}}}},{{"id":"{}","kind":{{"type":"score_progress"}}}}],"edges":[{{"from":"dataset","to":"{}"}},{{"from":"dataset","to":"{}"}}]}}"#,
            ids.0, ids.1, ids.0, ids.1
        );
        assert!(
            CurationGraph::decode(CURATION_ENCODING_VERSION, &clash).is_err(),
            "{clash}"
        );
    }
}

#[test]
fn every_node_saves_its_recipe_and_nothing_it_read() {
    let mut payload = CurationGraph::new(None);
    payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][1],
        serde_json::json!({"id": "score_progress-1", "kind": {"type": "score_progress"}})
    );
    // What earlier encodings stored is refused rather than carried: a rule on
    // a model node, and a Select without its conditions.
    for stored in [
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"m","kind":{"type":"score_progress"},"keeps":{"keep":"progressing","minimum":0.5}}],"edges":[{"from":"dataset","to":"m"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"s","kind":{"type":"select"}}],"edges":[{"from":"dataset","to":"s"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"t","kind":{"type":"track_objects","camera":"/top","prompt":"bottle"}}],"edges":[{"from":"dataset","to":"t"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"subtasks":[]}"#,
    ] {
        assert!(
            CurationGraph::decode(CURATION_ENCODING_VERSION, stored).is_err(),
            "{stored}"
        );
    }
}

/// A graph with two Selects over whole episodes: longer and shorter ones.
fn two_outputs() -> (CurationGraph, String, String) {
    let mut payload = CurationGraph::new(None);
    let mut add = |comparison| {
        let id = payload.add_child(DATASET_NODE_ID, select()).unwrap();
        payload
            .set_selection(
                &id,
                Selection::episodes_where(EpisodeFact::DurationSeconds, comparison, 10.0),
            )
            .unwrap();
        id
    };
    let (long, short) = (add(Comparison::AtLeast), add(Comparison::AtMost));
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
    // Publishing the other node records nothing new about the first: a
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
    // Unpublishing forgets the node, and only a published one.
    payload.remove_publication(&short, 6).unwrap();
    assert!(payload.publication(&short).is_none());
    assert!(payload.remove_publication(&short, 7).is_err());
}

#[test]
fn a_copy_unpublished_elsewhere_differs_by_its_publications_alone() {
    let (mut open, long, short) = two_outputs();
    open.record_publication(&long, "long_episodes", 3).unwrap();
    open.record_publication(&short, "short_episodes", 4)
        .unwrap();
    // Data unpublished one view and stored the curation.
    let mut stored = open.clone();
    stored.remove_publication(&long, 5).unwrap();
    assert!(open.same_recipe(&stored));
    open.take_publications(&stored);
    assert_eq!(open, stored);
    // A recipe edited elsewhere is another writer's change.
    let mut edited = stored.clone();
    edited.add_child(DATASET_NODE_ID, select()).unwrap();
    assert!(!open.same_recipe(&edited));
}

#[test]
fn the_views_a_stored_payload_publishes_are_read_without_decoding_it() {
    let (mut payload, long, _) = two_outputs();
    payload
        .record_publication(&long, "long_episodes", 3)
        .unwrap();
    assert_eq!(
        PublishedOutput::views_in(&payload.encode().unwrap()),
        ["long_episodes"]
    );
    // A payload naming none, or not a payload at all, publishes nothing.
    assert!(PublishedOutput::views_in(r#"{"nodes":[]}"#).is_empty());
    assert!(PublishedOutput::views_in("not a payload").is_empty());
}

#[test]
fn a_publication_names_one_node_by_one_view_name() {
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
fn a_published_node_keeps_its_branch_until_it_is_unpublished() {
    let (mut payload, long, _) = two_outputs();
    payload
        .record_publication(&long, "long_episodes", 1)
        .unwrap();
    // A step after it leaves it published as it was.
    let mut followed = payload.clone();
    followed
        .add_child(
            &long,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    assert_eq!(followed.validate(), Ok(()));
    assert!(followed.publication(&long).is_some());
    // Its branch does not go while the view is there.
    assert!(
        payload
            .remove_node(&long)
            .unwrap_err()
            .contains("long_episodes")
    );
    payload.remove_publication(&long, 2).unwrap();
    payload.remove_node(&long).unwrap();
}

#[test]
fn a_track_objects_node_saves_what_it_tracks_and_its_rate() {
    let mut payload = CurationGraph::new(None);
    let mut tracking = ObjectTracking::default();
    tracking.set_camera(Some("/top-camera".to_owned()));
    tracking.set_prompt("  bottle ");
    assert_eq!(tracking.rate(), 5, "five pictures a second by default");
    assert!(tracking.set_rate(0).is_err() && tracking.set_rate(31).is_err());
    tracking.set_rate(2).unwrap();
    let track = payload
        .add_child(DATASET_NODE_ID, CurationNodeKind::TrackObjects(tracking))
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][1]["kind"],
        serde_json::json!({"type": "track_objects", "camera": "/top-camera", "prompt": "bottle", "rate": 2})
    );
    // It keeps nothing by itself, so whatever reads episodes may follow it.
    assert_eq!(
        labels(&payload.child_kinds(&track)),
        [
            "Break down subtasks",
            "Score progress",
            "Track objects",
            "Select"
        ]
    );
    let plural = |prompt: &str| {
        let mut tracking = ObjectTracking::default();
        tracking.set_prompt(prompt);
        tracking.plural()
    };
    assert_eq!(
        ["bottle", "box", "cherry", "toy", "glass", ""].map(plural),
        ["bottles", "boxes", "cherries", "toys", "glasses", "objects"]
    );
}
