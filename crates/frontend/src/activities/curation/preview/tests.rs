use super::input::PreviewInput;
use super::*;
use crate::activities::curation::annotate::{CategoryPlacement, JudgmentValue, SubtaskRow};
use crate::activities::curation::document::{
    CurationGraph, CurationNodeKind, DATASET_NODE_ID, ObjectTracking, ProgressScoring, Selection,
    SubtaskAnnotation, SubtaskCategory, SubtaskGrouping, SubtaskSearch,
};
use crate::activities::curation::path::CurationPath;
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::score_progress::Moment;
use crate::activities::episode::Episode;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

/// Two episodes of 20 and 12 seconds, whose first messages are at one second
/// of log time.
fn sample() -> EpisodeSample {
    EpisodeSample::fixture(vec![
        Episode::fixture(&EpisodeId::fixture(1), 20_000_000_000),
        Episode::fixture(&EpisodeId::fixture(2), 12_000_000_000),
    ])
}

fn graph() -> CurationGraph {
    CurationGraph::new(Some(
        DatasetSelection::new(vec!["dataset_folding".to_owned()]).unwrap(),
    ))
}

fn breakdown() -> CurationNodeKind {
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".into()));
    CurationNodeKind::BreakDownSubtasks(annotation)
}

fn searching(query: &str) -> CurationNodeKind {
    let mut search = SubtaskSearch::default();
    search.set_query(query);
    CurationNodeKind::FindSimilarSubtasks(search)
}

/// Record what `node` read of `episode` from what reaches it now, as a run
/// would.
fn record_current(
    graph: &CurationGraph,
    reads: &mut PreviewReads,
    (node, episode): (&str, &EpisodeId),
    rows: PreviewRows,
) {
    let sample = sample();
    let input = ReadInput::current(&PopulationReader::new(graph, &sample, reads), node, episode);
    reads.record(graph.node(node).unwrap(), (episode, input), Ok(rows));
}

/// The one subtask an episode's Break down found.
fn subtask(episode: u32, (start, end): (f64, f64), description: &str) -> SubtaskRow {
    SubtaskRow::new(EpisodeId::fixture(episode), (start, end), description)
        .unwrap()
        .placed_at(0)
}

#[test]
fn a_later_read_replaces_an_earlier_one_and_a_reconfigured_node_has_read_nothing() {
    let mut graph = graph();
    let annotate = graph.add_child(DATASET_NODE_ID, breakdown()).unwrap();
    let node = graph.node(&annotate).unwrap().clone();
    let episode = EpisodeId::fixture(1);
    let mut reads = PreviewReads::default();
    reads.record(
        &node,
        (&episode, ReadInput::untracked()),
        Err("timed out".into()),
    );
    assert!(reads.subtasks(&node, &episode).is_empty());
    let row = subtask(1, (0.0, 2.0), "fold it");
    reads.record(
        &node,
        (&episode, ReadInput::untracked()),
        Ok(PreviewRows::Subtasks(vec![row.clone()])),
    );
    assert_eq!(reads.subtasks(&node, &episode), std::slice::from_ref(&row));
    // Another camera: what it read answered the first.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/wrist-camera".into()));
    }
    let moved = graph.node(&annotate).unwrap().clone();
    assert!(reads.subtasks(&moved, &episode).is_empty());
    reads.forget_removed(&CurationGraph::new(None));
    assert!(reads.subtasks(&node, &episode).is_empty());
}

