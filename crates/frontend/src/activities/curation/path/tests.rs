use super::*;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{
    Comparison, DATASET_NODE_ID, EpisodeFact, MomentDetection, Selection, SubtaskAnnotation,
    SubtaskCategory, SubtaskGrouping, SubtaskSearch,
};
use crate::activities::curation::moments::Moment;
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::preview::{PreviewReads, PreviewRows, ReadInput};
use crate::activities::episode::Episode;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};

fn graph() -> CurationGraph {
    CurationGraph::new(Some(
        DatasetSelection::new(vec!["abc130k".to_owned()]).unwrap(),
    ))
}

fn child(graph: &mut CurationGraph, parent: &str, kind: CurationNodeKind) -> String {
    graph
        .add_child(parent, kind)
        .expect("the grammar allows it")
}

fn published(graph: &CurationGraph, node: &str) -> String {
    CurationPath::to(graph, node).unwrap().statement().unwrap()
}

/// `node`'s preview over `input`, as its parent's preview hands it on.
fn preview(graph: &CurationGraph, node: &str, input: StepInput) -> String {
    CurationPath::to(graph, node)
        .unwrap()
        .preview(input)
        .unwrap()
}

/// The intervals of the bound episode that reach a node that reads the
/// recording.
fn intervals() -> StepInput {
    StepInput::new(
        vec!["preview_input(episode_id, start_ns, end_ns) AS (VALUES ($1, CAST(5 AS BIGINT), CAST(9 AS BIGINT)))".to_owned()],
        Relation::new("preview_input", Vec::new()),
    )
}

/// Rows standing in for what `parent`'s preview holds, in the columns its
/// relation carries.
fn rows_of(graph: &CurationGraph, parent: &str) -> StepInput {
    let relation = CurationPath::to(graph, parent)
        .unwrap()
        .relation()
        .unwrap()
        .renamed("preview_input");
    StepInput::new(
        vec!["preview_input AS (SELECT * FROM parent_rows)".to_owned()],
        relation,
    )
}

/// A Break down node on the top camera of a dataset that records the
/// operator's instruction.
fn breakdown() -> CurationNodeKind {
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".to_owned()));
    annotation.set_task_topic(Some("/instruction".to_owned()));
    CurationNodeKind::BreakDownSubtasks(annotation)
}

fn searching(query: &str) -> CurationNodeKind {
    let mut search = SubtaskSearch::default();
    search.set_query(query);
    CurationNodeKind::FindSimilarSubtasks(search)
}

fn grouping(names: &[&str]) -> CurationNodeKind {
    let mut grouping = SubtaskGrouping::default();
    grouping
        .set_categories(
            names
                .iter()
                .map(|name| {
                    SubtaskCategory::new(*name, format!("The robot {name}s the cloth.")).unwrap()
                })
                .collect(),
        )
        .unwrap();
    CurationNodeKind::GroupSimilarSubtasks(grouping)
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

#[test]
fn a_select_on_duration_keeps_whole_episodes_and_publishes_the_fact_it_read() {
    let mut graph = graph();
    let long = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::Select(Selection::Episodes {
            fact: EpisodeFact::DurationSeconds,
            comparison: Comparison::AtLeast,
            value: 10.0,
        }),
    );
    assert_eq!(
        published(&graph, &long),
        "WITH
  dataset_0 AS (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, CAST(message_start_time + duration_ns AS BIGINT) AS end_ns, CAST(duration_ns AS DOUBLE) / 1e9 AS duration_seconds, message_count AS messages, channel_count AS channels, CAST(source_bytes AS DOUBLE) / 1e6 AS size_megabytes FROM (SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, message_start_time, duration_ns, channel_count FROM spql.mcap.episodes('abc130k')) listed),
  select_1 AS (SELECT episode_id, start_ns, end_ns, duration_seconds, messages, channels, size_megabytes FROM dataset_0 WHERE duration_seconds >= CAST(10 AS DOUBLE))
SELECT episode_id, start_ns, end_ns, duration_seconds FROM select_1"
    );
}

