//! Find similar subtasks: the node's step asks `spql.ai.similarity` how likely
//! every subtask that reaches it is the operation its query names, and its
//! preview runs that step over the subtasks Break down's preview holds, many
//! episodes to a query, as the curation's own run.
//!
//! Every subtask is listed with its probability, best first, divided where
//! the node's cutoff falls: the node hands on the subtasks at least that
//! likely, and the cutoff slides on the strip over the ranking without asking
//! anything again. Asking a question saves it as the node's query.
use super::coverage::{CoverageMeasure, EpisodeCoverage, SubtaskList};
use super::source::SubtaskSource;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{CurationNodeKind, Selection};
use crate::activities::curation::episode_map::{
    EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, MapReveal, SegmentPaint,
};
use crate::activities::curation::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use crate::activities::curation::population::Slice;
use crate::activities::curation::review::{
    Counted, FailedEpisodes, IndexBar, NodeBar, NodeNotes, NodeReach, RowStanding, SubtaskEntry,
    SubtaskEvidence, SubtaskIntervals, SubtaskReview, TabStop,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus, RunState};
use crate::activities::curation::threshold::{AxisUnit, StripDot, Threshold, ThresholdStrip};
use crate::activities::curation::thumbnail::{Thumbnail, ThumbnailMoment, Thumbnails};
use crate::activities::episode::Episode;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Motion, Notice, NoticeTone, Place, TextInput,
    TextInputKind,
};

/// What a search lists: once it has answered, the subtasks it judged; before
/// that, the subtasks that reach it, in their own order.
#[derive(Clone, PartialEq)]
struct SearchTable {
    rows: Vec<SubtaskRow>,
    /// Each row's probability, in table order, once the search has answered
    /// for any of them; `None` for a row the model could not answer.
    answers: Option<Vec<Option<f64>>>,
}

impl SearchTable {
    fn of(source: &SubtaskSource) -> Self {
        let judged = source.judged();
        if judged.is_empty() {
            return Self {
                rows: source.reaching().to_vec(),
                answers: None,
            };
        }
        Self {
            rows: judged.to_vec(),
            answers: Some(judged.iter().map(SubtaskRow::similarity).collect()),
        }
    }
    /// The row the question was taken from, when this table holds it.
    fn about(&self, source: &SubtaskSource) -> Option<usize> {
        let asked = source.asked_about()?;
        self.rows
            .iter()
            .position(|row| row.is(asked.episode(), asked.start_time()))
    }
    /// The rows to show, best first, each with its probability; rows that tie,
    /// and rows without an answer, keep their table order.
    fn ranked(&self, chosen: Option<usize>) -> Vec<(usize, RowStanding)> {
        let Some(answers) = &self.answers else {
            return (0..self.rows.len())
                .map(|row| (row, RowStanding::plain(chosen == Some(row))))
                .collect();
        };
        let mut order = (0..self.rows.len()).collect::<Vec<_>>();
        let rank = |row: usize| answers[row].unwrap_or(f64::NEG_INFINITY);
        order.sort_by(|left, right| rank(*right).total_cmp(&rank(*left)));
        order
            .into_iter()
            .map(|row| {
                let standing = RowStanding::scored(
                    answers[row].unwrap_or_default(),
                    answers[row].is_some(),
                    chosen == Some(row),
                );
                (row, standing)
            })
            .collect()
    }
    /// The best-scoring row other than the one asked about, once there is one.
    fn top(&self, about: Option<usize>) -> Option<&SubtaskRow> {
        self.ranked(None)
            .into_iter()
            .map(|(row, _)| row)
            .find(|row| Some(*row) != about)
            .filter(|_| self.answers.is_some())
            .and_then(|row| self.rows.get(row))
    }
    /// Which episodes the answers reached at the cutoff. The row asked about
    /// is the question, and finding it the same as itself says nothing about
    /// its episode, so it is not counted.
    fn coverage(
        &self,
        source: &SubtaskSource,
        (about, cutoff): (Option<usize>, f64),
    ) -> Option<EpisodeCoverage> {
        let answers = self.answers.as_ref()?;
        Some(EpisodeCoverage::of(
            &source.measured(),
            self.rows
                .iter()
                .zip(answers)
                .enumerate()
                .filter(|(row, _)| Some(*row) != about)
                .filter_map(|(_, (row, answer))| Some((row.episode(), (*answer)?))),
            CoverageMeasure::BestMatch { at_least: cutoff },
        ))
    }
    /// Every answered row as a dot at its probability, but the row asked
    /// about, which is the question; kept in ink at or above the cutoff.
    fn dots(&self, (about, cutoff): (Option<usize>, f64)) -> Vec<StripDot> {
        let answers = self
            .answers
            .iter()
            .flatten()
            .enumerate()
            .map(|(row, answer)| answer.filter(|_| Some(row) != about))
            .collect::<Vec<_>>();
        let kept = answers
            .iter()
            .map(|answer| answer.is_some_and(|probability| probability >= cutoff))
            .collect::<Vec<_>>();
        StripDot::of(&answers, &kept)
    }
}

