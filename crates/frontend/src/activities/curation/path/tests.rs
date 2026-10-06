use super::*;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{
    Comparison, DATASET_NODE_ID, EpisodeFact, ProgressScoring, Selection, SubtaskAnnotation,
    SubtaskCategory, SubtaskGrouping, SubtaskSearch,
};
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::preview::{PreviewReads, PreviewRows, ReadInput};
use crate::activities::curation::score_progress::{Moment, ProgressScorer};
use crate::activities::episode::Episode;
use spiral_robotics_common::curation::{AnswerSource, FillEpisodes};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};

fn graph() -> CurationGraph {
    CurationGraph::new(Some(
        DatasetSelection::new(vec!["dataset_abc130k".to_owned()]).unwrap(),
    ))
}

fn child(graph: &mut CurationGraph, parent: &str, kind: CurationNodeKind) -> String {
    graph
        .add_child(parent, kind)
        .expect("the grammar allows it")
}

/// A child keeping by `rule` rather than the rule its kind starts with.
fn keeping(
    graph: &mut CurationGraph,
    (parent, kind): (&str, CurationNodeKind),
    rule: Selection,
) -> String {
    graph
        .add_keeping(parent, kind, rule)
        .expect("the grammar allows it")
}

/// Episodes lasting at least, or at most, `seconds`.
fn lasting(comparison: Comparison, seconds: f64) -> Selection {
    Selection::episodes_where(EpisodeFact::DurationSeconds, comparison, seconds)
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
        .without_answers()
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

/// A grouping by `names`, which a new node keeps every one of.
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

/// The column a node's call stores its answers in, on its path.
fn column(graph: &CurationGraph, node: &str) -> String {
    let path = CurationPath::to(graph, node).unwrap();
    let columns = path.answer_columns().unwrap();
    columns.last().unwrap().as_ref().unwrap().column.to_string()
}

const ANSWERS: &str = "\"dataset_answers_abc130k\"";

#[test]
fn a_select_on_duration_keeps_whole_episodes_and_publishes_the_fact_it_read() {
    let mut graph = graph();
    let long = keeping(
        &mut graph,
        (DATASET_NODE_ID, CurationNodeKind::Select),
        lasting(Comparison::AtLeast, 10.0),
    );
    // The view reads every episode the Select keeps, and asks no model.
    assert_eq!(
        published(&graph, &long),
        "WITH
  dataset_0 AS (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, CAST(message_start_time + duration_ns AS BIGINT) AS end_ns, CAST(duration_ns AS DOUBLE) / 1e9 AS duration_seconds, message_count AS messages, channel_count AS channels, CAST(source_bytes AS DOUBLE) / 1e6 AS size_megabytes FROM (SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, message_start_time, duration_ns, channel_count FROM spql.mcap.episodes('dataset_abc130k')) listed),
  select_1 AS (SELECT episode_id, start_ns, end_ns, duration_seconds, messages, channels, size_megabytes FROM dataset_0 WHERE duration_seconds >= CAST(10 AS DOUBLE))
SELECT episode_id, start_ns, end_ns, duration_seconds FROM select_1"
    );
    let path = CurationPath::to(&graph, &long).unwrap();
    assert!(path.fills().unwrap().is_empty(), "nothing to store");
}

#[test]
fn a_view_reads_the_answers_its_models_stored_for_every_episode_its_selects_keep() {
    let mut graph = graph();
    let long = keeping(
        &mut graph,
        (DATASET_NODE_ID, CurationNodeKind::Select),
        lasting(Comparison::AtLeast, 10.0),
    );
    let score = child(
        &mut graph,
        &long,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let scores = column(&graph, &score);
    assert!(scores.starts_with("score_progress_"));
    let published = published(&graph, &score);
    // The episodes the Select keeps are joined to the answers table once,
    // and nothing is bounded, read from a camera, or asked of a model.
    assert!(
        published.contains(&format!(
            "episodes AS (SELECT i.episode_id, i.start_ns, i.end_ns, i.duration_seconds, i.messages, i.channels, i.size_megabytes, i.start_ns AS origin_ns, a.{scores}.value AS {scores} FROM select_1 i JOIN {ANSWERS} a ON a.episode_id = i.episode_id)"
        )),
        "{published}"
    );
    for absent in ["LIMIT", "spql.ai.", "read_topic"] {
        assert!(!published.contains(absent), "{absent}\n{published}");
    }
    // Its fill scores every episode the Select keeps, on the Modal service.
    let fills = CurationPath::to(&graph, &score).unwrap().fills().unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].column().as_str(), scores);
    assert_eq!(
        fills[0].source(),
        &AnswerSource::ProgressService {
            camera: ProgressScorer::WarpSimBottles.camera().to_owned(),
            model: "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
                .to_owned(),
            revision: "warp-progress-0.2.0-fp16".to_owned(),
        }
    );
    let FillEpisodes::Reaching(reaching) = fills[0].episodes() else {
        panic!("a published fill asks what reaches the node")
    };
    assert!(reaching.ends_with("\nSELECT DISTINCT episode_id FROM select_1"));
    assert_eq!(
        ReadOnlyQuery::new(reaching, ReadOnlyQueryMode::Curation).authorize(),
        Ok(())
    );
}

