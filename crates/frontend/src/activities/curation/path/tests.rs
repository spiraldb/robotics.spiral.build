use super::*;
use crate::activities::curation::document::{
    CategoryId, Comparison, DATASET_NODE_ID, EpisodeCondition, EpisodeFact, EpisodeNumber,
    ObjectTracking, ProgressScoring, SelectCondition, Selection, SubtaskAnnotation,
    SubtaskCategory, SubtaskGrouping, SubtaskSearch,
};
use crate::activities::curation::score_progress::ProgressScorer;
use spiral_robotics_common::curation::{AnswerSource, CategoryDefinition, FillEpisodes};
use spiral_robotics_common::datasets::EpisodeTask;
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};

fn graph() -> CurationGraph {
    CurationGraph::new(Some(
        crate::activities::episode::EpisodeListing::mcap_fixture("abc130k"),
    ))
}

fn child(graph: &mut CurationGraph, parent: &str, kind: CurationNodeKind) -> String {
    graph
        .add_child(parent, kind)
        .expect("the grammar allows it")
}

/// A Select after `parent` keeping by `conditions`.
fn selecting(graph: &mut CurationGraph, parent: &str, conditions: Vec<SelectCondition>) -> String {
    let select = child(
        graph,
        parent,
        CurationNodeKind::Select(Selection::keeping_everything()),
    );
    graph
        .set_selection(&select, Selection::of(conditions))
        .expect("conditions on the nodes above");
    select
}

fn lasting(comparison: Comparison, seconds: f64) -> SelectCondition {
    SelectCondition::Episodes(EpisodeCondition::new(
        EpisodeNumber::Fact(EpisodeFact::DurationSeconds),
        comparison,
        seconds,
    ))
}

fn published(graph: &CurationGraph, node: &str) -> String {
    CurationPath::to(graph, node).unwrap().statement().unwrap()
}

/// What `node`'s preview of two listed episodes reads.
fn preview(graph: &CurationGraph, node: &str) -> String {
    CurationPath::to(graph, node).unwrap().preview(2).unwrap()
}

/// A Break down node on the top camera of a dataset that records the
/// operator's instruction.
fn breakdown() -> CurationNodeKind {
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".to_owned()));
    annotation.set_task(Some(EpisodeTask::Recorded("/instruction".to_owned())));
    CurationNodeKind::BreakDownSubtasks(annotation)
}

fn searching(query: &str) -> CurationNodeKind {
    let mut search = SubtaskSearch::default();
    search.set_query(query);
    CurationNodeKind::FindSimilarSubtasks(search)
}

/// A grouping by `names`, numbered in their order.
fn grouping(names: &[&str]) -> CurationNodeKind {
    CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::of(
        names
            .iter()
            .map(|name| {
                SubtaskCategory::new(*name, format!("The robot {name}s the cloth.")).unwrap()
            })
            .collect(),
    ))
}

/// A Track objects node tracking bottles on the top camera.
fn tracking_bottles() -> CurationNodeKind {
    let mut tracking = ObjectTracking::default();
    tracking.set_camera(Some("/top-camera".to_owned()));
    tracking.set_prompt("bottle");
    CurationNodeKind::TrackObjects(tracking)
}

/// Both readings pass the read-only gate Publish and the preview admit by.
fn admitted(published: &str, preview: &str) {
    assert_eq!(
        ReadOnlyQuery::new(published, ReadOnlyQueryMode::Curation).authorize(),
        Ok(())
    );
    assert_eq!(
        ReadOnlyQuery::new(preview, ReadOnlyQueryMode::Preview).authorize(),
        Ok(())
    );
}

/// The column a node's call stores its answers in.
fn column(graph: &CurationGraph, node: &str) -> String {
    CurationPath::to(graph, node)
        .unwrap()
        .answer_column()
        .unwrap()
        .to_string()
}

const ANSWERS: &str = "\"datasets\".\"abc130k\".\"answers\"";

