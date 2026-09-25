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
    let score = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let moving = child(
        &mut graph,
        &score,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let published = published(&graph, &moving);
    for step in [
        "FROM spql.mcap.read_topic('abc130k', '/cameras/top_camera-images-rgb') r",
        "WHERE r.id IN (SELECT episode_id FROM dataset_0))",
        // Each episode's scores are a column of its row, whose start is its
        // first message.
        "episodes AS (SELECT i.episode_id, i.start_ns, i.end_ns, i.duration_seconds, i.messages, i.channels, i.size_megabytes, i.start_ns AS origin_ns, spql.ai.score_progress(camera_video_1.source, model => 'uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f') AS scores FROM dataset_0 i JOIN camera_video_1 ON camera_video_1.id = i.episode_id)",
        "r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS start_ns, p.frame_index AS frame_index, CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS progress_rate FROM episodes r CROSS JOIN LATERAL UNNEST(r.scores) p)",
        "COALESCE(LEAD(start_ns) OVER (PARTITION BY episode_id, reached_start_ns, reached_end_ns ORDER BY frame_index), start_ns + (start_ns - LAG(start_ns) OVER (PARTITION BY episode_id, reached_start_ns, reached_end_ns ORDER BY frame_index))) AS end_ns",
        "score_progress_1 AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, LEAST(end_ns, reached_end_ns) AS end_ns, progress_rate FROM score_progress_1_moments WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns AND end_ns > start_ns)",
        "FROM select_2_numbered WHERE (progress_rate IS NULL OR progress_rate >= CAST(0.2 AS DOUBLE)))",
        "select_2_stretches AS (SELECT episode_id, MIN(start_ns) OVER (PARTITION BY episode_id, stretch) AS start_ns, MAX(end_ns) OVER (PARTITION BY episode_id, stretch) AS end_ns, AVG(progress_rate) OVER (PARTITION BY episode_id, stretch) AS progress_rate, ROW_NUMBER() OVER (PARTITION BY episode_id, stretch ORDER BY start_ns) AS place FROM select_2_kept)",
        "select_2 AS (SELECT episode_id, start_ns, end_ns, progress_rate FROM select_2_stretches WHERE place = 1)",
    ] {
        assert!(published.contains(step), "{step}\n{published}");
    }
    assert!(
        published.ends_with("\nSELECT episode_id, start_ns, end_ns, progress_rate FROM select_2")
    );
    // The preview scores only the bound episode, and unnests its scores over
    // the intervals the dataset's preview hands on rather than the dataset
    // read again.
    let scoring = preview(&graph, &score, intervals());
    for step in [
        "WITH\n  preview_input(episode_id, start_ns, end_ns) AS (VALUES ($1, ",
        "FROM spql.mcap.read_topic('abc130k', '/cameras/top_camera-images-rgb') r JOIN spql.mcap.episodes('abc130k') e ON e.id = r.id WHERE r.id = $1)",
        "episodes AS (SELECT e.id AS episode_id, CAST(e.message_start_time AS BIGINT) AS origin_ns, spql.ai.score_progress(camera_video_1.source, model => 'uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f') AS scores FROM spql.mcap.episodes('abc130k') e JOIN camera_video_1 ON camera_video_1.id = e.id WHERE e.id = $1)",
        "preview_reached AS (SELECT i.episode_id, i.start_ns, i.end_ns, e.origin_ns, e.scores FROM preview_input i JOIN episodes e ON e.episode_id = i.episode_id)",
        "FROM preview_reached r CROSS JOIN LATERAL UNNEST(r.scores) p)",
    ] {
        assert!(scoring.contains(step), "{step}\n{scoring}");
    }
    assert!(!scoring.contains("dataset_0"));
    assert!(
        scoring.ends_with(
            "\nSELECT episode_id, start_ns, end_ns, progress_rate FROM score_progress_1"
        )
    );
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
        "instruction_1 AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time) AS position FROM spql.mcap.read_topic('abc130k', '/instruction') r WHERE r.id IN (SELECT episode_id FROM select_1)) instructions WHERE position = 1)",
        // The subtasks are a column of the episode's row, kept by the Select.
        "spql.ai.breakdown_task(camera_video_1.source, sample_interval => 0.5, model => 'google/gemini-3.8-flash', task => instruction_1.task) AS subtasks FROM select_1 i JOIN camera_video_1 ON camera_video_1.id = i.episode_id LEFT JOIN instruction_1 ON instruction_1.id = i.episode_id)",
        // Unnested from the row, and moved to log time: presentation time
        // counts from the episode's first message.
        "r.origin_ns + CAST(s.start_ns AS BIGINT) AS start_ns, r.origin_ns + CAST(s.end_ns AS BIGINT) AS end_ns, s.description AS subtask FROM episodes r CROSS JOIN LATERAL UNNEST(r.subtasks) AS s)",
        // Cut to the interval of the row they came on.
        "break_down_subtasks_2 AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, LEAST(end_ns, reached_end_ns) AS end_ns, subtask FROM break_down_subtasks_2_subtasks WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns)",
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
    assert!(breaking.contains("WHERE r.id = $1) instructions"));
    assert!(breaking.contains("task => instruction_1.task, on_error => 'fail') AS subtasks"));
    assert!(
        breaking.contains("FROM preview_reached r CROSS JOIN LATERAL UNNEST(r.subtasks) AS s)")
    );
    assert!(!breaking.contains("select_1") && !breaking.contains("dataset_0"));
    admitted(&published, &breaking);

    // A dataset that records no instruction asks without a task.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_task_topic(None);
    }
    let published = self::published(&graph, &annotate);
    assert!(!published.contains("instruction_"));
    assert!(published.contains(
        "model => 'google/gemini-3.8-flash') AS subtasks FROM select_1 i JOIN camera_video_1 ON camera_video_1.id = i.episode_id)"
    ));
}