#[test]
fn score_progress_scores_every_frame_and_joins_the_moments_its_rule_keeps() {
    let mut graph = graph();
    let score = keeping(
        &mut graph,
        (
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        ),
        Selection::Progressing { minimum: 0.2 },
    );
    let scores = column(&graph, &score);
    let published = published(&graph, &score);
    for step in [
        // Each episode's scores are a column of its row, whose start is its
        // first message.
        format!("episodes AS (SELECT i.episode_id, i.start_ns, i.end_ns, i.duration_seconds, i.messages, i.channels, i.size_megabytes, i.start_ns AS origin_ns, a.{scores}.value AS {scores} FROM dataset_0 i JOIN {ANSWERS} a ON a.episode_id = i.episode_id)"),
        format!("r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS start_ns, p.frame_index AS frame_index, CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS progress_rate FROM episodes r CROSS JOIN LATERAL UNNEST(r.{scores}) p)"),
        "COALESCE(LEAD(start_ns) OVER (PARTITION BY episode_id, reached_start_ns, reached_end_ns ORDER BY frame_index), start_ns + (start_ns - LAG(start_ns) OVER (PARTITION BY episode_id, reached_start_ns, reached_end_ns ORDER BY frame_index))) AS end_ns".to_owned(),
        "score_progress_1 AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, LEAST(end_ns, reached_end_ns) AS end_ns, progress_rate FROM score_progress_1_moments WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns AND end_ns > start_ns)".to_owned(),
        // The rule keeps from what the step wrote, under a name of its own.
        "FROM score_progress_1_kept_numbered WHERE (progress_rate IS NULL OR progress_rate >= CAST(0.2 AS DOUBLE)))".to_owned(),
        "score_progress_1_kept_stretches AS (SELECT episode_id, MIN(start_ns) OVER (PARTITION BY episode_id, stretch) AS start_ns, MAX(end_ns) OVER (PARTITION BY episode_id, stretch) AS end_ns, AVG(progress_rate) OVER (PARTITION BY episode_id, stretch) AS progress_rate, ROW_NUMBER() OVER (PARTITION BY episode_id, stretch ORDER BY start_ns) AS place FROM score_progress_1_kept_passing)".to_owned(),
        "score_progress_1_kept AS (SELECT episode_id, start_ns, end_ns, progress_rate FROM score_progress_1_kept_stretches WHERE place = 1)".to_owned(),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, progress_rate FROM score_progress_1_kept"
    ));
    // The preview reads the bound episode's stored scores, which it has the
    // backend fill first, and unnests them over the intervals the dataset's
    // preview hands on rather than the dataset read again. It leaves the
    // rule out: the browser keeps by it over every frame the step read.
    let scoring = preview(&graph, &score, intervals());
    for step in [
        "WITH\n  preview_input(episode_id, start_ns, end_ns) AS (VALUES ($1, ".to_owned(),
        format!(
            "episodes AS (SELECT e.id AS episode_id, CAST(e.message_start_time AS BIGINT) AS origin_ns, a.{scores}.value AS {scores} FROM spql.mcap.episodes('dataset_abc130k') e JOIN {ANSWERS} a ON a.episode_id = e.id WHERE e.id = $1)"
        ),
        format!(
            "preview_reached AS (SELECT i.episode_id, i.start_ns, i.end_ns, e.origin_ns, e.{scores} FROM preview_input i JOIN episodes e ON e.episode_id = i.episode_id)"
        ),
        format!("FROM preview_reached r CROSS JOIN LATERAL UNNEST(r.{scores}) p)"),
    ] {
        assert!(scoring.contains(&step), "{step}\n{scoring}");
    }
    assert!(!scoring.contains("dataset_0") && !scoring.contains("_kept"));
    assert!(!scoring.contains("spql.ai.") && !scoring.contains("read_topic"));
    assert!(
        scoring.ends_with(
            "\nSELECT episode_id, start_ns, end_ns, progress_rate FROM score_progress_1"
        )
    );
    let listed = vec![EpisodeId::fixture(1)];
    let fill = CurationPath::to(&graph, &score)
        .unwrap()
        .fill_for(listed.clone())
        .unwrap();
    assert_eq!(fill.column().as_str(), scores);
    assert_eq!(fill.episodes(), &FillEpisodes::Listed(listed));
    admitted(&published, &scoring);
}