/// The workspace shown for a selected Find similar subtasks node.
#[component]
pub(in crate::activities::curation) fn SearchWorkspace(
    controller: CurationController,
    node: String,
) -> Element {
    let source = SubtaskSource::of(controller, &node);
    let spans = controller.sample().as_ref().map(EpisodeSpans::of);
    let saved = source.query().trim().to_owned();
    let mut query = use_signal(|| saved.clone());
    // A question asked from a row elsewhere replaces the saved one, and the
    // box follows it.
    use_effect(use_reactive((&saved,), move |(saved,)| query.set(saved)));
    // The results a click opens, one tab per episode; the row chosen is the
    // one playing in the tab in front.
    let tabs = OpenEpisodes::use_open();
    let mut problem = use_signal(|| Option::<String>::None);
    // Bumped whenever a question is asked, so the map lights up again for
    // every new question and never for a click.
    let mut generation = use_signal(|| 0_u32);
    let run = controller.run_state(&node);
    let running = run != RunState::Idle;

    // Asking saves the question as the node's query, then runs the node's
    // step over the subtasks that reach it, as few queries as they fit. The
    // run is the curation's: choosing another node leaves it asking.
    let asked_node = node.clone();
    let search = use_callback(move |()| {
        let text = query.peek().trim().to_owned();
        if text.is_empty() || controller.run_state(&asked_node) != RunState::Idle {
            return;
        }
        let asked = controller.edit(|payload| {
            match payload.node_mut(&asked_node).map(|node| node.kind_mut()) {
                Some(CurationNodeKind::FindSimilarSubtasks(search)) => {
                    search.set_query(text.clone());
                    Ok(())
                }
                _ => Err("This node no longer searches subtasks.".to_owned()),
            }
        });
        // What it has not answered from these subtasks, and what failed.
        let pending = controller
            .progress(&asked_node)
            .map(|progress| {
                let mut pending = progress.pending();
                pending.extend(progress.failed().iter().map(|(episode, _)| episode.clone()));
                pending
            })
            .unwrap_or_default();
        generation += 1;
        problem.set(
            asked
                .and_then(|()| controller.run_preview(&asked_node, &pending))
                .err(),
        );
    });
    // Asking costs one judgment of each subtask, so a saved question the
    // node has not answered from the subtasks that reach it now is asked at
    // once: after Find similar on a row, and on opening a search whose
    // answer is gone or out of date. A search whose last answer failed
    // waits for Search.
    let unanswered = node.clone();
    use_effect(move || {
        let Some(progress) = controller.progress(&unanswered) else {
            return;
        };
        let asking = controller.with_payload(|payload| {
            matches!(
                payload.node(&unanswered).map(|node| node.kind()),
                Some(CurationNodeKind::FindSimilarSubtasks(search)) if !search.query().trim().is_empty()
            )
        });
        let pending = progress.pending();
        if asking
            && !pending.is_empty()
            && progress.failed().is_empty()
            && controller.run_state(&unanswered) == RunState::Idle
        {
            generation += 1;
            problem.set(controller.run_preview(&unanswered, &pending).err());
        }
    });

    // A row that asked from elsewhere also left where it was, so its thumbnail
    // is carried from there into the question it became.
    use_effect(move || {
        let Some(from) = controller.take_launch() else {
            return;
        };
        spawn(async move {
            Motion::after_render().await;
            if let Some(thumbnail) = SubtaskList::RANKED
                .element()
                .and_then(|list| list.closest(".curation-similar-search").ok().flatten())
                .and_then(|workspace| {
                    workspace
                        .query_selector(".curation-similar-asked .curation-thumbnail")
                        .ok()
                        .flatten()
                })
            {
                Motion::arrive_from(&thumbnail, from);
            }
        });
    });

    let table = SearchTable::of(&source);
    let about = table.about(&source);
    // How close a subtask must be to be kept: the node's own rule.
    let rule = controller
        .with_payload(|payload| payload.node(&node).and_then(|node| node.keeps()).cloned());
    let cutoff = rule
        .as_ref()
        .and_then(Selection::minimum)
        .unwrap_or_default();
    // The best answer opens as soon as there is one, so the question's result
    // is on screen, not a click away.
    let top = table.top(about).map(|row| {
        (
            row.episode().clone(),
            Some((row.start_time(), row.end_time())),
        )
    });
    use_effect(use_reactive((&top,), move |(top,)| tabs.open_first(top)));

    let Some(annotation) = source.annotation().map(str::to_owned) else {
        return rsx! {
            section { class: "curation-similar-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This search is not connected to a Break down subtasks node."
                }
            }
        };
    };
    let rows = table.rows.clone();
    let context = source.context();
    let measured = source.measured();
    let thumbnails = Thumbnails::use_camera(controller, context.camera());
    let slices = rows.iter().map(Slice::of_subtask).collect::<Vec<_>>();
    let lanes = spans
        .as_ref()
        .map(|spans| EpisodeLanes::of(measured.episodes(), &slices, spans))
        .unwrap_or_default();
    let chosen = tabs.front().and_then(|tab| {
        rows.iter()
            .position(|row| tab.holds(row.episode(), row.start_time()))
    });
    let choose = {
        let rows = rows.clone();
        move |row: usize| {
            if let Some(row) = rows.get(row) {
                tabs.open(row.episode(), Some((row.start_time(), row.end_time())));
            }
        }
    };
    // The row asked about is the question, not one of its answers: it would
    // only rank itself first.
    let listed = table
        .ranked(chosen)
        .into_iter()
        .filter(|(row, _)| Some(*row) != about)
        .collect::<Vec<_>>();
    let answered = table.answers.clone();
    let summary = table.coverage(&source, (about, cutoff)).map_or_else(
        || {
            format!(
                "{} across {}",
                Counted::subtasks(rows.len()),
                Counted::episodes(measured.len())
            )
        },
        |coverage| coverage.summary(),
    );
    let progress = running
        .then(|| controller.activity().get(&node).cloned())
        .flatten();
    let paint = MapPaint::new(
        answered
            .clone()
            .map(|answers| {
                answers
                    .iter()
                    .map(|answer| answer.map_or(SegmentPaint::Unasked, SegmentPaint::Match))
                    .collect()
            })
            .unwrap_or_default(),
        chosen,
    )
    .asked_about(about)
    .revealing(
        if running && answered.is_none() {
            MapReveal::Reading
        } else {
            MapReveal::Ripple { origin: about }
        },
        generation(),
    );
    let failed = source.progress().failed().len();
    let reason = source
        .progress()
        .failed()
        .first()
        .map(|(_, reason)| reason.clone());
    let stale = source
        .progress()
        .is_stale()
        .then(|| "Out of date: the subtasks that reach this node changed since it ran".to_owned());
    let pending = source.progress().pending().len();
    // An empty box asks nothing, and a question every episode has answered
    // is already on screen.
    let typed = Some(query().trim().to_owned()).filter(|text| !text.is_empty());
    let answered_already = typed.as_deref() == Some(saved.as_str()) && pending == 0 && failed == 0;
    let asked_row = source.asked_about().cloned();
    let places = listed.iter().map(|(row, _)| *row).collect::<Vec<_>>();
    let listed_count = places.len();
    let chosen_place = chosen.and_then(|chosen| places.iter().position(|row| *row == chosen));
    // The first listed row an answered search puts below its cutoff; a
    // ranking whose every row is below it, or none, draws no divider.
    let cut = answered.as_ref().and_then(|answers| {
        places
            .iter()
            .position(|row| {
                answers
                    .get(*row)
                    .copied()
                    .flatten()
                    .is_none_or(|probability| probability < cutoff)
            })
            .filter(|cut| *cut > 0)
    });
    // What the cutoff keeps, of the rows answered other than the question.
    let kept = listed
        .iter()
        .filter(|(row, _)| {
            answered
                .as_ref()
                .and_then(|answers| answers.get(*row).copied().flatten())
                .is_some_and(|probability| probability >= cutoff)
        })
        .count();
    let dots = table.dots((about, cutoff));
    let threshold = Threshold::over(&dots, cutoff, AxisUnit::Percent, true);
    let find = {
        let annotation = annotation.clone();
        move |row: SubtaskRow, from: Option<Place>| controller.find_similar(&annotation, &row, from)
    };
    rsx! {
        section {
            class: "curation-similar-workspace curation-similar-search",
            "aria-label": "Find similar subtasks",
            onpointerdown: move |_| {
                if let Some(workspace) = SubtaskList::RANKED
                    .element()
                    .and_then(|list| list.closest(".curation-similar-search").ok().flatten())
                {
                    Motion::finish_within(&workspace);
                }
            },
            NodeBar {
                lead: rsx! {
                    form {
                        class: "curation-similar-query",
                        onsubmit: move |event| {
                            event.prevent_default();
                            search.call(());
                        },
                        TextInput {
                            class: "curation-similar-query-input",
                            kind: TextInputKind::Search,
                            value: query(),
                            "aria-label": "Subtask to match",
                            placeholder: "Describe a subtask, or use Find similar on any row",
                            oninput: move |value| query.set(value),
                        }
                        if running {
                            Button {
                                purpose: ActionPurpose::Stop,
                                variant: ButtonVariant::Outline,
                                disabled: run == RunState::Stopping,
                                title: "Stop asking; the episodes in flight stay unanswered",
                                onclick: {
                                    let node = node.clone();
                                    move |_| controller.stop_run(&node)
                                },
                                if run == RunState::Stopping { "Stopping…" } else { "Stop" }
                            }
                        } else {
                            Button {
                                purpose: ActionPurpose::Start,
                                variant: ButtonVariant::Solid,
                                r#type: "submit",
                                disabled: typed.is_none() || answered_already,
                                title: if typed.is_none() {
                                    "Type a subtask to search for.".to_owned()
                                } else if answered_already {
                                    "Every episode has answered this question.".to_owned()
                                } else {
                                    format!("Ask it of the subtasks of {}", Counted::episodes(pending))
                                },
                                "Search"
                            }
                        }
                    }
                },
                note: NodeNotes::join([stale, FailedEpisodes::note(failed)]),
                reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                actions: rsx! {
                    if answered.is_some() {
                        p { class: "curation-threshold-tally", role: "status",
                            "Keeps {kept} of {Counted::subtasks(listed_count)}"
                        }
                    }
                },
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let Some(reason) = reason {
                Notice { tone: NoticeTone::Danger, "An episode failed: {reason}" }
            }
            if !rows.is_empty() {
                div { class: "curation-similar-overview",
                    if let Some(row) = asked_row {
                        AskedAbout { row, thumbnails }
                    }
                    EpisodeMap {
                        lanes,
                        paint,
                        // While the model reads, the line over the map says how
                        // far it has got; once it answers, what it found.
                        summary: progress.clone().unwrap_or(summary),
                        on_select: {
                            let choose = choose.clone();
                            move |row: usize| {
                            choose(row);
                            let place = places.iter().position(|listed| *listed == row);
                            spawn(async move {
                                Motion::after_render().await;
                                if let Some(place) = place {
                                    SubtaskList::RANKED.reveal(place);
                                }
                            });
                            }
                        },
                    }
                    if answered.is_some() {
                        ThresholdStrip {
                            dots,
                            threshold,
                            chosen,
                            label: "Similarity threshold".to_owned(),
                            on_select: {
                                let choose = choose.clone();
                                move |row: usize| choose(row)
                            },
                            on_threshold: {
                                let node = node.clone();
                                move |minimum: f64| {
                                    let _ = controller.set_rule(&node, Selection::Similar { minimum });
                                }
                            },
                        }
                    }
                }
            }
            if listed.is_empty() && about.is_none() {
                EmptyState { title: "No subtasks to search",
                    p { "Run Break down subtasks on this dataset first; search asks about the subtasks its preview read." }
                }
            } else {
                SubtaskReview {
                    index: rsx! {
                        IndexBar {
                            p { class: "curation-index-caption",
                                // The row asked about is the question, not an answer.
                                if about.is_some() { "{listed_count} other subtasks" } else { "{Counted::subtasks(listed_count)}" }
                                if answered.is_some() { " · closest first" }
                            }
                        }
                        ol {
                            id: SubtaskList::RANKED.id(),
                            class: "curation-entries",
                            "aria-label": "Subtasks, closest to the query first; each row ends with how likely it is the same operation",
                            for (position , (row , standing)) in listed.into_iter().enumerate() {
                                // Where the ranking crosses the line the
                                // episode count above is drawn at: likely
                                // matches above it, the rest below.
                                Fragment { key: "{rows[row].episode()}-{rows[row].start_time()}",
                                if Some(position) == cut {
                                    li { class: "curation-entries-divider", role: "separator",
                                        "Below {threshold.label()} · left out"
                                    }
                                }
                                SubtaskEntry {
                                    row: rows[row].clone(),
                                    standing: standing.roving(TabStop::at(position, chosen_place)),
                                    episode: Episode::listed_name_of(rows[row].episode()),
                                    thumbnails,
                                    on_select: {
                                        let choose = choose.clone();
                                        move |()| choose(row)
                                    },
                                    on_find_similar: {
                                        let find = find.clone();
                                        let asked = rows[row].clone();
                                        move |from| find(asked.clone(), from)
                                    },
                                }
                                }
                            }
                        }
                    },
                    evidence: rsx! {
                        EpisodeTabs {
                            open: tabs,
                            empty: "Choose a result to watch it play.".to_owned(),
                            render: move |tab: OpenEpisode| {
                                let Some(row) = rows
                                    .iter()
                                    .position(|row| tab.holds(row.episode(), row.start_time()))
                                else {
                                    return rsx! {};
                                };
                                let asked = rows[row].clone();
                                rsx! {
                                    SubtaskEvidence {
                                        row: asked.clone(),
                                        play: tab.play(),
                                        intervals: SubtaskIntervals::around(&asked, &rows),
                                        context: context.clone(),
                                        on_open: {
                                            let focus = EpisodeFocus::new(
                                                annotation.clone(),
                                                asked.episode().clone(),
                                                Some((asked.start_time(), asked.end_time())),
                                            );
                                            move |()| controller.focus_episode(focus.clone())
                                        },
                                        on_find_similar: (Some(row) != about).then(|| {
                                            let find = find.clone();
                                            let asked = asked.clone();
                                            EventHandler::new(move |()| find(asked.clone(), None))
                                        }),
                                    }
                                }
                            },
                        }
                    },
                }
            }
        }
    }
}