#[test]
fn a_recording_is_handed_the_intervals_that_reach_it_in_the_recordings_clock() {
    let mut graph = graph();
    let annotate = graph.add_child(DATASET_NODE_ID, breakdown()).unwrap();
    let (sample, reads) = (sample(), PreviewReads::default());
    let reader = PopulationReader::new(&graph, &sample, &reads);
    let input = PreviewInput::of(&graph, &annotate).unwrap();
    let rows = input.rows(&reader.input(&annotate), &sample).unwrap();
    // A whole episode, from its first message to its last, exactly.
    assert_eq!(
        rows[&EpisodeId::fixture(2)].rows(),
        ["($1, CAST(1000000000 AS BIGINT), CAST(13000000000 AS BIGINT))"]
    );
    let statement = CurationPath::to(&graph, &annotate)
        .unwrap()
        .preview(input.step_input(rows[&EpisodeId::fixture(2)].rows().iter()))
        .unwrap();
    assert!(statement.starts_with(
        "WITH\n  preview_input(episode_id, start_ns, end_ns) AS (VALUES ($1, CAST(1000000000 AS BIGINT), CAST(13000000000 AS BIGINT))),"
    ));
}

#[test]
fn below_score_progress_a_recording_is_handed_the_stretches_it_keeps() {
    let mut graph = graph();
    let score = graph
        .add_keeping(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
            Selection::Progressing { minimum: 0.2 },
        )
        .unwrap();
    let annotate = graph.add_child(&score, breakdown()).unwrap();
    let episode = EpisodeId::fixture(1);
    let moments = [0.5, 0.4, 0.1, 0.3]
        .into_iter()
        .enumerate()
        .map(|(second, rate)| {
            Moment::new(
                episode.clone(),
                (second as f64, second as f64 + 1.0),
                Some(rate),
            )
            .unwrap()
        })
        .collect();
    let mut reads = PreviewReads::default();
    record_current(
        &graph,
        &mut reads,
        (&score, &episode),
        PreviewRows::Moments(moments),
    );
    let sample = sample();
    let reader = PopulationReader::new(&graph, &sample, &reads);
    let rows = PreviewInput::of(&graph, &annotate)
        .unwrap()
        .rows(&reader.input(&annotate), &sample)
        .unwrap();
    // Seconds 0–2 and 3–4 progress, as the node's own rule joins them.
    assert_eq!(
        rows[&episode].rows(),
        [
            "($1, CAST(1000000000 AS BIGINT), CAST(3000000000 AS BIGINT))",
            "($1, CAST(4000000000 AS BIGINT), CAST(5000000000 AS BIGINT))"
        ]
    );
    // A rule every frame passes hands on the scored recording as one stretch,
    // never a row per frame.
    graph
        .set_rule(&score, Selection::Progressing { minimum: -1.0 })
        .unwrap();
    let reader = PopulationReader::new(&graph, &sample, &reads);
    let rows = PreviewInput::of(&graph, &annotate)
        .unwrap()
        .rows(&reader.input(&annotate), &sample)
        .unwrap();
    assert_eq!(
        rows[&episode].rows(),
        ["($1, CAST(1000000000 AS BIGINT), CAST(5000000000 AS BIGINT))"]
    );
}

