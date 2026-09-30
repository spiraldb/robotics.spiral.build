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
    // A search keeps by its own cutoff, so what follows reads what it kept;
    // there is nothing left for a Select to keep by.
    assert!(
        payload
            .add_child(&search, CurationNodeKind::Select)
            .is_err()
    );
    assert!(payload.add_child(&search, group()).is_ok());
    assert!(payload.add_child("missing", group()).is_err());
    assert_eq!(payload.children_of(DATASET_NODE_ID).len(), 2);
    assert_eq!(payload.validate(), Ok(()));
}

#[test]
fn each_node_keeps_by_the_rule_its_kind_and_place_allow() {
    let mut payload = CurationGraph::new(None);
    let rule = |id: &str, payload: &CurationGraph| {
        payload
            .node(id)
            .and_then(CurationNode::keeps)
            .map(Selection::basis)
    };
    let score = payload
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let episodes = payload
        .add_child(DATASET_NODE_ID, CurationNodeKind::Select)
        .unwrap();
    let annotate = payload
        .add_child(
            &score,
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
        )
        .unwrap();
    let search = payload
        .add_child(
            &annotate,
            CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
        )
        .unwrap();
    let group = payload
        .add_child(
            &annotate,
            CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
        )
        .unwrap();
    assert_eq!(rule(&score, &payload), Some(SelectionBasis::Progress));
    assert_eq!(
        rule(&episodes, &payload),
        Some(SelectionBasis::EpisodeFacts)
    );
    assert_eq!(rule(&search, &payload), Some(SelectionBasis::Similarity));
    assert_eq!(rule(&group, &payload), Some(SelectionBasis::Categories));
    assert_eq!(rule(&annotate, &payload), None);
    // A Select reads only what the dataset recorded, so it follows nothing a
    // model answered; a grouping chooses its own categories.
    for measure in [&score, &annotate, &search, &group] {
        assert!(
            payload
                .add_child(measure, CurationNodeKind::Select)
                .is_err(),
            "{measure}"
        );
    }
    // Every one of them is an output but Break down.
    assert_eq!(
        payload
            .outputs()
            .iter()
            .map(|output| output.id())
            .collect::<Vec<_>>(),
        [
            score.as_str(),
            episodes.as_str(),
            search.as_str(),
            group.as_str()
        ]
    );
    // A rule of another basis is refused, and one the kind keeps by is taken.
    assert!(
        payload
            .set_rule(&score, Selection::Similar { minimum: 0.5 })
            .is_err()
    );
    assert!(
        payload
            .set_rule(&annotate, Selection::Similar { minimum: 0.5 })
            .is_err()
    );
    payload
        .set_rule(&score, Selection::Progressing { minimum: 0.4 })
        .unwrap();
    assert!(
        payload
            .set_rule(
                &episodes,
                Selection::Episodes {
                    conditions: Vec::new()
                }
            )
            .is_err(),
        "a Select of episodes names at least one condition"
    );
    assert_eq!(payload.validate(), Ok(()));
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &payload.encode().unwrap()).unwrap(),
        payload
    );
}

/// A graph whose grouping has proposed `names`, and the grouping's id.
fn grouped(names: &[&str]) -> (CurationGraph, String) {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(
            &annotate,
            CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
        )
        .unwrap();
    payload
        .node_mut(&group)
        .unwrap()
        .propose_categories(categories(names))
        .unwrap();
    (payload, group)
}

/// The names `node` keeps, in its rule's order.
fn kept(payload: &CurationGraph, node: &str) -> Vec<String> {
    match payload.node(node).and_then(CurationNode::keeps) {
        Some(Selection::Categories { names }) => names.clone(),
        other => panic!("a rule of categories, not {other:?}"),
    }
}