#[test]
fn a_breakdown_reads_each_episode_whole_and_cuts_its_subtasks_to_what_reached_it() {
    let mut graph = graph();
    let long = keeping(
        &mut graph,
        (DATASET_NODE_ID, CurationNodeKind::Select),
        lasting(Comparison::AtLeast, 10.0),
    );
    let annotate = child(&mut graph, &long, breakdown());
    let subtasks = column(&graph, &annotate);
    assert!(subtasks.starts_with("breakdown_task_"));
    // The view reads the stored subtasks on the row the Select kept, and
    // lists each at its place, cut to the interval of the row it came on.
    let published = published(&graph, &annotate);
    assert!(published.contains(&format!(
        "a.{subtasks}.value AS {subtasks} FROM select_1 i JOIN {ANSWERS} a"
    )));
    assert!(published.contains("g._ AS subtask_index"));
    assert!(published.contains("WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns)"));
    assert!(!published.contains("spql.ai.") && !published.contains("read_topic"));
    // Its fill asks the model about the episode bound as `$1` whole, from
    // the chosen camera, with its first instruction as the task.
    let fills = CurationPath::to(&graph, &annotate)
        .unwrap()
        .fills()
        .unwrap();
    let AnswerSource::Statement { statement } = fills[0].source() else {
        panic!("Break down is filled by a statement")
    };
    assert!(statement.contains("spql.mcap.read_topic('dataset_abc130k', '/top-camera')"));
    assert!(statement.contains(
        "spql.ai.breakdown_task(camera_video_1.source, sample_interval => 0.5, model => 'google/gemini-3.8-flash', task => instruction_1.task)"
    ));
    assert!(statement.ends_with(&format!(
        "\nSELECT episode_id, {subtasks} AS answer FROM episodes"
    )));
    // The preview reads the bound episode's stored subtasks, which its fill
    // stored first, and cuts them to its parent's preview rows.
    let breaking = preview(&graph, &annotate, intervals());
    assert!(breaking.contains(&format!("a.{subtasks}.value AS {subtasks}")));
    assert!(breaking.contains("WHERE e.id = $1)"));
    assert!(!breaking.contains("spql.ai.") && !breaking.contains("select_1"));
    admitted(&published, &breaking);

    // A dataset that records no instruction asks without a task, and that
    // is another call, stored in another column.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_task_topic(None);
    }
    assert_ne!(column(&graph, &annotate), subtasks);
    let fills = CurationPath::to(&graph, &annotate)
        .unwrap()
        .fills()
        .unwrap();
    let AnswerSource::Statement { statement } = fills[0].source() else {
        panic!("Break down is filled by a statement")
    };
    assert!(!statement.contains("instruction_"));
    assert!(statement.contains("model => 'google/gemini-3.8-flash') AS breakdown_task_"));
}