#[test]
fn a_select_on_duration_keeps_whole_episodes_and_publishes_the_dataset_table() {
    let mut graph = graph();
    let long = selecting(
        &mut graph,
        DATASET_NODE_ID,
        vec![lasting(Comparison::AtLeast, 10.0)],
    );
    // The view reads every episode the Select keeps, and asks no model.
    assert_eq!(
        published(&graph, &long),
        "WITH
  dataset AS (SELECT episode_id, start_ns AS origin_ns, [STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)] AS kept, duration_seconds, messages, channels, size_megabytes FROM (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, CAST(message_start_time + duration_ns AS BIGINT) AS end_ns, CAST(duration_ns AS DOUBLE) / 1e9 AS duration_seconds, message_count AS messages, channel_count AS channels, CAST(source_bytes AS DOUBLE) / 1e6 AS size_megabytes FROM (SELECT id, source_uri, source_bytes, message_count, message_start_time, duration_ns, channel_count FROM \"datasets\".\"abc130k\".\"episodes\") listed) spans),
  select_1_numbers AS (SELECT r.episode_id, r.kept, d.duration_seconds AS number_0 FROM dataset r JOIN dataset d ON d.episode_id = r.episode_id),
  select_1 AS (SELECT episode_id, kept FROM select_1_numbers WHERE number_0 >= CAST(10 AS DOUBLE))
SELECT r.episode_id, r.kept, d.duration_seconds, d.messages, d.channels, d.size_megabytes FROM select_1 r JOIN dataset d ON d.episode_id = r.episode_id"
    );
    let path = CurationPath::to(&graph, &long).unwrap();
    assert!(path.fills().unwrap().is_empty(), "nothing to store");
    let names = path
        .table()
        .unwrap()
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["duration_seconds", "messages", "channels", "size_megabytes"]
    );
}

/// The demo: Dataset → Track objects ("bottle") → Select keeping the
/// lowest 25% of episodes by duration per bottle.
#[test]
fn the_fastest_quarter_by_duration_per_bottle_ranks_every_reaching_episode() {
    let mut graph = graph();
    let track = child(&mut graph, DATASET_NODE_ID, tracking_bottles());
    let fastest = selecting(
        &mut graph,
        &track,
        vec![SelectCondition::Episodes(EpisodeCondition::new(
            EpisodeNumber::DurationPer(track.clone()),
            Comparison::Lowest,
            0.25,
        ))],
    );
    let tracks = column(&graph, &track);
    let published = published(&graph, &fastest);
    for step in [
        format!(
            "episodes AS (SELECT d.episode_id, d.origin_ns, a.{tracks}.value AS {tracks} FROM dataset d JOIN {ANSWERS} a ON a.episode_id = d.episode_id)"
        ),
        // The tracks, a picture at a time in log time, folded per episode.
        format!(
            "track_objects_1_entries AS (SELECT r.episode_id, r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS timestamp_ns, p.objects AS objects FROM episodes r CROSS JOIN LATERAL UNNEST(r.{tracks}) p)"
        ),
        "track_objects_1 AS (SELECT episode_id, spql.list.collect(STRUCT_PACK(timestamp_ns := timestamp_ns, objects := objects)) AS track_objects_1 FROM (SELECT * FROM track_objects_1_entries ORDER BY episode_id, timestamp_ns) ordered GROUP BY episode_id)".to_owned(),
        // Each reaching episode's duration per the most bottles one picture held.
        "select_1_numbers AS (SELECT r.episode_id, r.kept, CASE WHEN m0.most > 0 THEN d.duration_seconds / CAST(m0.most AS DOUBLE) END AS number_0 FROM dataset r JOIN dataset d ON d.episode_id = r.episode_id LEFT JOIN (SELECT episode_id, MAX(spql.list.len(objects)) AS most FROM track_objects_1_entries GROUP BY episode_id) m0 ON m0.episode_id = r.episode_id)".to_owned(),
        "select_1_ranked AS (SELECT episode_id, kept, number_0, CAST(RANK() OVER (PARTITION BY number_0 IS NULL ORDER BY number_0) - 1 AS DOUBLE) / CAST(GREATEST(COUNT(number_0) OVER (PARTITION BY number_0 IS NULL), 2) - 1 AS DOUBLE) AS rank_0 FROM select_1_numbers)".to_owned(),
        "select_1 AS (SELECT episode_id, kept FROM select_1_ranked WHERE number_0 IS NOT NULL AND rank_0 <= CAST(0.25 AS DOUBLE))".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    assert!(published.ends_with(
        "\nSELECT r.episode_id, r.kept, d.duration_seconds, d.messages, d.channels, d.size_megabytes, track_objects_1.track_objects_1 FROM select_1 r JOIN dataset d ON d.episode_id = r.episode_id LEFT JOIN track_objects_1 ON track_objects_1.episode_id = r.episode_id"
    ));
    // The tracks are filled for every episode, at the node's rate, on the
    // Modal service; the Select asks nothing.
    let fills = CurationPath::to(&graph, &fastest).unwrap().fills().unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(
        fills[0].source(),
        &AnswerSource::ObjectTracking {
            camera: "/top-camera".to_owned(),
            prompt: "bottle".to_owned(),
            rate: 5,
            model: "facebook/sam3.1@daa63191845a41281374e725f4c9e51c7a824460".to_owned(),
            revision: "object-tracking-0.2.0".to_owned(),
        }
    );
    let FillEpisodes::Reaching(reaching) = fills[0].episodes() else {
        panic!("a published fill asks what reaches the node")
    };
    assert!(
        reaching.ends_with("\nSELECT episode_id FROM dataset"),
        "{reaching}"
    );
    // The preview reads how many bottles each picture of the listed
    // episodes held, never the boxes.
    let counting = preview(&graph, &track);
    assert!(counting.contains(&format!(
        "episodes AS (SELECT e.id AS episode_id, CAST(e.message_start_time AS BIGINT) AS origin_ns, a.{tracks}.value AS {tracks} FROM \"datasets\".\"abc130k\".\"episodes\" e JOIN {ANSWERS} a ON a.episode_id = e.id WHERE e.id IN ($1, $2))"
    )));
    assert!(counting.ends_with(
        "\nSELECT episode_id, timestamp_ns, spql.list.len(objects) AS value FROM track_objects_1_entries"
    ));
    let path = CurationPath::to(&graph, &track).unwrap();
    assert!(path.preview(0).is_err(), "a preview lists an episode");
    admitted(&published, &counting);
    // The rate is part of the call: another rate is another column.
    if let CurationNodeKind::TrackObjects(tracking) = graph.node_mut(&track).unwrap().kind_mut() {
        tracking.set_rate(2).unwrap();
    }
    assert_ne!(column(&graph, &track), tracks);
}

#[test]
fn a_progress_rate_narrows_kept_to_the_stretches_progressing_fast_enough() {
    let mut graph = graph();
    let score = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let moving = selecting(
        &mut graph,
        &score,
        vec![SelectCondition::Progressing {
            node: score.clone(),
            minimum: 0.2,
        }],
    );
    let scores = column(&graph, &score);
    let published = published(&graph, &moving);
    for step in [
        format!(
            "score_progress_1_entries AS (SELECT r.episode_id, r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS timestamp_ns, CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS progress_rate FROM episodes r CROSS JOIN LATERAL UNNEST(r.{scores}) p)"
        ),
        "score_progress_1 AS (SELECT episode_id, spql.list.collect(STRUCT_PACK(timestamp_ns := timestamp_ns, progress_rate := progress_rate)) AS score_progress_1 FROM (SELECT * FROM score_progress_1_entries ORDER BY episode_id, timestamp_ns) ordered GROUP BY episode_id)".to_owned(),
        // No episode condition: the Select's input is the dataset's reach.
        "select_1_unnested AS (SELECT p.episode_id, k.start_ns, k.end_ns FROM dataset p CROSS JOIN LATERAL UNNEST(p.kept) k)".to_owned(),
        "select_1_within_0_moments AS (SELECT episode_id, timestamp_ns AS start_ns, COALESCE(LEAD(timestamp_ns) OVER (PARTITION BY episode_id ORDER BY timestamp_ns), timestamp_ns + (timestamp_ns - LAG(timestamp_ns) OVER (PARTITION BY episode_id ORDER BY timestamp_ns))) AS end_ns, progress_rate AS value, ROW_NUMBER() OVER (PARTITION BY episode_id ORDER BY timestamp_ns) AS position FROM score_progress_1_entries)".to_owned(),
        "FROM select_1_within_0_moments WHERE end_ns > start_ns AND (value IS NULL OR value >= CAST(0.2 AS DOUBLE)))".to_owned(),
        "select_1_within_0 AS (SELECT episode_id, MIN(start_ns) AS start_ns, MAX(end_ns) AS end_ns FROM select_1_within_0_passing GROUP BY episode_id, stretch)".to_owned(),
        "select_1_kept_0 AS (SELECT k.episode_id, GREATEST(k.start_ns, w.start_ns) AS start_ns, LEAST(k.end_ns, w.end_ns) AS end_ns FROM select_1_unnested k JOIN select_1_within_0 w ON w.episode_id = k.episode_id AND w.start_ns < k.end_ns AND w.end_ns > k.start_ns)".to_owned(),
        // Ranges that overlap or touch fold into one.
        "select_1_islands AS (SELECT episode_id, start_ns, end_ns, SUM(opens) OVER (PARTITION BY episode_id ORDER BY start_ns, end_ns ROWS UNBOUNDED PRECEDING) AS island FROM (SELECT episode_id, start_ns, end_ns, CASE WHEN start_ns <= MAX(end_ns) OVER (PARTITION BY episode_id ORDER BY start_ns, end_ns ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) THEN 0 ELSE 1 END AS opens FROM select_1_kept_0) flagged)".to_owned(),
        "select_1 AS (SELECT episode_id, spql.list.collect(STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)) AS kept FROM (SELECT episode_id, MIN(start_ns) AS start_ns, MAX(end_ns) AS end_ns FROM select_1_islands GROUP BY episode_id, island ORDER BY episode_id, start_ns) ordered GROUP BY episode_id)".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    // The preview reads the listed episodes' frames and their rates.
    let scoring = preview(&graph, &score);
    assert!(scoring.ends_with(
        "\nSELECT episode_id, timestamp_ns, progress_rate AS value FROM score_progress_1_entries"
    ));
    assert!(!scoring.contains("\"messages\""));
    admitted(&published, &scoring);
    let fills = CurationPath::to(&graph, &moving).unwrap().fills().unwrap();
    assert_eq!(
        fills[0].source(),
        &AnswerSource::ProgressService {
            camera: ProgressScorer::Released.camera().to_owned(),
            model: "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
                .to_owned(),
            revision: "warp-progress-0.2.0-fp16".to_owned(),
        }
    );
}

#[test]
fn each_call_names_the_column_it_is_stored_in() {
    let mut graph = graph();
    let score = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let annotate = child(&mut graph, &score, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the towel"));
    let group = child(&mut graph, &annotate, grouping(&["fold", "grasp"]));
    // A call always hashes to the same column, so what it stored is read
    // without asking again.
    assert_eq!(
        [&score, &annotate, &search, &group].map(|node| column(&graph, node)),
        [
            "score_progress_67aae2",
            "break_down_2c3345",
            "similarity_9f8877",
            "subtask_category_be7603"
        ]
    );
}

#[test]
fn a_breakdown_reads_each_episode_whole_and_lists_its_subtasks_in_log_time() {
    let mut graph = graph();
    let long = selecting(
        &mut graph,
        DATASET_NODE_ID,
        vec![lasting(Comparison::AtLeast, 10.0)],
    );
    let annotate = child(&mut graph, &long, breakdown());
    let subtasks = column(&graph, &annotate);
    assert!(subtasks.starts_with("break_down_"));
    let published = published(&graph, &annotate);
    for step in [
        format!(
            "break_down_subtasks_1_listed AS (SELECT r.episode_id, r.origin_ns, g._ AS subtask_index, spql.list.get(r.{subtasks}, g._) AS listed FROM episodes r CROSS JOIN LATERAL spql.range(CAST(spql.list.len(r.{subtasks}) AS BIGINT)) g)"
        ),
        "break_down_subtasks_1_entries AS (SELECT l.episode_id, l.subtask_index, l.listed.description AS description, l.origin_ns + CAST(l.listed.start_ns AS BIGINT) AS start_ns, l.origin_ns + CAST(l.listed.end_ns AS BIGINT) AS end_ns FROM break_down_subtasks_1_listed l)".to_owned(),
        "spql.list.collect(STRUCT_PACK(description := description, start_ns := start_ns, end_ns := end_ns)) AS break_down_subtasks_1 FROM (SELECT * FROM break_down_subtasks_1_entries ORDER BY episode_id, subtask_index) ordered GROUP BY episode_id)".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    // A model node keeps nothing, so its view is what reaches it.
    assert!(published.contains(" FROM select_1 r JOIN dataset d"));
    assert!(!published.contains("\"messages\""));
    // Its fill asks the model about each episode the Select keeps whole,
    // from the chosen camera, with its first instruction as the task.
    let fills = CurationPath::to(&graph, &annotate)
        .unwrap()
        .fills()
        .unwrap();
    let AnswerSource::BreakDown(call) = fills[0].source() else {
        panic!("Break down is filled by its model call")
    };
    assert_eq!(
        (
            call.camera(),
            call.sample_interval().seconds(),
            call.model()
        ),
        ("/top-camera", 0.5, "google/gemini-3.8-flash")
    );
    assert_eq!(
        call.task(),
        Some(&EpisodeTask::Recorded("/instruction".to_owned()))
    );
    let FillEpisodes::Reaching(reaching) = fills[0].episodes() else {
        panic!("a published fill asks what reaches the node")
    };
    assert!(reaching.ends_with("\nSELECT episode_id FROM select_1"));
    // The preview reads the listed episodes' stored subtasks.
    let breaking = preview(&graph, &annotate);
    assert!(breaking.ends_with(
        "\nSELECT episode_id, description, start_ns, end_ns FROM break_down_subtasks_1_entries"
    ));
    assert!(!breaking.contains("select_1"));
    admitted(&published, &breaking);

    // A dataset that records no instruction asks without a task, and that
    // is another call, stored in another column.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_task(None);
    }
    assert_ne!(column(&graph, &annotate), subtasks);
}

#[test]
fn a_search_lists_every_subtask_with_its_similarity_and_a_select_keeps_the_close_ones() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the towel's edge"));
    let close = selecting(
        &mut graph,
        &search,
        vec![SelectCondition::Similar {
            node: search.clone(),
            minimum: 0.7,
        }],
    );
    let (subtasks, similarities) = (column(&graph, &annotate), column(&graph, &search));
    let published = published(&graph, &close);
    for step in [
        format!(
            "find_similar_subtasks_1_listed AS (SELECT r.episode_id, r.origin_ns, g._ AS subtask_index, spql.list.get(r.{subtasks}, g._) AS listed, spql.list.get(r.{similarities}, g._) AS judgment FROM episodes r CROSS JOIN LATERAL spql.range(CAST(spql.list.len(r.{subtasks}) AS BIGINT)) g)"
        ),
        ", l.judgment AS similarity FROM find_similar_subtasks_1_listed l)".to_owned(),
        "spql.list.collect(STRUCT_PACK(start_ns := start_ns, end_ns := end_ns, similarity := similarity)) AS find_similar_subtasks_1".to_owned(),
        "select_1_within_0 AS (SELECT episode_id, start_ns, end_ns FROM find_similar_subtasks_1_entries WHERE similarity >= CAST(0.7 AS DOUBLE))".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    // The stored subtasks and similarities are each read once on the row.
    for stored in [&subtasks, &similarities] {
        assert_eq!(published.matches(&format!("a.{stored}.value")).count(), 1);
    }
    assert!(published.ends_with(
        ", break_down_subtasks_1.break_down_subtasks_1, find_similar_subtasks_1.find_similar_subtasks_1 FROM select_1 r JOIN dataset d ON d.episode_id = r.episode_id LEFT JOIN break_down_subtasks_1 ON break_down_subtasks_1.episode_id = r.episode_id LEFT JOIN find_similar_subtasks_1 ON find_similar_subtasks_1.episode_id = r.episode_id"
    ));
    // Its fill judges each subtask the Break down stored for an episode.
    let fills = CurationPath::to(&graph, &close).unwrap().fills().unwrap();
    let AnswerSource::Similarity(call) = fills[1].source() else {
        panic!("Find similar is filled by its model call")
    };
    assert_eq!(call.subtasks().as_str(), subtasks);
    assert_eq!(
        (call.query(), call.model()),
        ("fold the towel's edge", "typesafe/jev-1.13")
    );
    let asking = preview(&graph, &search);
    assert!(asking.ends_with(
        "\nSELECT episode_id, description, start_ns, end_ns, similarity FROM find_similar_subtasks_1_entries"
    ));
    admitted(&published, &asking);
}

/// The example the plan names: fold subtasks, grouped from Break down.
#[test]
fn a_grouping_writes_a_column_per_category_and_a_select_keeps_the_chosen_ones() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let group = child(&mut graph, &annotate, grouping(&["fold", "flatten"]));
    let fold = serde_json::from_value::<CategoryId>(serde_json::json!(1)).unwrap();
    let folds = selecting(
        &mut graph,
        &group,
        vec![SelectCondition::InCategories {
            node: group.clone(),
            categories: vec![fold],
        }],
    );
    let (subtasks, categories) = (column(&graph, &annotate), column(&graph, &group));
    assert!(categories.starts_with("subtask_category_"));
    let published = published(&graph, &folds);
    for step in [
        ", l.judgment.category AS category, l.judgment.probability AS probability FROM group_similar_subtasks_1_listed l)".to_owned(),
        "group_similar_subtasks_1 AS (SELECT episode_id, spql.list.collect(STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)) FILTER (WHERE category = 'fold') AS group_similar_subtasks_1_category_1, spql.list.collect(STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)) FILTER (WHERE category = 'flatten') AS group_similar_subtasks_1_category_2 FROM (SELECT * FROM group_similar_subtasks_1_entries ORDER BY episode_id, subtask_index) ordered GROUP BY episode_id)".to_owned(),
        "select_1_within_0 AS (SELECT episode_id, start_ns, end_ns FROM group_similar_subtasks_1_entries WHERE category IN ('fold'))".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    // Its fill classifies each subtask the Break down stored.
    let fills = CurationPath::to(&graph, &folds).unwrap().fills().unwrap();
    let AnswerSource::Classify(call) = fills[1].source() else {
        panic!("Group similar is filled by its model call")
    };
    assert_eq!(call.subtasks().as_str(), subtasks);
    assert_eq!(
        call.categories(),
        [
            CategoryDefinition::new("fold", "The robot folds the cloth."),
            CategoryDefinition::new("flatten", "The robot flattens the cloth."),
        ]
    );
    assert_eq!(call.model(), "typesafe/jev-1.13");
    let table = CurationPath::to(&graph, &folds).unwrap().table().unwrap();
    assert_eq!(
        table
            .columns()
            .iter()
            .filter(|column| column.node() == group)
            .map(|column| (column.name().to_owned(), column.meaning()))
            .collect::<Vec<_>>(),
        [
            (
                "group_similar_subtasks_1_category_1".to_owned(),
                ColumnMeaning::Category(fold)
            ),
            (
                "group_similar_subtasks_1_category_2".to_owned(),
                ColumnMeaning::Category(serde_json::from_value(serde_json::json!(2)).unwrap())
            ),
        ]
    );
    let grouping = preview(&graph, &group);
    assert!(grouping.ends_with(
        "\nSELECT episode_id, description, start_ns, end_ns, category, probability FROM group_similar_subtasks_1_entries"
    ));
    admitted(&published, &grouping);
}

#[test]
fn one_call_shares_its_column_across_curations_and_another_call_does_not() {
    let (mut one, mut other) = (graph(), graph());
    let annotated = [
        child(&mut one, DATASET_NODE_ID, breakdown()),
        child(&mut other, DATASET_NODE_ID, breakdown()),
    ];
    assert_eq!(
        column(&one, &annotated[0]),
        column(&other, &annotated[1]),
        "the same call is asked once"
    );
    let searches = [
        child(&mut one, &annotated[0], searching("fold it")),
        child(&mut other, &annotated[1], searching("fold it")),
    ];
    assert_eq!(column(&one, &searches[0]), column(&other, &searches[1]));
    // A search of other subtasks is another call.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        other.node_mut(&annotated[1]).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/wrist-camera".to_owned()));
    }
    assert_ne!(column(&one, &annotated[0]), column(&other, &annotated[1]));
    assert_ne!(column(&one, &searches[0]), column(&other, &searches[1]));
}

#[test]
fn a_node_missing_what_it_needs_says_so() {
    let mut graph = graph();
    let annotate = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
    );
    let group = child(&mut graph, &annotate, grouping(&[]));
    let search = child(&mut graph, &annotate, searching("  "));
    let reason = |graph: &CurationGraph, node: &str| {
        CurationPath::to(graph, node)
            .unwrap()
            .statement()
            .unwrap_err()
            .message()
            .to_owned()
    };
    assert_eq!(
        reason(&graph, &annotate),
        "Choose the camera Break down reads."
    );
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/top-camera".to_owned()));
    }
    assert_eq!(
        reason(&graph, &group),
        "Generate the categories, or write them, before grouping."
    );
    assert_eq!(reason(&graph, &search), "Type the subtask to search for.");
    // A Select choosing no category has nothing to keep.
    graph
        .propose_categories(
            &group,
            vec![SubtaskCategory::new("fold", "Folding.").unwrap()],
        )
        .unwrap();
    let none = selecting(
        &mut graph,
        &group,
        vec![SelectCondition::InCategories {
            node: group.clone(),
            categories: Vec::new(),
        }],
    );
    assert_eq!(
        reason(&graph, &none),
        "Choose at least one category: this Select keeps none."
    );
    let mut unnamed = ObjectTracking::default();
    unnamed.set_camera(Some("/top-camera".to_owned()));
    let track = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::TrackObjects(unnamed),
    );
    assert!(reason(&graph, &track).contains("Name what to track"));
}