#[test]
fn a_grouping_holds_its_categories_and_refuses_a_list_the_classifier_would() {
    let (mut payload, annotate) = annotated_graph();
    let group = payload
        .add_child(
            &annotate,
            CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
        )
        .unwrap();
    assert!(
        kept(&payload, &group).is_empty(),
        "none before any are proposed"
    );
    let node = payload.node_mut(&group).unwrap();
    node.propose_categories(categories(&["fold", "grasp"]))
        .unwrap();
    // The same name twice would be two indistinguishable options.
    assert!(
        node.propose_categories(categories(&["fold", "fold"]))
            .unwrap_err()
            .contains("appears twice")
    );
    let too_many = (0..256)
        .map(|index| format!("c{index}"))
        .collect::<Vec<_>>();
    let too_many = too_many.iter().map(String::as_str).collect::<Vec<_>>();
    assert!(node.propose_categories(categories(&too_many)).is_err());
    // A refused list leaves the categories, and what is kept, as they were.
    assert_eq!(kept(&payload, &group), ["fold", "grasp"]);
    assert!(SubtaskCategory::new("fold", " ").is_err());

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
        json["nodes"][2]["keeps"],
        serde_json::json!({"keep": "categories", "names": ["fold", "grasp"]})
    );
    assert_eq!(
        CurationGraph::decode(CURATION_ENCODING_VERSION, &payload.encode().unwrap()).unwrap(),
        payload
    );
    // A document naming a category twice is refused on the way in too, and
    // so is one keeping a category its grouping does not name.
    let twice = payload.encode().unwrap().replace("\"grasp\"", "\"fold\"");
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, &twice).is_err());
    let stray = payload
        .encode()
        .unwrap()
        .replace("\"names\":[\"fold\",", "\"names\":[\"flatten\",");
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, &stray).is_err());
}

#[test]
fn a_grouping_keeps_what_it_chose_through_every_edit_of_its_categories() {
    let (mut payload, group) = grouped(&["fold", "grasp", "smooth"]);
    assert_eq!(kept(&payload, &group), ["fold", "grasp", "smooth"]);
    // Leaving a category out is a rule of the grouping's own names.
    let only = |names: &[&str]| Selection::Categories {
        names: names.iter().map(|name| (*name).to_owned()).collect(),
    };
    payload.set_rule(&group, only(&["fold", "smooth"])).unwrap();
    assert!(
        payload.set_rule(&group, only(&["flatten"])).is_err(),
        "a category the grouping does not name"
    );
    let node = payload.node_mut(&group).unwrap();
    // A renamed category keeps its standing under its new name, and a name
    // another category holds is refused.
    node.edit_category(
        "fold",
        SubtaskCategory::new("fold in half", "Folding the cloth over itself.").unwrap(),
    )
    .unwrap();
    node.edit_category(
        "grasp",
        SubtaskCategory::new("reach", "Moving toward the cloth.").unwrap(),
    )
    .unwrap();
    assert!(
        node.edit_category("reach", SubtaskCategory::new("smooth", "Flat.").unwrap())
            .is_err()
    );
    assert_eq!(kept(&payload, &group), ["fold in half", "smooth"]);
    // Merging drops the merged category, and the one it joins keeps both
    // definitions and its own standing.
    let node = payload.node_mut(&group).unwrap();
    node.merge_category("smooth", "reach").unwrap();
    assert_eq!(kept(&payload, &group), ["fold in half"]);
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
    let node = payload.node_mut(&group).unwrap();
    assert!(node.merge_category("reach", "reach").is_err());
    assert!(node.merge_category("gone", "reach").is_err());
    // A list proposed afresh is kept whole.
    node.propose_categories(categories(&["lift", "place"]))
        .unwrap();
    assert_eq!(kept(&payload, &group), ["lift", "place"]);
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
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION + 1, &encoded).is_err());
    assert!(CurationGraph::decode(CURATION_ENCODING_VERSION, r#"{"nodes":[]}"#).is_err());
    let two_parents = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5}},{"id":"b","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"},{"from":"dataset","to":"b"},{"from":"a","to":"b"}]}"#;
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
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    assert_eq!(
        json["nodes"][1]["kind"],
        serde_json::json!({"type": "score_progress"})
    );
    assert_eq!(
        json["nodes"][1]["keeps"],
        serde_json::json!({"keep": "progressing", "minimum": 0.0})
    );
    assert!(json.get("moments").is_none());
    // What earlier encodings stored is refused rather than carried: what the
    // nodes read, which the preview reads again; a Select holding one fact of
    // its own; and a rule a node's kind or place does not keep by.
    for stored in [
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"s","kind":{"type":"select","keep":"episodes","fact":"duration_seconds","comparison":"at_least","value":10.0}}],"edges":[{"from":"dataset","to":"s"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"s","kind":{"type":"select"}}],"edges":[{"from":"dataset","to":"s"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5},"keeps":{"keep":"similar","minimum":0.5}}],"edges":[{"from":"dataset","to":"a"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"m","kind":{"type":"score_progress"},"keeps":{"keep":"similar","minimum":0.5}}],"edges":[{"from":"dataset","to":"m"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"m","kind":{"type":"score_progress","completed_episodes":[]}}],"edges":[{"from":"dataset","to":"m"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5,"completed_episodes":[]}}],"edges":[{"from":"dataset","to":"a"}]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"subtasks":[]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"groupings":[]}"#,
        r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"rankings":[]}"#,
    ] {
        assert!(
            CurationGraph::decode(CURATION_ENCODING_VERSION, stored).is_err(),
            "{stored}"
        );
    }
    // A grouping keeps by its own categories, so one saved without that
    // rule, or with a Select choosing them after it, is refused.
    let grouping = r#"{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","sample_interval_seconds":0.5}},{"id":"g","kind":{"type":"group_similar_subtasks","categories":[{"name":"fold","definition":"Folding."}]}"#;
    let edges = r#"{"from":"dataset","to":"a"},{"from":"a","to":"g"}"#;
    for stored in [
        format!(r#"{{"nodes":[{grouping}}}],"edges":[{edges}]}}"#),
        format!(
            r#"{{"nodes":[{grouping},"keeps":{{"keep":"categories","names":["fold"]}}}},{{"id":"s","kind":{{"type":"select"}},"keeps":{{"keep":"categories","names":["fold"]}}}}],"edges":[{edges},{{"from":"g","to":"s"}}]}}"#
        ),
    ] {
        assert!(
            CurationGraph::decode(CURATION_ENCODING_VERSION, &stored).is_err(),
            "{stored}"
        );
    }
    let unproposed = format!(
        r#"{{"nodes":[{},"keeps":{{"keep":"categories","names":[]}}}}],"edges":[{edges}]}}"#,
        grouping.replace(
            r#","categories":[{"name":"fold","definition":"Folding."}]"#,
            ""
        )
    );
    let decoded = CurationGraph::decode(CURATION_ENCODING_VERSION, &unproposed).unwrap();
    assert_eq!(
        decoded.node("g").unwrap().kind(),
        &CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default())
    );
}

