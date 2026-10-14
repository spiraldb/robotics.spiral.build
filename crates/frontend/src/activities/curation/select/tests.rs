//! A Select's two readings keep the same rows: what the browser keeps over
//! the preview's reads is what Spiral's engine answered for the compiled
//! statement over the same answers (`spql.list.collect` folding `kept`,
//! `RANK` and `COUNT` windows ranking), run with literal stand-ins for the
//! listing and the answers table.
use crate::activities::curation::annotate::{CategoryPlacement, JudgmentValue, SubtaskRow};
use crate::activities::curation::document::{
    Comparison, CurationGraph, CurationNodeKind, DATASET_NODE_ID, EpisodeCondition, EpisodeNumber,
    ObjectTracking, ProgressScoring, SelectCondition, Selection, SubtaskAnnotation,
    SubtaskCategory, SubtaskGrouping, SubtaskSearch,
};
use crate::activities::curation::measured_timeline::{MeasuredTimeline, TimelineEntry};
use crate::activities::curation::path::CurationPath;
use crate::activities::curation::population::{EpisodeSample, KeptRanges, PopulationReader};
use crate::activities::curation::preview::{PreviewReads, PreviewRows};
use crate::activities::episode::Episode;
use spiral_robotics_common::datasets::EpisodeId;

/// Four episodes of 30, 10, 25, and 20 seconds, each first message at one
/// second of log time.
fn sample() -> EpisodeSample {
    EpisodeSample::fixture(
        [30, 10, 25, 20]
            .into_iter()
            .zip(1..)
            .map(|(seconds, ordinal)| {
                Episode::fixture(&EpisodeId::fixture(ordinal), seconds * 1_000_000_000)
            })
            .collect(),
    )
}

fn graph() -> CurationGraph {
    CurationGraph::new(Some(
        crate::activities::episode::EpisodeListing::mcap_fixture("bottles"),
    ))
}

fn add(graph: &mut CurationGraph, parent: &str, kind: CurationNodeKind) -> String {
    graph.add_child(parent, kind).unwrap()
}

/// A Select after `parent` keeping by `conditions`.
fn select(graph: &mut CurationGraph, parent: &str, conditions: Vec<SelectCondition>) -> String {
    let id = add(
        graph,
        parent,
        CurationNodeKind::Select(Selection::keeping_everything()),
    );
    graph.set_selection(&id, Selection::of(conditions)).unwrap();
    id
}

/// `values` one second apart from the episode's start.
fn timeline(values: &[Option<f64>]) -> PreviewRows {
    PreviewRows::Timeline(MeasuredTimeline::new(
        values
            .iter()
            .enumerate()
            .map(|(second, value)| TimelineEntry::new(second as f64, *value))
            .collect(),
    ))
}

/// What `node` read of each episode, under the column it stores in.
fn record(
    graph: &CurationGraph,
    reads: &mut PreviewReads,
    node: &str,
    read: Vec<(u32, PreviewRows)>,
) {
    let column = CurationPath::to(graph, node)
        .unwrap()
        .answer_column()
        .unwrap();
    for (ordinal, rows) in read {
        reads.record((node, &column), &EpisodeId::fixture(ordinal), Ok(rows));
    }
}

/// What the Select keeps, as `(episode, [(start, end)])` in whole seconds.
fn kept(graph: &CurationGraph, reads: &PreviewReads, node: &str) -> Vec<(u32, Vec<(f64, f64)>)> {
    let sample = sample();
    PopulationReader::new(graph, &sample, reads)
        .of(node)
        .kept_episodes()
        .iter()
        .map(|kept| {
            let ordinal = (1..=4)
                .find(|ordinal| &EpisodeId::fixture(*ordinal) == kept.episode())
                .unwrap();
            (ordinal, kept.kept().ranges().to_vec())
        })
        .collect()
}

fn tracking() -> CurationNodeKind {
    let mut tracking = ObjectTracking::default();
    tracking.set_camera(Some("/top-camera".to_owned()));
    tracking.set_prompt("bottle");
    CurationNodeKind::TrackObjects(tracking)
}