/// The row a search is asking about, pinned above its answers: its thumbnail
/// and where it came from, in one line. Its words are already in the box above
/// it.
#[component]
fn AskedAbout(row: SubtaskRow, thumbnails: Thumbnails) -> Element {
    rsx! {
        div { class: "curation-similar-asked",
            Thumbnail { moment: ThumbnailMoment::of_subtask(&row), thumbnails }
            p { class: "curation-similar-asked-facts",
                "Asked from "
                code { "{Episode::listed_name_of(row.episode())}" }
                " at "
                code { "{row.start_time().clock_label()} – {row.end_time().clock_label()}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SearchTable;
    use crate::activities::curation::annotate::{JudgmentValue, SubtaskRow};
    use spiral_robotics_common::datasets::EpisodeId;

    #[test]
    fn an_answered_search_ranks_its_rows_and_lists_the_unanswered_last() {
        let row = |start: f64, similarity: Option<f64>| {
            SubtaskRow::new(EpisodeId::fixture(1), (start, start + 1.0), "fold")
                .unwrap()
                .judged(
                    SubtaskRow::SIMILARITY,
                    JudgmentValue::Similarity(similarity),
                )
        };
        let rows = vec![
            row(0.0, Some(0.2)),
            row(1.0, None),
            row(2.0, Some(0.9)),
            row(3.0, Some(0.2)),
        ];
        let table = SearchTable {
            answers: Some(rows.iter().map(SubtaskRow::similarity).collect()),
            rows,
        };
        let order = table
            .ranked(None)
            .into_iter()
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            [2, 0, 3, 1],
            "ties keep table order, the unanswered go last"
        );
        assert_eq!(table.top(Some(2)).map(SubtaskRow::start_time), Some(0.0));
        let unasked = SearchTable {
            rows: table.rows.clone(),
            answers: None,
        };
        assert_eq!(
            unasked.top(None),
            None,
            "nothing is best before anything is asked"
        );
    }
}