/// A graph with two outputs over whole episodes: longer and shorter ones.
fn two_outputs() -> (CurationGraph, String, String) {
    let mut payload = CurationGraph::new(None);
    let rule =
        |comparison| Selection::episodes_where(EpisodeFact::DurationSeconds, comparison, 10.0);
    let long = payload
        .add_keeping(
            DATASET_NODE_ID,
            CurationNodeKind::Select,
            rule(Comparison::AtLeast),
        )
        .unwrap();
    let short = payload
        .add_keeping(
            DATASET_NODE_ID,
            CurationNodeKind::Select,
            rule(Comparison::AtMost),
        )
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
fn a_publication_an_older_studio_bounded_to_its_first_episodes_still_reads() {
    let (mut payload, long, _) = two_outputs();
    payload
        .record_publication(&long, "long_episodes", 3)
        .unwrap();
    let mut encoded: serde_json::Value = serde_json::from_str(&payload.encode().unwrap()).unwrap();
    encoded["published"][0]["episodes"] = serde_json::json!(20);
    let recorded = CurationGraph::decode(CURATION_ENCODING_VERSION, &encoded.to_string()).unwrap();
    let (published, node) = recorded.publications().next().unwrap();
    assert_eq!(
        (node.id(), published.view()),
        (long.as_str(), "long_episodes")
    );
    assert!(!recorded.encode().unwrap().contains("episodes\":20"));
}

#[test]
fn a_copy_unpublished_elsewhere_differs_by_its_publications_alone() {
    let (mut open, long, short) = two_outputs();
    open.record_publication(&long, "long_episodes", 3).unwrap();
    open.record_publication(&short, "short_episodes", 4)
        .unwrap();
    let shortest = Selection::episodes_where(EpisodeFact::DurationSeconds, Comparison::AtMost, 5.0);
    // Data unpublished one view and stored the curation.
    let mut stored = open.clone();
    stored.remove_publication(&long, 5).unwrap();
    assert!(open.same_recipe(&stored));
    open.take_publications(&stored);
    assert_eq!(open, stored);
    // A recipe edited elsewhere is another writer's change.
    let mut edited = stored.clone();
    edited
        .add_keeping(DATASET_NODE_ID, CurationNodeKind::Select, shortest)
        .unwrap();
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
fn a_published_output_keeps_its_branch_until_it_is_unpublished() {
    let (mut payload, long, _) = two_outputs();
    payload
        .record_publication(&long, "long_episodes", 1)
        .unwrap();
    // A step after it leaves it an output, published as it was.
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
