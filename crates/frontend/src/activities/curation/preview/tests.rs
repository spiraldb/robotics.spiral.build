use super::*;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{
    Comparison, CurationGraph, CurationNodeKind, DATASET_NODE_ID, EpisodeFact, ObjectTracking,
    ProgressScoring, Selection, SubtaskAnnotation, SubtaskCategory, SubtaskGrouping,
};
use crate::activities::curation::path::CurationPath;
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::episode::Episode;
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource, FillEpisodes};
use spiral_robotics_common::datasets::EpisodeId;

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
        crate::activities::episode::EpisodeListing::mcap_fixture("folding"),
    ))
}

fn breakdown() -> CurationNodeKind {
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".into()));
    CurationNodeKind::BreakDownSubtasks(annotation)
}

fn column(graph: &CurationGraph, node: &str) -> AnswerColumn {
    CurationPath::to(graph, node)
        .unwrap()
        .answer_column()
        .unwrap()
}

fn subtask(episode: u32, description: &str) -> SubtaskRow {
    SubtaskRow::new(EpisodeId::fixture(episode), (0.0, 2.0), description).unwrap()
}

#[test]
fn a_later_read_replaces_an_earlier_one_and_a_reconfigured_node_has_read_nothing() {
    let mut graph = graph();
    let annotate = graph.add_child(DATASET_NODE_ID, breakdown()).unwrap();
    let stored = column(&graph, &annotate);
    let episode = EpisodeId::fixture(1);
    let mut reads = PreviewReads::default();
    reads.record((&annotate, &stored), &episode, Err("timed out".into()));
    assert!(reads.subtasks((&annotate, &stored), &episode).is_empty());
    let row = subtask(1, "fold it");
    reads.record(
        (&annotate, &stored),
        &episode,
        Ok(PreviewRows::Subtasks(vec![row.clone()])),
    );
    assert_eq!(
        reads.subtasks((&annotate, &stored), &episode),
        std::slice::from_ref(&row)
    );
    // Another camera is another call: what it read answered the first.
    if let CurationNodeKind::BreakDownSubtasks(annotation) =
        graph.node_mut(&annotate).unwrap().kind_mut()
    {
        annotation.set_camera(Some("/wrist-camera".into()));
    }
    let moved = column(&graph, &annotate);
    assert!(reads.subtasks((&annotate, &moved), &episode).is_empty());
    reads.forget_removed(&CurationGraph::new(None));
    assert!(reads.subtasks((&annotate, &stored), &episode).is_empty());
}

#[test]
fn a_node_reads_the_episodes_that_reach_it_and_nothing_goes_stale_above() {
    let mut graph = graph();
    let long = graph
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::Select(Selection::keeping_everything()),
        )
        .unwrap();
    let annotate = graph.add_child(&long, breakdown()).unwrap();
    let stored = column(&graph, &annotate);
    let (first, second) = (EpisodeId::fixture(1), EpisodeId::fixture(2));
    let mut reads = PreviewReads::default();
    reads.record(
        (&annotate, &stored),
        &first,
        Ok(PreviewRows::Subtasks(vec![subtask(1, "fold")])),
    );
    let sample = sample();
    let progress = |graph: &CurationGraph, reads: &PreviewReads| {
        PreviewProgress::of(&PopulationReader::new(graph, &sample, reads), &annotate)
    };
    let both = progress(&graph, &reads);
    assert_eq!(both.read(), std::slice::from_ref(&first));
    assert_eq!(both.pending(), std::slice::from_ref(&second));
    assert!(!both.is_unmeasured());
    // A Select above that leaves the second episode out leaves nothing for
    // the node to read; what it read of the first is still current.
    graph
        .set_selection(
            &long,
            Selection::episodes_where(EpisodeFact::DurationSeconds, Comparison::AtLeast, 15.0),
        )
        .unwrap();
    let narrowed = progress(&graph, &reads);
    assert_eq!(narrowed.read(), std::slice::from_ref(&first));
    assert!(narrowed.pending().is_empty());
    reads.record(
        (&annotate, &stored),
        &first,
        Err("the model gave up".into()),
    );
    assert_eq!(
        progress(&graph, &reads).failed(),
        [(first, "the model gave up".to_owned())]
    );
}

#[test]
fn every_model_node_previews_what_it_has_the_backend_answer_and_store_first() {
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
    let (sample, reads) = (sample(), PreviewReads::default());
    let reader = PopulationReader::new(&graph, &sample, &reads);
    for node in [&score, &track, &annotate, &group] {
        let plan = PreviewPlan::of(&reader, node, &episodes).unwrap();
        let fill = plan.fill();
        assert_eq!(fill.episodes(), &FillEpisodes::Listed(episodes.to_vec()));
        // One read then reads what was stored of both, and asks nothing.
        let statement = plan.statement();
        let stored = graph.listing().unwrap().dataset().answers(fill.column());
        assert!(statement.contains(&format!("LEFT JOIN {stored} ")));
        assert!(statement.contains("WHERE e.id IN ($1, $2))"));
        match fill.source() {
            AnswerSource::ProgressService { .. } => assert_eq!(node, &score),
            AnswerSource::ObjectTracking {
                camera,
                prompt,
                rate,
                ..
            } => {
                assert_eq!(node, &track);
                assert_eq!(
                    (camera.as_str(), prompt.as_str(), *rate),
                    ("/top-camera", "bottle", 5)
                );
            }
            AnswerSource::BreakDown(call) => {
                assert_eq!(node, &annotate);
                assert_eq!(call.camera(), "/top-camera");
            }
            AnswerSource::Classify(call) => {
                assert_eq!(node, &group);
                assert_eq!(call.subtasks(), &column(&graph, &annotate));
                assert_eq!(call.categories()[0].name(), "fold");
            }
            AnswerSource::Similarity(_) => panic!("No node here finds similar subtasks"),
            AnswerSource::DatasetStatement { .. } => {
                panic!("A preview asks about its listed episodes, one at a time")
            }
        }
    }
    // A Select asks nothing, so it has no preview.
    let select = graph
        .add_child(
            DATASET_NODE_ID,
            CurationNodeKind::Select(Selection::keeping_everything()),
        )
        .unwrap();
    let reader = PopulationReader::new(&graph, &sample, &reads);
    assert!(PreviewPlan::of(&reader, &select, &episodes).is_err());
}