#[test]
fn a_graph_without_a_dataset_has_nothing_to_read() {
    let mut graph = CurationGraph::new(None);
    let long = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::Select(Selection::keeping_everything()),
    );
    assert!(matches!(
        CurationPath::to(&graph, &long).unwrap().statement(),
        Err(PathError(_))
    ));
}

/// A compiled statement read back as its `WITH` entries, by name, and the
/// final `SELECT`, for counting where each entry is read.
struct CompiledEntries {
    entries: Vec<(String, String)>,
    select: String,
}

impl CompiledEntries {
    fn of(statement: &str) -> Self {
        let body = statement
            .strip_prefix("WITH\n  ")
            .expect("a WITH statement");
        let (with, select) = body.rsplit_once("\nSELECT ").expect("a final SELECT");
        // Entries end at a comma outside every parenthesis, bracket, and
        // quote.
        let (mut entries, mut depth, mut quoted, mut start) = (Vec::new(), 0, false, 0);
        for (index, character) in with.char_indices() {
            match character {
                '\'' => quoted = !quoted,
                '(' | '[' if !quoted => depth += 1,
                ')' | ']' if !quoted => depth -= 1,
                ',' if !quoted && depth == 0 => {
                    entries.push(with[start..index].trim());
                    start = index + 1;
                }
                _ => {}
            }
        }
        entries.push(with[start..].trim());
        let entries = entries
            .into_iter()
            .map(|entry| {
                let name = entry
                    .split(|character: char| {
                        !(character.is_ascii_alphanumeric() || character == '_')
                    })
                    .next()
                    .expect("a named entry");
                (name.to_owned(), entry.to_owned())
            })
            .collect();
        Self {
            entries,
            select: select.to_owned(),
        }
    }