/// Dataset, moments, the Select of the moving ones, then Break down on the
/// scorer's own camera: one video feeds both models, and the subtasks ride
/// on every moment and stretch until Break down unnests them.
#[test]
fn both_models_answer_on_the_episode_row_and_the_subtasks_ride_to_their_step() {
    let mut graph = graph();
    let score = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let moving = child(
        &mut graph,
        &score,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some(ProgressScorer::WarpSimBottles.camera().to_owned()));
    let annotate = child(
        &mut graph,
        &moving,
        CurationNodeKind::BreakDownSubtasks(annotation),
    );
    let published = published(&graph, &annotate);
    assert_eq!(published.matches("spql.mcap.read_topic(").count(), 1);
    for step in [
        "i.start_ns AS origin_ns, spql.ai.score_progress(camera_video_1.source, model => 'uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f') AS scores, spql.ai.breakdown_task(camera_video_1.source, sample_interval => 0.5, model => 'google/gemini-3.8-flash') AS subtasks FROM dataset_0 i JOIN camera_video_1 ON camera_video_1.id = i.episode_id)",
        "score_progress_1_frames AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns, r.origin_ns, r.subtasks, r.origin_ns + CAST(p.timestamp_ns AS BIGINT) AS start_ns,",
        "LEAST(end_ns, reached_end_ns) AS end_ns, progress_rate, origin_ns, subtasks FROM score_progress_1_moments",
        "AVG(progress_rate) OVER (PARTITION BY episode_id, stretch) AS progress_rate, origin_ns, subtasks, ROW_NUMBER()",
        "select_2 AS (SELECT episode_id, start_ns, end_ns, progress_rate, origin_ns, subtasks FROM select_2_stretches WHERE place = 1)",
        "break_down_subtasks_3_subtasks AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns, r.origin_ns + CAST(s.start_ns AS BIGINT) AS start_ns, r.origin_ns + CAST(s.end_ns AS BIGINT) AS end_ns, s.description AS subtask FROM select_2 r CROSS JOIN LATERAL UNNEST(r.subtasks) AS s)",
    ] {
        assert!(published.contains(step), "{step}\n{published}");
    }
    // The preview's rows are flat: Break down asks the bound episode for its
    // own subtasks beside the stretches it is handed.
    let breaking = preview(&graph, &annotate, intervals());
    assert!(!breaking.contains("score_progress"));
    assert!(breaking.contains(
        "preview_reached AS (SELECT i.episode_id, i.start_ns, i.end_ns, e.origin_ns, e.subtasks FROM preview_input i JOIN episodes e ON e.episode_id = i.episode_id)"
    ));
    admitted(&published, &breaking);
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
    let score = child(
        &mut graph,
        &short,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let moving = child(
        &mut graph,
        &score,
        CurationNodeKind::Select(Selection::Progressing { minimum: 0.2 }),
    );
    let annotate_moving = child(&mut graph, &moving, breakdown());
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
    // the rows the parent's preview holds, beside the node's own episode row
    // when it asks the episode anything.
    for (node, input, (published_input, previewed_input)) in [
        (
            &annotate,
            intervals(),
            ("episodes".to_owned(), "preview_reached"),
        ),
        (
            &score,
            intervals(),
            ("episodes".to_owned(), "preview_reached"),
        ),
        (
            &annotate_moving,
            intervals(),
            (name(&moving), "preview_reached"),
        ),
        (
            &search,
            rows_of(&graph, &annotate),
            (name(&annotate), "preview_input"),
        ),
        (
            &group,
            rows_of(&graph, &annotate),
            (name(&annotate), "preview_input"),
        ),
    ] {
        let own = name(node);
        // The node's own entries and the final SELECT, from its first entry.
        let step = |statement: &str| {
            let first = statement.find(&format!("\n  {own}")).unwrap();
            statement[first..].to_owned()
        };
        // The same step over either input, failing where the view answers
        // NULL.
        let published = step(&published(&graph, node)).replace(&published_input, previewed_input);
        let previewed = step(&preview(&graph, node, input)).replace(", on_error => 'fail'", "");
        assert_eq!(previewed, published);
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

    /// Every entry that asks a model, itself or through an entry it reads.
    fn asking(&self) -> Vec<&str> {
        let mut asking = Vec::<&str>::new();
        for (name, entry) in &self.entries {
            if entry.contains("spql.ai.")
                || asking
                    .iter()
                    .any(|above| Self::reads_within(entry, above) > 0)
            {
                asking.push(name);
            }
        }
        asking
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
        CurationNodeKind::Select(Selection::Categories { .. }) => {
            vec![CurationNodeKind::Select(Selection::Categories {
                names: vec!["fold".into()],
            })]
        }
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

/// Spiral pastes an entry into every place that reads it, so an entry read
/// twice runs twice. No published path reads an entry that asks a model more
/// than once, and each model node's call is written once.
#[test]
fn no_published_path_asks_a_model_twice() {
    let mut graph = graph();
    let mut most_models = 0;
    every_path(&mut graph, (DATASET_NODE_ID, 6), &mut |graph, node| {
        let statement = published(graph, node);
        let compiled = CompiledEntries::of(&statement);
        for asking in compiled.asking() {
            assert!(
                compiled.reads(asking) <= 1,
                "{asking} is read {} times\n{statement}",
                compiled.reads(asking)
            );
        }
        let models = CurationPath::to(graph, node)
            .unwrap()
            .nodes
            .iter()
            .filter(|node| {
                !matches!(
                    node.kind(),
                    CurationNodeKind::Dataset | CurationNodeKind::Select(_)
                )
            })
            .count();
        assert_eq!(statement.matches("spql.ai.").count(), models, "{statement}");
        most_models = most_models.max(models);
    });
    // Some path asks all four: moments, Break down, and both judgments.
    assert_eq!(most_models, 4);
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
    let score = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let moving = child(
        &mut graph,
        &score,
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