#[test]
fn a_node_below_a_changed_one_is_stale_and_reads_again_only_what_changed() {
    let mut graph = graph();
    let annotate = graph.add_child(DATASET_NODE_ID, breakdown()).unwrap();
    let search = graph
        .add_child(&annotate, searching("fold the towel"))
        .unwrap();
    let (first, second) = (EpisodeId::fixture(1), EpisodeId::fixture(2));
    let mut reads = PreviewReads::default();
    for (episode, description) in [(1, "fold the towel"), (2, "grasp the corner")] {
        record_current(
            &graph,
            &mut reads,
            (&annotate, &EpisodeId::fixture(episode)),
            PreviewRows::Subtasks(vec![subtask(episode, (0.0, 4.0), description)]),
        );
    }
    let judged = |episode: u32, description: &str| {
        subtask(episode, (0.0, 4.0), description)
            .judged(SubtaskRow::SIMILARITY, JudgmentValue::Similarity(Some(0.9)))
    };
    for (episode, description) in [(1, "fold the towel"), (2, "grasp the corner")] {
        record_current(
            &graph,
            &mut reads,
            (&search, &EpisodeId::fixture(episode)),
            PreviewRows::Subtasks(vec![judged(episode, description)]),
        );
    }
    let sample = sample();
    let progress = |reads: &PreviewReads, graph: &CurationGraph, node: &str| {
        PreviewProgress::of(&PopulationReader::new(graph, &sample, reads), node)
    };
    assert!(!progress(&reads, &graph, &search).is_stale());
    assert_eq!(
        progress(&reads, &graph, &search).read(),
        [first.clone(), second.clone()]
    );

    // Break down runs again on one episode and answers differently: the
    // search below is stale there, and only there.
    record_current(
        &graph,
        &mut reads,
        (&annotate, &second),
        PreviewRows::Subtasks(vec![subtask(2, (0.0, 3.0), "pinch the corner")]),
    );
    let stale = progress(&reads, &graph, &search);
    assert!(stale.is_stale());
    assert_eq!(stale.pending(), std::slice::from_ref(&second));
    assert_eq!(stale.read(), std::slice::from_ref(&first));

    // Another camera for Break down leaves it having read nothing, and the
    // search below it stale.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/wrist-camera".into()));
    }
    assert_eq!(
        progress(&reads, &graph, &annotate).pending(),
        [first.clone(), second.clone()]
    );
    assert!(progress(&reads, &graph, &search).is_stale());
}

#[test]
fn every_measurement_previews_what_it_has_the_backend_answer_and_store_first() {
    use spiral_robotics_common::curation::{AnswerSource, FillEpisodes};
    let mut graph = graph();
    let score = graph
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
        )
        .unwrap();
    let mut tracking = ObjectTracking::default();
    tracking.set_camera(Some("/top-camera".to_owned()));
    tracking.set_prompt("bottle");
    let track = graph
        .add_child(DATASET_NODE_ID, CurationNodeKind::TrackObjects(tracking))
        .unwrap();
    let annotate = graph.add_child(DATASET_NODE_ID, breakdown()).unwrap();
    let grouping = SubtaskGrouping::of(vec![
        SubtaskCategory::new("fold", "Folding cloth.").unwrap(),
    ]);
    let group = graph
        .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks(grouping))
        .unwrap();
    let episodes = [EpisodeId::fixture(1), EpisodeId::fixture(2)];
    let sample = sample();
    let mut reads = PreviewReads::default();
    for (fixture, episode) in (1..).zip(&episodes) {
        record_current(
            &graph,
            &mut reads,
            (&annotate, episode),
            PreviewRows::Subtasks(vec![subtask(fixture, (0.0, 3.0), "grasp")]),
        );
    }
    let reader = PopulationReader::new(&graph, &sample, &reads);
    for node in [&score, &track, &annotate, &group] {
        let plan = PreviewPlan::of(&reader, node, &episodes).unwrap();
        let fill = plan.fill();
        assert_eq!(fill.episodes(), &FillEpisodes::Listed(episodes.to_vec()));
        // Each episode's preview then reads what was stored, and asks nothing.
        assert_eq!(plan.statements().len(), 2);
        for statement in plan.statements() {
            assert!(statement.contains(&format!("a.{}.value", fill.column())));
            assert!(!statement.contains("spql.ai."), "{statement}");
        }
        match fill.source() {
            AnswerSource::ProgressService { .. } => assert_eq!(node, &score),
            AnswerSource::ObjectTracking { camera, prompt, .. } => {
                assert_eq!(node, &track);
                assert_eq!(
                    (camera.as_str(), prompt.as_str()),
                    ("/top-camera", "bottle")
                );
            }
            AnswerSource::Statement { statement } if node == &group => {
                assert!(
                    statement.contains("spql.ai.classify(s.listed.description"),
                    "{statement}"
                )
            }
            AnswerSource::Statement { statement } => {
                assert!(statement.contains("spql.ai.breakdown_task("), "{statement}")
            }
        }
    }
    let _ = CategoryPlacement::new("fold", 0.5).unwrap();
}