#[test]
fn a_progress_select_scores_every_frame_and_joins_the_moments_it_keeps() {
    let mut graph = graph();
    let detect = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
    );
    let moving = child(
        &mut graph,
        &detect,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let published = published(&graph, &moving);
    for step in [
        "FROM spql.mcap.read_topic('abc130k', '/cameras/top_camera-images-rgb') r",
        "WHERE r.id IN (SELECT episode_id FROM dataset_0))",
        "spql.ai.score_progress(source, model => 'uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f') AS scores FROM identify_unproductive_moments_1_videos)",
        "CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS pace FROM identify_unproductive_moments_1_scores s CROSS JOIN LATERAL UNNEST(s.scores) p)",
        "FROM identify_unproductive_moments_1_moments m JOIN dataset_0 i ON i.episode_id = m.episode_id",
        "FROM select_2_numbered WHERE (pace IS NULL OR pace >= CAST(0.2 AS DOUBLE)))",
        "select_2 AS (SELECT episode_id, MIN(start_ns) AS start_ns, MAX(end_ns) AS end_ns, AVG(pace) AS pace FROM select_2_kept GROUP BY episode_id, stretch)",
    ] {
        assert!(published.contains(step), "{step}\n{published}");
    }
    assert!(published.ends_with("\nSELECT episode_id, start_ns, end_ns, pace FROM select_2"));
    // Spiral admits only aggregates and ranking functions over a window.
    assert!(!published.contains("LAG(") && !published.contains("LEAD("));
    // The preview scores only the bound episode, over the intervals the
    // dataset's preview hands on rather than the dataset read again.
    let scoring = preview(&graph, &detect, intervals());
    assert!(
        scoring.starts_with("WITH\n  preview_input(episode_id, start_ns, end_ns) AS (VALUES ($1, ")
    );
    assert!(
        scoring.contains("WHERE r.id IN (SELECT episode_id FROM preview_input) AND r.id = $1)")
    );
    assert!(scoring.contains("JOIN preview_input i ON i.episode_id = m.episode_id"));
    assert!(!scoring.contains("dataset_0"));
    assert!(scoring.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, pace FROM identify_unproductive_moments_1"
    ));
    admitted(&published, &scoring);
}

#[test]
fn a_breakdown_reads_each_episode_whole_and_cuts_its_subtasks_to_what_reached_it() {
    let mut graph = graph();
    let long = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::Select(Selection::Episodes {
            fact: EpisodeFact::DurationSeconds,
            comparison: Comparison::AtLeast,
            value: 10.0,
        }),
    );
    let annotate = child(&mut graph, &long, breakdown());
    let published = published(&graph, &annotate);
    for step in [
        // The chosen camera's packets of the episodes that reach the node.
        "FROM spql.mcap.read_topic('abc130k', '/top-camera') r JOIN spql.mcap.episodes('abc130k') e ON e.id = r.id WHERE r.id IN (SELECT episode_id FROM select_1))",
        // Each episode's first instruction is its task.
        "break_down_subtasks_2_tasks AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time) AS position FROM spql.mcap.read_topic('abc130k', '/instruction') r WHERE r.id IN (SELECT episode_id FROM select_1)) instructions WHERE position = 1)",
        "break_down_subtasks_2_breakdowns AS (SELECT v.id AS episode_id, spql.ai.breakdown_task(v.source, sample_interval => 0.5, model => 'google/gemini-3.8-flash', task => t.task) AS subtasks FROM break_down_subtasks_2_videos v LEFT JOIN break_down_subtasks_2_tasks t ON t.id = v.id)",
        // Unnested straight from the call, before any join.
        "FROM break_down_subtasks_2_breakdowns b CROSS JOIN LATERAL UNNEST(b.subtasks) AS s)",
        // Presentation time counts from the episode's first message.
        "e.origin_ns + o.start_offset_ns AS start_ns, e.origin_ns + o.end_offset_ns AS end_ns",
        // Cut to the intervals that reached the node.
        "break_down_subtasks_2 AS (SELECT s.episode_id, GREATEST(s.start_ns, i.start_ns) AS start_ns, LEAST(s.end_ns, i.end_ns) AS end_ns, s.subtask FROM break_down_subtasks_2_subtasks s JOIN select_1 i ON i.episode_id = s.episode_id AND s.start_ns < i.end_ns AND s.end_ns > i.start_ns)",
    ] {
        assert!(published.contains(step), "{step}\n{published}");
    }
    assert!(
        published
            .ends_with("\nSELECT episode_id, start_ns, end_ns, subtask FROM break_down_subtasks_2")
    );
    // The preview reads the bound episode wherever a topic is read, cuts to
    // the intervals its parent's preview handed on, and fails an episode the
    // model cannot break down with the reason.
    let breaking = preview(&graph, &annotate, intervals());
    assert!(breaking.contains(
        "WHERE r.id IN (SELECT episode_id FROM preview_input) AND r.id = $1) instructions"
    ));
    assert!(breaking.contains("task => t.task, on_error => 'fail') AS subtasks"));
    assert!(breaking.contains("FROM break_down_subtasks_2_subtasks s JOIN preview_input i ON"));
    assert!(!breaking.contains("select_1") && !breaking.contains("dataset_0"));
    admitted(&published, &breaking);

    // A dataset that records no instruction asks without a task.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_task_topic(None);
    }
    let published = self::published(&graph, &annotate);
    assert!(!published.contains("_tasks"));
    assert!(published.contains(
        "model => 'google/gemini-3.8-flash') AS subtasks FROM break_down_subtasks_2_videos v)"
    ));
}