/// Dataset, Score progress keeping the moving stretches, then Break down on
/// the scorer's own camera: both answers are read on the episode row, and
/// the subtasks ride on every moment and stretch until Break down lists
/// them.
#[test]
fn both_answers_are_read_on_the_episode_row_and_the_subtasks_ride_to_their_step() {
    let mut graph = graph();
    let score = keeping(
        &mut graph,
        (
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        ),
        Selection::Progressing { minimum: 0.2 },
    );
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some(ProgressScorer::WarpSimBottles.camera().to_owned()));
    let annotate = child(
        &mut graph,
        &score,
        CurationNodeKind::BreakDownSubtasks(annotation),
    );
    let (scores, subtasks) = (column(&graph, &score), column(&graph, &annotate));
    let published = published(&graph, &annotate);
    assert_eq!(published.matches(ANSWERS).count(), 1);
    for step in [
        format!(
            "i.start_ns AS origin_ns, a.{scores}.value AS {scores}, a.{subtasks}.value AS {subtasks} FROM dataset_0 i JOIN {ANSWERS} a ON a.episode_id = i.episode_id)"
        ),
        format!(
            "score_progress_1_frames AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns, r.origin_ns, r.{subtasks}, r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS start_ns,"
        ),
        format!(
            "score_progress_1_kept AS (SELECT episode_id, start_ns, end_ns, progress_rate, origin_ns, {subtasks} FROM score_progress_1_kept_stretches WHERE place = 1)"
        ),
        format!(
            "spql.list.get(r.{subtasks}, g._) AS listed FROM score_progress_1_kept r CROSS JOIN LATERAL spql.range(CAST(spql.list.len(r.{subtasks}) AS BIGINT)) g)"
        ),
    ] {
        assert!(published.contains(&step), "{step}\n{published}");
    }
    // Filled in path order: the subtasks of the episodes whose stretches
    // reach Break down, read over the scores filled first.
    let fills = CurationPath::to(&graph, &annotate)
        .unwrap()
        .fills()
        .unwrap();
    assert_eq!(
        fills
            .iter()
            .map(|fill| fill.column().to_string())
            .collect::<Vec<_>>(),
        [scores.clone(), subtasks.clone()]
    );
    let FillEpisodes::Reaching(reaching) = fills[1].episodes() else {
        panic!("a published fill asks what reaches the node")
    };
    assert!(reaching.contains(&format!("a.{scores}.value AS {scores} FROM dataset_0 i")));
    assert!(!reaching.contains(&subtasks));
    assert!(reaching.ends_with("\nSELECT DISTINCT episode_id FROM score_progress_1_kept"));
    // The preview's rows are flat: Break down asks the bound episode for its
    // own subtasks beside the stretches it is handed.
    let breaking = preview(&graph, &annotate, intervals());
    assert!(!breaking.contains("score_progress"));
    assert!(breaking.contains(&format!(
        "preview_reached AS (SELECT i.episode_id, i.start_ns, i.end_ns, e.origin_ns, e.{subtasks} FROM preview_input i JOIN episodes e ON e.episode_id = i.episode_id)"
    )));
    admitted(&published, &breaking);
}

/// The example the plan names: fold subtasks, grouped from Break down.
#[test]
fn a_grouping_publishes_the_subtasks_of_the_categories_it_keeps() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let group = child(&mut graph, &annotate, grouping(&["fold", "flatten"]));
    graph
        .set_rule(
            &group,
            Selection::Categories {
                names: vec!["fold".into()],
            },
        )
        .unwrap();
    let (subtasks, categories) = (column(&graph, &annotate), column(&graph, &group));
    assert!(categories.starts_with("subtask_category_"));
    let published = published(&graph, &group);
    // Each subtask's category is the stored one at its place in the Break
    // down answer, and the list rides on no further.
    assert!(
        published.contains(&format!(
            "group_similar_subtasks_2 AS (SELECT episode_id, start_ns, end_ns, subtask, subtask_index, origin_ns, {categories}, spql.list.get({categories}, subtask_index) AS subtask_category FROM break_down_subtasks_1)"
        )),
        "{published}"
    );
    assert!(published.contains(&format!(
        "group_similar_subtasks_2_kept AS (SELECT episode_id, start_ns, end_ns, subtask, subtask_index, subtask_category, origin_ns, {categories} FROM group_similar_subtasks_2 WHERE group_similar_subtasks_2.subtask_category.category IN ('fold'))"
    )));
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, subtask, subtask_category FROM group_similar_subtasks_2_kept"
    ));
    assert!(!published.contains("spql.ai."));
    // Its fill classifies each subtask the Break down stored for the
    // episode bound as `$1`, collected in their order.
    let fills = CurationPath::to(&graph, &group).unwrap().fills().unwrap();
    let AnswerSource::Statement { statement } = fills[1].source() else {
        panic!("a judgment is filled by a statement")
    };
    assert_eq!(
        statement,
        &format!(
            "WITH e AS (SELECT a.episode_id, a.{subtasks}.value AS subtasks FROM {ANSWERS} a WHERE a.episode_id = $1), \
             s AS (SELECT e.episode_id, spql.list.get(e.subtasks, g._) AS listed \
             FROM e CROSS JOIN LATERAL spql.range(CAST(spql.list.len(e.subtasks) AS BIGINT)) g) \
             SELECT s.episode_id, spql.list.collect(spql.ai.classify(s.listed.description, [STRUCT_PACK(name := 'fold', definition := 'The robot folds the cloth.'), STRUCT_PACK(name := 'flatten', definition := 'The robot flattens the cloth.')], model => 'typesafe/jev-1.13')) AS answer \
             FROM s GROUP BY s.episode_id"
        )
    );
    // The preview reads the stored categories of the rows Break down's
    // preview holds, and asks nothing.
    let grouping = preview(&graph, &group, rows_of(&graph, &annotate));
    assert!(grouping.contains(&format!("a.{categories}.value AS {categories}")));
    assert!(grouping.contains(&format!(
        "spql.list.get({categories}, subtask_index) AS subtask_category"
    )));
    assert!(!grouping.contains("spql.ai."));
    admitted(&published, &grouping);
}