    /// How many times `name` is read, as the relation of a `FROM` or a
    /// `JOIN`, anywhere in the statement.
    fn reads(&self, name: &str) -> usize {
        self.entries
            .iter()
            .map(|(_, entry)| Self::reads_within(entry, name))
            .sum::<usize>()
            + Self::reads_within(&self.select, name)
    }

    fn reads_within(text: &str, name: &str) -> usize {
        ["FROM ", "JOIN "]
            .iter()
            .map(|keyword| {
                let read = format!("{keyword}{name}");
                text.match_indices(&read)
                    .filter(|(at, _)| {
                        text[at + read.len()..]
                            .chars()
                            .next()
                            .is_none_or(|next| !(next.is_ascii_alphanumeric() || next == '_'))
                    })
                    .count()
            })
            .sum()
    }
}

/// Every kind the grammar offers after a node, configured to compile: a
/// Select reading the nearest node above it that it can read, or the
/// dataset's duration.
fn configured(graph: &CurationGraph, parent: &str, kind: CurationNodeKind) -> CurationNodeKind {
    match kind {
        CurationNodeKind::BreakDownSubtasks(_) => breakdown(),
        CurationNodeKind::FindSimilarSubtasks(_) => searching("fold the towel"),
        CurationNodeKind::GroupSimilarSubtasks(_) => grouping(&["fold", "grasp"]),
        CurationNodeKind::TrackObjects(_) => tracking_bottles(),
        CurationNodeKind::Select(_) => {
            let mut graph = graph.clone();
            let probe = graph
                .add_child(
                    parent,
                    CurationNodeKind::Select(Selection::keeping_everything()),
                )
                .expect("a Select follows anything");
            let condition = graph
                .conditions_for(&probe)
                .pop()
                .map(|choice| choice.condition().clone())
                .expect("the facts at least");
            CurationNodeKind::Select(Selection::of(vec![condition]))
        }
        kind => kind,
    }
}

