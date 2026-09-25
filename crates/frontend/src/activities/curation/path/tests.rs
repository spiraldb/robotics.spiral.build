use super::*;
use crate::activities::curation::document::{
    Comparison, DATASET_NODE_ID, EpisodeFact, MomentDetection, Selection, SubtaskAnnotation,
    SubtaskSearch,
};
use crate::activities::curation::moments::Moment;
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::preview::PreviewReads;
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

fn statements(graph: &CurationGraph, node: &str) -> (String, String) {
    let path = CurationPath::to(graph, node).unwrap();
    (
        path.statement(PathSource::Published).unwrap(),
        path.statement(PathSource::Preview).unwrap(),
    )
}

/// The preview with its bound episode taken out, which is what the published
/// view must be.
fn unbound(preview: &str) -> String {
    preview
        .replace(" WHERE id = $1", "")
        .replace(" AND r.id = $1", "")
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
    let (published, preview) = statements(&graph, &long);
    assert_eq!(
        published,
        "WITH
  dataset_0 AS (SELECT id AS episode_id, CAST(message_start_time AS BIGINT) AS start_ns, CAST(message_start_time + duration_ns AS BIGINT) AS end_ns, CAST(duration_ns AS DOUBLE) / 1e9 AS duration_seconds, message_count AS messages, channel_count AS channels, CAST(source_bytes AS DOUBLE) / 1e6 AS size_megabytes FROM (SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, message_start_time, duration_ns, channel_count FROM spql.mcap.episodes('abc130k')) listed),
  select_1 AS (SELECT episode_id, start_ns, end_ns, duration_seconds, messages, channels, size_megabytes FROM dataset_0 WHERE duration_seconds >= CAST(10 AS DOUBLE))
SELECT episode_id, start_ns, end_ns, duration_seconds FROM select_1"
    );
    // The preview reads only the bound episode, and nothing else differs.
    assert!(preview.contains(" listed WHERE id = $1)"));
    assert_eq!(unbound(&preview), published);
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
    let (published, preview) = statements(&graph, &moving);
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
    // The preview binds its episode where the dataset is listed and where the
    // camera is read, so it never reads the rest of the dataset.
    assert!(preview.contains("WHERE r.id IN (SELECT episode_id FROM dataset_0) AND r.id = $1)"));
    assert_eq!(unbound(&preview), published);
    assert_eq!(
        ReadOnlyQuery::new(&published, ReadOnlyQueryMode::Curation).authorize(),
        Ok(())
    );
    assert_eq!(
        ReadOnlyQuery::new(&preview, ReadOnlyQueryMode::Preview).authorize(),
        Ok(())
    );
}

#[test]
fn a_path_through_a_node_spiral_cannot_run_yet_names_what_it_waits_for() {
    let mut graph = graph();
    let annotate = child(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
    );
    let group = child(
        &mut graph,
        &annotate,
        CurationNodeKind::GroupSimilarSubtasks,
    );
    let folds = child(
        &mut graph,
        &group,
        CurationNodeKind::Select(Selection::Categories {
            names: vec!["fold".into()],
        }),
    );
    let search = child(
        &mut graph,
        &annotate,
        CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
    );
    let close = child(
        &mut graph,
        &search,
        CurationNodeKind::Select(Selection::Similar { minimum: 0.5 }),
    );
    let waits = |node: &str| {
        CurationPath::to(&graph, node)
            .unwrap()
            .statement(PathSource::Published)
            .unwrap_err()
    };
    assert_eq!(
        waits(&folds),
        PathError::Waiting(vec!["spql.ai.breakdown_task", "spql.ai.classify"])
    );
    assert_eq!(
        waits(&folds).message(),
        "Publishing waits for spql.ai.breakdown_task and spql.ai.classify in Spiral."
    );
    assert_eq!(
        waits(&close),
        PathError::Waiting(vec!["spql.ai.breakdown_task", "spql.ai.similarity"])
    );
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
        CurationPath::to(&graph, &long)
            .unwrap()
            .statement(PathSource::Published),
        Err(PathError::Invalid(_))
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
    let mut reads = PreviewReads::default();
    reads.record(
        &detect,
        &first,
        Ok(frames(
            &first,
            &[
                (0, 20, Some(0.5)),
                (20, 40, None),
                (40, 60, Some(0.1)),
                (60, 80, Some(0.3)),
                (80, 95, Some(0.4)),
                (95, 100, Some(0.05)),
            ],
        )),
    );
    reads.record(
        &detect,
        &second,
        Ok(frames(&second, &[(0, 30, Some(0.9)), (30, 40, Some(0.9))])),
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