/// Bottles one picture a second: the most of them 3, 2, none, and 4, so
/// their durations per bottle are 10, 5, unknown, and 5 seconds.
fn counted(graph: &CurationGraph, reads: &mut PreviewReads, track: &str) {
    record(
        graph,
        reads,
        track,
        vec![
            (1, timeline(&[Some(2.0), Some(3.0), Some(3.0), Some(1.0)])),
            (2, timeline(&[Some(2.0), Some(2.0)])),
            (3, timeline(&[Some(0.0), Some(0.0)])),
            (4, timeline(&[Some(4.0)])),
        ],
    );
}

fn per_bottle(track: &str, comparison: Comparison, value: f64) -> SelectCondition {
    SelectCondition::Episodes(EpisodeCondition::new(
        EpisodeNumber::DurationPer(track.to_owned()),
        comparison,
        value,
    ))
}

#[test]
fn the_lowest_share_by_duration_per_bottle_ranks_the_episodes_that_have_one() {
    let mut graph = graph();
    let track = add(&mut graph, DATASET_NODE_ID, tracking());
    let mut reads = PreviewReads::default();
    counted(&graph, &mut reads, &track);
    let whole = |seconds: f64| vec![(0.0, seconds)];
    // Ranked among the three that have a number (5, 5, 10): the two at 5
    // share rank 0, and 10 is at 100%. An episode tracking no bottle has no
    // number, and so is never kept.
    let fastest = select(
        &mut graph,
        &track,
        vec![per_bottle(&track, Comparison::Lowest, 0.25)],
    );
    assert_eq!(
        kept(&graph, &reads, &fastest),
        [(2, whole(10.0)), (4, whole(20.0))]
    );
    let slowest = select(
        &mut graph,
        &track,
        vec![per_bottle(&track, Comparison::Highest, 0.25)],
    );
    assert_eq!(kept(&graph, &reads, &slowest), [(1, whole(30.0))]);
    // Highest half: 10 ranks 0 from the top and both 5s rank 50%.
    let half = select(
        &mut graph,
        &track,
        vec![per_bottle(&track, Comparison::Highest, 0.5)],
    );
    assert_eq!(
        kept(&graph, &reads, &half),
        [(1, whole(30.0)), (2, whole(10.0)), (4, whole(20.0))]
    );
    // Compared with a value, as many as hold.
    let crowded = select(
        &mut graph,
        &track,
        vec![SelectCondition::Episodes(EpisodeCondition::new(
            EpisodeNumber::MostAtOnce(track.clone()),
            Comparison::AtLeast,
            3.0,
        ))],
    );
    assert_eq!(
        kept(&graph, &reads, &crowded),
        [(1, whole(30.0)), (4, whole(20.0))]
    );
    // A share of one episode ranks it 0, kept by any share.
    let numbers = [Some(7.0), None];
    let lone = EpisodeCondition::new(EpisodeNumber::MostAtOnce(track), Comparison::Lowest, 0.0);
    assert_eq!(lone.holds(&numbers), [true, false]);
}