/// The example the plan names: fold subtasks, grouped from Break down.
#[test]
fn a_grouping_publishes_each_subtask_with_its_category_and_a_select_keeps_the_chosen() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let group = child(&mut graph, &annotate, grouping(&["fold", "flatten"]));
    let folds = child(
        &mut graph,
        &group,
        CurationNodeKind::Select(Selection::Categories {
            names: vec!["fold".into()],
        }),
    );
    let published = published(&graph, &folds);
    assert!(
        published.contains(
            "group_similar_subtasks_2 AS (SELECT episode_id, start_ns, end_ns, subtask, spql.ai.classify(subtask, [STRUCT_PACK(name := 'fold', definition := 'The robot folds the cloth.'), STRUCT_PACK(name := 'flatten', definition := 'The robot flattens the cloth.')], model => 'typesafe/jev-1.13') AS subtask_category FROM break_down_subtasks_1)"
        ),
        "{published}"
    );
    assert!(published.contains(
        "select_3 AS (SELECT episode_id, start_ns, end_ns, subtask, subtask_category FROM group_similar_subtasks_2 WHERE group_similar_subtasks_2.subtask_category.category IN ('fold'))"
    ));
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, subtask, subtask_category FROM select_3"
    ));
    // The preview judges the rows Break down's preview holds, and nothing
    // above it is asked again.
    let grouping = preview(&graph, &group, rows_of(&graph, &annotate));
    assert!(grouping.contains(
        "spql.ai.classify(subtask, [STRUCT_PACK(name := 'fold', definition := 'The robot folds the cloth.'), STRUCT_PACK(name := 'flatten', definition := 'The robot flattens the cloth.')], model => 'typesafe/jev-1.13', on_error => 'fail') AS subtask_category FROM preview_input)"
    ));
    assert!(!grouping.contains("breakdown_task"));
    assert!(grouping.ends_with("\nSELECT episode_id, start_ns, end_ns, subtask, subtask_category FROM group_similar_subtasks_2"));
    admitted(&published, &grouping);
}

#[test]
fn a_search_publishes_each_subtask_with_its_similarity_and_a_repeat_writes_its_own_column() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the towel's edge"));
    let close = child(
        &mut graph,
        &search,
        CurationNodeKind::Select(Selection::Similar { minimum: 0.5 }),
    );
    let published = published(&graph, &close);
    assert!(
        published.contains(
            "find_similar_subtasks_2 AS (SELECT episode_id, start_ns, end_ns, subtask, spql.ai.similarity(subtask, 'fold the towel''s edge', model => 'typesafe/jev-1.13') AS similarity FROM break_down_subtasks_1)"
        ),
        "{published}"
    );
    assert!(
        published.contains("FROM find_similar_subtasks_2 WHERE similarity >= CAST(0.5 AS DOUBLE))")
    );
    assert!(
        published
            .ends_with("\nSELECT episode_id, start_ns, end_ns, subtask, similarity FROM select_3")
    );
    let asking = preview(&graph, &search, rows_of(&graph, &annotate));
    assert!(asking.contains(
        "spql.ai.similarity(subtask, 'fold the towel''s edge', model => 'typesafe/jev-1.13', on_error => 'fail') AS similarity FROM preview_input)"
    ));
    admitted(&published, &asking);

    // A second search below the first keeps the first's column and writes
    // its own, which the Select below it reads.
    let again = child(&mut graph, &close, searching("smooth the cloth"));
    let closer = child(
        &mut graph,
        &again,
        CurationNodeKind::Select(Selection::Similar { minimum: 0.8 }),
    );
    let published = self::published(&graph, &closer);
    assert!(
        published.contains("AS similarity_2 FROM select_3)"),
        "{published}"
    );
    assert!(published.contains("WHERE similarity_2 >= CAST(0.8 AS DOUBLE))"));
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, subtask, similarity, similarity_2 FROM select_5"
    ));
    let path = CurationPath::to(&graph, &again).unwrap();
    assert_eq!(
        path.relation().unwrap().measured(SubtaskRow::SIMILARITY),
        Some("similarity_2")
    );
    // Its preview is handed both columns, and passes the first on.
    let again_searching = preview(&graph, &again, rows_of(&graph, &close));
    assert!(again_searching.contains(
        "SELECT episode_id, start_ns, end_ns, subtask, similarity, spql.ai.similarity(subtask, 'smooth the cloth'"
    ));
}