#[test]
fn a_search_publishes_the_subtasks_its_cutoff_keeps_and_a_repeat_writes_its_own_column() {
    let mut graph = graph();
    let annotate = child(&mut graph, DATASET_NODE_ID, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the towel's edge"));
    let similarities = column(&graph, &search);
    let published = published(&graph, &search);
    assert!(
        published.contains(&format!(
            "find_similar_subtasks_2 AS (SELECT episode_id, start_ns, end_ns, subtask, subtask_index, origin_ns, {similarities}, spql.list.get({similarities}, subtask_index) AS similarity FROM break_down_subtasks_1)"
        )),
        "{published}"
    );
    // A new search keeps the subtasks more likely than not the same
    // operation.
    assert!(published.contains(&format!(
        "find_similar_subtasks_2_kept AS (SELECT episode_id, start_ns, end_ns, subtask, subtask_index, similarity, origin_ns, {similarities} FROM find_similar_subtasks_2 WHERE similarity >= CAST(0.5 AS DOUBLE))"
    )));
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, subtask, similarity FROM find_similar_subtasks_2_kept"
    ));
    let fills = CurationPath::to(&graph, &search).unwrap().fills().unwrap();
    let AnswerSource::Statement { statement } = fills[1].source() else {
        panic!("a judgment is filled by a statement")
    };
    assert!(statement.contains(&format!(
        "a.{}.value AS subtasks",
        column(&graph, &annotate)
    )));
    assert!(statement.contains(
        "spql.list.collect(spql.ai.similarity(s.listed.description, 'fold the towel''s edge', model => 'typesafe/jev-1.13')) AS answer"
    ));
    let asking = preview(&graph, &search, rows_of(&graph, &annotate));
    assert!(asking.contains(&format!(
        "spql.list.get({similarities}, subtask_index) AS similarity"
    )));
    assert!(!asking.contains("spql.ai."));
    admitted(&published, &asking);

    // A second search below the first keeps the first's column and writes
    // its own, which its own cutoff reads, from a column of its own.
    let again = keeping(
        &mut graph,
        (&search, searching("smooth the cloth")),
        Selection::Similar { minimum: 0.8 },
    );
    let again_similarities = column(&graph, &again);
    assert_ne!(again_similarities, similarities);
    let published = self::published(&graph, &again);
    assert!(
        published.contains(&format!(
            "spql.list.get({again_similarities}, subtask_index) AS similarity_2 FROM find_similar_subtasks_2_kept)"
        )),
        "{published}"
    );
    assert!(published.contains("WHERE similarity_2 >= CAST(0.8 AS DOUBLE))"));
    assert!(published.ends_with(
        "\nSELECT episode_id, start_ns, end_ns, subtask, similarity, similarity_2 FROM find_similar_subtasks_3_kept"
    ));
    let path = CurationPath::to(&graph, &again).unwrap();
    assert_eq!(
        path.relation().unwrap().measured(SubtaskRow::SIMILARITY),
        Some("similarity_2")
    );
    // The same search again reads the same stored column.
    let repeated = child(&mut graph, &again, searching("fold the towel's edge"));
    assert_eq!(column(&graph, &repeated), similarities);
    let repeated = self::published(&graph, &repeated);
    assert_eq!(
        repeated.matches(&format!("a.{similarities}.value")).count(),
        1
    );
    assert_eq!(
        repeated
            .matches(&format!("spql.list.get({similarities}, subtask_index)"))
            .count(),
        2
    );
    // Its preview is handed both columns, and passes the first on.
    let again_searching = preview(&graph, &again, rows_of(&graph, &search));
    assert!(again_searching.contains("subtask, subtask_index, similarity, "));
    assert!(again_searching.contains(" AS similarity_2 FROM preview_reached)"));
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
fn only_a_dataset_studio_imported_keeps_the_answers_a_model_node_publishes() {
    let mut graph = CurationGraph::new(Some(
        DatasetSelection::new(vec!["recordings".to_owned()]).unwrap(),
    ));
    let long = keeping(
        &mut graph,
        (DATASET_NODE_ID, CurationNodeKind::Select),
        lasting(Comparison::AtLeast, 10.0),
    );
    let score = child(
        &mut graph,
        &long,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    assert!(CurationPath::to(&graph, &long).unwrap().statement().is_ok());
    assert_eq!(
        CurationPath::to(&graph, &score)
            .unwrap()
            .statement()
            .unwrap_err()
            .message(),
        "recordings keeps no model answers; only datasets Studio imported do."
    );
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
    // A grouping that keeps no category has nothing to publish.
    graph
        .node_mut(&group)
        .unwrap()
        .propose_categories(vec![SubtaskCategory::new("fold", "Folding.").unwrap()])
        .unwrap();
    graph
        .set_rule(&group, Selection::Categories { names: Vec::new() })
        .unwrap();
    assert_eq!(
        reason(&graph, &group),
        "Keep at least one category: this grouping keeps none."
    );
}

#[test]
fn a_preview_is_the_published_step_over_its_parents_rows() {
    let mut graph = graph();
    let short = keeping(
        &mut graph,
        (DATASET_NODE_ID, CurationNodeKind::Select),
        lasting(Comparison::AtMost, 22.0),
    );
    let annotate = child(&mut graph, &short, breakdown());
    let search = child(&mut graph, &annotate, searching("fold the skirt"));
    let group = child(&mut graph, &annotate, grouping(&["fold", "grasp"]));
    let score = keeping(
        &mut graph,
        (
            &short,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        ),
        Selection::Progressing { minimum: 0.2 },
    );
    let annotate_moving = child(&mut graph, &score, breakdown());
    let name = |node: &str| {
        CurationPath::to(&graph, node)
            .unwrap()
            .relation()
            .unwrap()
            .name()
            .to_owned()
    };
    // What each step reads: published, the episode rows where the path
    // leaves whole episodes and the entry above it after that; previewed,
    // the rows the parent's preview holds, beside the node's own episode row.
    for (node, (published_input, previewed_input)) in [
        (&annotate, ("episodes".to_owned(), "preview_reached")),
        (&score, ("episodes".to_owned(), "preview_reached")),
        (&annotate_moving, (name(&score), "preview_reached")),
    ] {
        // The node's own step, whose rule the published view keeps by after
        // it and the preview leaves to the browser.
        let own = name(node).trim_end_matches("_kept").to_owned();
        // The node's own entries, from its first to its rule's or the final
        // SELECT.
        let step = |statement: &str| {
            let first = statement.find(&format!("\n  {own}")).unwrap();
            let rest = &statement[first..];
            let end = rest
                .find(&format!(",\n  {own}_kept"))
                .or_else(|| rest.find("\nSELECT "))
                .unwrap();
            rest[..end].to_owned()
        };
        let published = step(&published(&graph, node)).replace(&published_input, previewed_input);
        let previewed = step(&preview(&graph, node, intervals()));
        assert_eq!(previewed, published);
    }
    // A judgment's preview reads the stored list, as its view does.
    for node in [&search, &group] {
        let lookup = format!("spql.list.get({}, subtask_index)", column(&graph, node));
        assert!(preview(&graph, node, rows_of(&graph, &annotate)).contains(&lookup));
        assert!(published(&graph, node).contains(&lookup));
    }
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
        // Entries end at a comma outside every parenthesis and quote.
        let (mut entries, mut depth, mut quoted, mut start) = (Vec::new(), 0, false, 0);
        for (index, character) in with.char_indices() {
            match character {
                '\'' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth -= 1,
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

/// Every kind the grammar offers after a node, configured to compile, with
/// Break down both on a camera of its own and on the scorer's.
fn configured(kind: CurationNodeKind) -> Vec<CurationNodeKind> {
    match kind {
        CurationNodeKind::BreakDownSubtasks(_) => {
            let mut scorers = SubtaskAnnotation::default();
            scorers.set_camera(Some(ProgressScorer::WarpSimBottles.camera().to_owned()));
            vec![breakdown(), CurationNodeKind::BreakDownSubtasks(scorers)]
        }
        CurationNodeKind::FindSimilarSubtasks(_) => vec![searching("fold the towel")],
        CurationNodeKind::GroupSimilarSubtasks(_) => vec![grouping(&["fold", "grasp"])],
        kind => vec![kind],
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
    for kind in graph.child_kinds(node).into_iter().flat_map(configured) {
        let added = child(graph, node, kind);
        check(graph, &added);
        every_path(graph, (&added, below - 1), check);
        graph.remove_node(&added).unwrap();
    }
}

/// No published path asks a model: every node that asks one reads its
/// stored column, joined to the episode row once, and a fill writes each in
/// path order, finding the episodes that reach it over the columns above.
#[test]
fn no_published_path_asks_a_model_and_each_fill_reads_only_what_is_filled_before_it() {
    let mut graph = graph();
    let mut most_models = 0;
    every_path(&mut graph, (DATASET_NODE_ID, 6), &mut |graph, node| {
        let statement = published(graph, node);
        assert!(!statement.contains("spql.ai."), "{statement}");
        let path = CurationPath::to(graph, node).unwrap();
        let fills = path.fills().unwrap();
        assert!(statement.matches(ANSWERS).count() <= 1, "{statement}");
        if !fills.is_empty() {
            assert_eq!(CompiledEntries::of(&statement).reads("episodes"), 1);
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
            assert!(!reaching.contains("spql.ai."));
            let above = &fills[..filled];
            for later in &fills[filled..] {
                if !above.iter().any(|fill| fill.column() == later.column()) {
                    assert!(!reaching.contains(later.column().as_str()), "{reaching}");
                }
            }
            if let AnswerSource::Statement { statement } = fill.source() {
                assert_eq!(statement.matches("spql.ai.").count(), 1, "{statement}");
                assert_eq!(
                    ReadOnlyQuery::new(statement, ReadOnlyQueryMode::Curation).authorize(),
                    Ok(())
                );
            }
        }
        let models = path
            .nodes
            .iter()
            .filter(|node| {
                !matches!(
                    node.kind(),
                    CurationNodeKind::Dataset | CurationNodeKind::Select
                )
            })
            .count();
        assert_eq!(fills.len(), models);
        most_models = most_models.max(models);
    });
    // Some path asks a model at every one of its six steps: moments, Break
    // down, and a judgment of what each judgment before it kept.
    assert_eq!(most_models, 6);
}

#[test]
fn a_graph_without_a_dataset_has_nothing_to_read() {
    let mut graph = CurationGraph::new(None);
    let long = child(&mut graph, DATASET_NODE_ID, CurationNodeKind::Select);
    assert!(matches!(
        CurationPath::to(&graph, &long).unwrap().statement(),
        Err(PathError(_))
    ));
}

/// The stretches the browser keeps for Score progress are the ones its
/// statement keeps. The frames are the ones Spiral's in-memory engine was
/// given, in place of the unnested scores, and the stretches are what it
/// answered for everything after the unnest of this path's statement.
#[test]
fn the_browser_keeps_the_stretches_the_statement_keeps() {
    let mut graph = graph();
    let score = keeping(
        &mut graph,
        (
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        ),
        Selection::Progressing { minimum: 0.2 },
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
            .map(|(start, end, progress_rate)| {
                Moment::new(
                    episode.clone(),
                    (*start as f64 / 1e3, *end as f64 / 1e3),
                    *progress_rate,
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let score_node = graph.node(&score).unwrap().clone();
    let mut reads = PreviewReads::default();
    reads.record(
        &score_node,
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
        &score_node,
        (&second, ReadInput::untracked()),
        Ok(PreviewRows::Moments(frames(
            &second,
            &[(0, 30, Some(0.9)), (30, 40, Some(0.9))],
        ))),
    );
    let kept = PopulationReader::new(&graph, &sample, &reads)
        .of(&score)
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