#[test]
fn range_conditions_narrow_kept_one_after_another_and_drop_what_has_nothing_left() {
    let mut graph = graph();
    let score = add(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::ScoreProgress(ProgressScoring::default()),
    );
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".to_owned()));
    let annotate = add(
        &mut graph,
        &score,
        CurationNodeKind::BreakDownSubtasks(annotation),
    );
    let mut query = SubtaskSearch::default();
    query.set_query("fold the towel");
    let search = add(
        &mut graph,
        &annotate,
        CurationNodeKind::FindSimilarSubtasks(query),
    );
    let mut reads = PreviewReads::default();
    // Episode 1 progresses but for a stalled second and an unread one; the
    // second episode never does.
    record(
        &graph,
        &mut reads,
        &score,
        vec![
            (
                1,
                timeline(&[Some(0.5), None, Some(0.1), Some(0.3), Some(0.4)]),
            ),
            (2, timeline(&[Some(0.0), Some(0.1)])),
        ],
    );
    let similar = |ordinal: u32, (start, end): (f64, f64), similarity: f64| {
        SubtaskRow::new(EpisodeId::fixture(ordinal), (start, end), "a subtask")
            .unwrap()
            .judged(JudgmentValue::Similarity(Some(similarity)))
    };
    record(
        &graph,
        &mut reads,
        &search,
        vec![
            (
                1,
                PreviewRows::Subtasks(vec![
                    similar(1, (0.0, 4.0), 0.9),
                    similar(1, (4.0, 8.0), 0.3),
                    similar(1, (8.0, 30.0), 0.8),
                ]),
            ),
            (
                2,
                PreviewRows::Subtasks(vec![similar(2, (0.0, 10.0), 0.95)]),
            ),
        ],
    );
    let both = select(
        &mut graph,
        &search,
        vec![
            SelectCondition::Progressing {
                node: score.clone(),
                minimum: 0.2,
            },
            SelectCondition::Similar {
                node: search.clone(),
                minimum: 0.5,
            },
        ],
    );
    // Progressing keeps [0, 2) and [3, 5); the similar subtasks [0, 4) and
    // [8, 30). Episode 2 progresses nowhere, so nothing of it is left, and
    // episodes 3 and 4 were never scored.
    assert_eq!(
        kept(&graph, &reads, &both),
        [(1, vec![(0.0, 2.0), (3.0, 4.0)])]
    );
    let sample = sample();
    let reader = PopulationReader::new(&graph, &sample, &reads);
    let CurationNodeKind::Select(selection) = graph.node(&both).unwrap().kind() else {
        panic!("a Select");
    };
    let reading = selection.read(reader.input(&both), &reader);
    // Alone, progress keeps 4 of episode 1's 30 seconds, and similarity 26
    // of its 30 and all 10 of the second's.
    assert_eq!(reading.seconds_kept_by(0), Some(4.0));
    assert_eq!(reading.seconds_kept_by(1), Some(36.0));
    assert_eq!(reading.leaves_out(0), 3);
    let reached = KeptRanges::whole(30.0);
    assert_eq!(
        reached.without(reading.output().of(&EpisodeId::fixture(1)).unwrap()),
        KeptRanges::whole(30.0).within(&[(2.0, 3.0), (4.0, 30.0)])
    );
}

#[test]
fn chosen_categories_keep_the_subtasks_a_grouping_put_in_them() {
    let mut graph = graph();
    let mut annotation = SubtaskAnnotation::default();
    annotation.set_camera(Some("/top-camera".to_owned()));
    let annotate = add(
        &mut graph,
        DATASET_NODE_ID,
        CurationNodeKind::BreakDownSubtasks(annotation),
    );
    let group = add(
        &mut graph,
        &annotate,
        CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::of(vec![
            SubtaskCategory::new("fold", "Folding.").unwrap(),
            SubtaskCategory::new("grasp", "Grasping.").unwrap(),
        ])),
    );
    let placed = |ordinal: u32, (start, end): (f64, f64), category: &str| {
        SubtaskRow::new(EpisodeId::fixture(ordinal), (start, end), "a subtask")
            .unwrap()
            .judged(JudgmentValue::Category(Some(
                CategoryPlacement::new(category, 0.8).unwrap(),
            )))
    };
    let mut reads = PreviewReads::default();
    record(
        &graph,
        &mut reads,
        &group,
        vec![
            (
                1,
                PreviewRows::Subtasks(vec![
                    placed(1, (0.0, 4.0), "grasp"),
                    placed(1, (4.0, 8.0), "grasp"),
                    placed(1, (8.0, 30.0), "fold"),
                ]),
            ),
            (
                2,
                PreviewRows::Subtasks(vec![placed(2, (0.0, 10.0), "fold")]),
            ),
        ],
    );
    let CurationNodeKind::GroupSimilarSubtasks(grouping) = graph.node(&group).unwrap().kind()
    else {
        panic!("a grouping");
    };
    let folds = SelectCondition::InCategories {
        node: group.clone(),
        categories: vec![grouping.categories()[0].id()],
    };
    let kept_folds = select(&mut graph, &group, vec![folds]);
    assert_eq!(
        kept(&graph, &reads, &kept_folds),
        [(1, vec![(8.0, 30.0)]), (2, vec![(0.0, 10.0)])]
    );
}

#[test]
fn kept_ranges_that_overlap_or_touch_are_one() {
    // Two subtasks back to back, two that overlap, and one apart.
    let kept = KeptRanges::whole(30.0).within(&[
        (10.0, 12.0),
        (4.0, 8.0),
        (0.0, 4.0),
        (11.0, 15.0),
        (20.0, 21.0),
    ]);
    assert_eq!(kept.ranges(), &[(0.0, 8.0), (10.0, 15.0), (20.0, 21.0)]);
    assert_eq!(kept.seconds(), 14.0);
}