/// Every path of up to `below` nodes under `node` the grammar allows, each
/// checked by `check` once it is in `graph`.
fn every_path(
    graph: &mut CurationGraph,
    (node, below): (&str, usize),
    check: &mut impl FnMut(&CurationGraph, &str),
) {
    if below == 0 {
        return;
    }
    for kind in graph.child_kinds(node) {
        let kind = configured(graph, node, kind);
        let added = child(graph, node, kind);
        check(graph, &added);
        every_path(graph, (&added, below - 1), check);
        graph.remove_node(&added).unwrap();
    }
}

/// No published path asks a model: every node that asks one reads its
/// stored column on the episode row once, the reach chain reads each of its
/// steps once, and a fill writes each column in path order, finding the
/// episodes that reach it over the columns above.
#[test]
fn no_published_path_asks_a_model_and_each_fill_reads_only_what_is_filled_before_it() {
    let mut graph = graph();
    let mut paths = 0;
    every_path(&mut graph, (DATASET_NODE_ID, 5), &mut |graph, node| {
        paths += 1;
        let statement = published(graph, node);
        let path = CurationPath::to(graph, node).unwrap();
        let fills = path.fills().unwrap();
        assert!(statement.matches(ANSWERS).count() <= 1, "{statement}");
        let compiled = CompiledEntries::of(&statement);
        // The reach chain: every Select reads the reach above it once.
        for select in path
            .nodes
            .iter()
            .filter(|node| matches!(node.kind(), CurationNodeKind::Select(_)))
        {
            assert_eq!(compiled.reads(&select.stem()), 1, "{statement}");
        }
        for (filled, fill) in fills.iter().enumerate() {
            assert_eq!(
                statement
                    .matches(&format!("a.{}.value", fill.column()))
                    .count(),
                1,
                "{statement}"
            );
            let FillEpisodes::Reaching(reaching) = fill.episodes() else {
                panic!("a published fill asks what reaches the node")
            };
            let above = &fills[..filled];
            for later in &fills[filled..] {
                if !above.iter().any(|fill| fill.column() == later.column()) {
                    assert!(!reaching.contains(later.column().as_str()), "{reaching}");
                }
            }
        }
        let models = path
            .nodes
            .iter()
            .filter(|node| node.kind().is_model())
            .count();
        assert_eq!(fills.len(), models);
        assert_eq!(
            ReadOnlyQuery::new(&statement, ReadOnlyQueryMode::Curation).authorize(),
            Ok(())
        );
    });
    assert!(paths > 500, "{paths} paths");
}