#[test]
fn a_node_missing_what_its_step_needs_says_so() {
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
    let nothing = child(
        &mut graph,
        &group,
        CurationNodeKind::Select(Selection::Categories { names: Vec::new() }),
    );
    if let CurationNodeKind::GroupSimilarSubtasks(grouping) =
        graph.node_mut(&group).unwrap().kind_mut()
    {
        grouping
            .set_categories(vec![SubtaskCategory::new("fold", "Folding.").unwrap()])
            .unwrap();
    }
    assert_eq!(reason(&graph, &nothing), "Choose the categories to keep.");
}

#[test]
fn each_measures_preview_is_its_published_step_over_its_parents_rows() {
    let mut graph = graph();
    let short = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::Select(Selection::Episodes {
            fact: EpisodeFact::DurationSeconds,
            comparison: Comparison::AtMost,
            value: 22.0,
        }),
    );
    let annotate = child(&mut graph, &short, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the skirt"));
    let group = child(&mut graph, &annotate, grouping(&["fold", "grasp"]));
    let detect = child(
        &mut graph,
        &short,
        CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
    );
    let moving = child(
        &mut graph,
        &detect,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let annotate_moving = child(&mut graph, &moving, breakdown());
    for (node, input) in [
        (&annotate, intervals()),
        (&detect, intervals()),
        (&annotate_moving, intervals()),
        (&search, rows_of(&graph, &annotate)),
        (&group, rows_of(&graph, &annotate)),
    ] {
        let parent = graph.parent_of(node).unwrap();
        let above = CurationPath::to(&graph, parent)
            .unwrap()
            .relation()
            .unwrap();
        let own = CurationPath::to(&graph, node).unwrap().relation().unwrap();
        // The node's own entries and the final SELECT, from its first entry.
        let step = |statement: &str| {
            let first = statement.find(&format!("\n  {}", own.name())).unwrap();
            statement[first..].to_owned()
        };
        // Published, the step reads the parent's entry; previewed, the rows
        // the parent's preview holds, for the bound episode only, failing
        // where the view answers NULL.
        let published = step(&published(&graph, node)).replace(above.name(), "preview_input");
        let previewed = step(&preview(&graph, node, input))
            .replace(" AND r.id = $1", "")
            .replace(", on_error => 'fail'", "");
        assert_eq!(previewed, published);
    }
}

#[test]
fn a_graph_without_a_dataset_has_nothing_to_read() {
    let mut graph = CurationGraph::new(None);
    let long = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::Select(Selection::for_basis(
            crate::activities::curation::document::SelectionBasis::EpisodeFacts,
        )),
    );
    assert!(matches!(
        CurationPath::to(&graph, &long).unwrap().statement(),
        Err(PathError(_))
    ));
}

/// The stretches the browser keeps for a progress Select are the ones its
/// statement keeps. The frames are the ones Spiral's in-memory engine was
/// given, in place of the unnested scores, and the stretches are what it
/// answered for everything after the unnest of this path's statement.
#[test]
fn the_browser_keeps_the_stretches_the_statement_keeps() {
    let mut graph = graph();
    let detect = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
    );
    let moving = child(
        &mut graph,
        &detect,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let (first, second) = (EpisodeId::fixture(1), EpisodeId::fixture(2));
    let sample = EpisodeSample::fixture(vec![
        Episode::fixture(&first, 100_000_000),
        Episode::fixture(&second, 40_000_000),
    ]);
    // Frame times in milliseconds from each episode's first message, with
    // the last frame cut at the episode's end, as the statement cuts it.
    let frames = |episode: &EpisodeId, frames: &[(i64, i64, Option<f64>)]| {
        frames
            .iter()
            .map(|(start, end, pace)| {
                Moment::new(
                    episode.clone(),
                    (*start as f64 / 1e3, *end as f64 / 1e3),
                    *pace,
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let detection = graph.node(&detect).unwrap().clone();
    let mut reads = PreviewReads::default();
    reads.record(
        &detection,
        (&first, ReadInput::untracked()),
        Ok(PreviewRows::Moments(frames(
            &first,
            &[
                (0, 20, Some(0.5)),
                (20, 40, None),
                (40, 60, Some(0.1)),
                (60, 80, Some(0.3)),
                (80, 95, Some(0.4)),
                (95, 100, Some(0.05)),
            ],
        ))),
    );
    reads.record(
        &detection,
        (&second, ReadInput::untracked()),
        Ok(PreviewRows::Moments(frames(
            &second,
            &[(0, 30, Some(0.9)), (30, 40, Some(0.9))],
        ))),
    );
    let kept = PopulationReader::new(&graph, &sample, &reads)
        .of(&moving)
        .slices(&sample)
        .iter()
        .map(|slice| {
            (
                slice.episode().clone(),
                (slice.start() * 1e3).round() as i64,
                (slice.end() * 1e3).round() as i64,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kept,
        [(first.clone(), 0, 40), (first, 60, 95), (second, 0, 40),]
    );
}
