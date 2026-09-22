//! Find similar subtasks: one Noul per saved row against one query.
//!
//! The ranking is computed on the fly and kept nowhere — only the query
//! itself is remembered, so reopening the node asks the same question of
//! whatever the annotation has since produced. Every row is shown with its
//! probability, best first; there is no cutoff and no threshold to tune.
use super::coverage::{
    CoverageMeasure, CoverageStrip, EpisodeCoverage, MeasuredEpisodes, SubtaskList,
};
use super::decisions::DecisionRun;
use super::questions::{QuestionBatch, SearchQuestions};
use super::ranking::{RankedRow, SearchRanking};
use super::source::ExplorationSource;
use crate::activities::workflow::explore::document::{ExplorationNodeKind, SubtaskRow};
use crate::activities::workflow::explore::poster::{PosterCut, PosterFrames};
use crate::activities::workflow::explore::review::{
    NoSubtaskChosen, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::workflow::explore::state::{EpisodeFocus, ExplorationController};
use crate::client::completions::ModelClient;
use crate::client::decisions::DecisionResponse;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Notice, NoticeTone, TextInput, TextInputKind,
};

/// One search's answers so far: the ranking, how many of its requests have
/// come back, and the failure that ended it.
#[derive(Clone, PartialEq)]
struct SearchOutcome {
    ranking: SearchRanking,
    answered: usize,
    requests: usize,
    failure: Option<String>,
}
impl SearchOutcome {
    fn started(rows: usize, requests: usize) -> Self {
        Self {
            ranking: SearchRanking::of(rows),
            answered: 0,
            requests,
            failure: None,
        }
    }
    fn refused(problem: String) -> Self {
        Self {
            ranking: SearchRanking::of(0),
            answered: 0,
            requests: 0,
            failure: Some(problem),
        }
    }
    /// Whether this outcome ranks the table at all. A refusal asked nothing,
    /// so the rows the operator was reading stay in their own order behind
    /// the reason they were not re-ordered.
    fn ranks(&self) -> bool {
        self.requests > 0
    }
    fn absorb(&mut self, batch: &QuestionBatch, answers: &DecisionResponse) {
        self.ranking.absorb(batch, answers);
        self.answered += 1;
    }
    fn fail(&mut self, problem: String) {
        self.failure = Some(problem);
    }
    /// What the run is doing, while it is still doing it.
    fn progress(&self) -> Option<String> {
        (self.failure.is_none() && self.answered < self.requests).then(|| {
            format!(
                "Ranking {} subtasks · {} of {} batches answered",
                self.ranking.len(),
                self.answered,
                self.requests
            )
        })
    }
    /// The rows to show, best first, each with its probability.
    fn ranked(&self, chosen: Option<usize>) -> Vec<(usize, SubtaskStanding)> {
        self.ranking
            .ranked()
            .iter()
            .map(|ranked| {
                (
                    ranked.row(),
                    SubtaskStanding::scored(
                        ranked.probability(),
                        ranked.answered(),
                        chosen == Some(ranked.row()),
                    ),
                )
            })
            .collect()
    }
    /// Whether jev has answered for anything yet. Until it has, the ranking is
    /// the table in its own order and says nothing about any episode.
    fn has_answers(&self) -> bool {
        self.answered > 0
    }
    /// The best-scoring row, once there is one.
    fn top(&self) -> Option<usize> {
        self.ranking
            .ranked()
            .first()
            .filter(|ranked| ranked.answered())
            .map(RankedRow::row)
    }
    /// Which episodes the ranking reached, read in the index's own order so a
    /// cell points at the best-ranked row of its episode.
    ///
    /// Every ranked row is offered, in place, so the strip's places and the
    /// index's rows stay the same list; a row the table no longer holds joins
    /// no episode but still takes its place.
    fn coverage(&self, episodes: &MeasuredEpisodes, rows: &[SubtaskRow]) -> EpisodeCoverage {
        let ranked = self.ranking.ranked();
        EpisodeCoverage::of(
            episodes,
            ranked.iter().map(|ranked| {
                (
                    rows.get(ranked.row()).map_or("", SubtaskRow::episode),
                    ranked.probability(),
                )
            }),
            CoverageMeasure::BestMatch,
        )
    }
}

/// The workspace shown for a selected Find similar subtasks node.
#[component]
pub(in crate::activities::workflow::explore) fn SearchWorkspace(
    controller: ExplorationController,
    node: String,
) -> Element {
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        ExplorationSource::of(controller, &node)
    }));
    let mut query = use_signal(|| source.peek().query().to_owned());
    let mut outcome = use_signal(|| Option::<SearchOutcome>::None);
    let mut selected = use_signal(|| Option::<usize>::None);
    let mut running = use_signal(|| false);
    // The query belongs to the node, and a ranking belongs to the query that
    // produced it, so both start again when the canvas selects another node.
    let mut showing = use_signal(|| Option::<String>::None);
    // Set before the first paint, not in the effect that spends the money, so
    // the page says a ranking is going out before the request does.
    let mut reopened = use_signal(|| !source.peek().query().trim().is_empty());
    use_effect(use_reactive((&node,), move |(node,)| {
        if showing.peek().as_deref() == Some(node.as_str()) {
            return;
        }
        showing.set(Some(node.clone()));
        let saved = source.peek().query().to_owned();
        query.set(saved.clone());
        outcome.set(None);
        selected.set(None);
        // Nothing is persisted but the query itself, so a node reopened with
        // one asks it again rather than showing an unranked list the operator
        // has to re-run by hand. A ranking is one round of cheap requests.
        reopened.set(!saved.trim().is_empty());
    }));

    let searched_node = node.clone();
    let search =
        use_callback(move |()| {
            if running() {
                return;
            }
            let node = searched_node.clone();
            let text = query.peek().trim().to_owned();
            // The exploration records no task instruction today; when
            // annotation learns one, it belongs in the state beside the query.
            let task = None;
            let batches = match SearchQuestions::new(text.clone(), task) {
                Ok(questions) => questions.batches(&source.peek().descriptions()),
                Err(problem) => {
                    outcome.set(Some(SearchOutcome::refused(problem)));
                    return;
                }
            };
            if let Err(problem) = controller.edit(|payload| {
                match payload.node_mut(&node).map(|node| node.kind_mut()) {
                    Some(ExplorationNodeKind::FindSimilarSubtasks(search)) => {
                        search.set_query(text.clone());
                        Ok(())
                    }
                    _ => Err("This node no longer searches subtasks.".to_owned()),
                }
            }) {
                outcome.set(Some(SearchOutcome::refused(problem)));
                return;
            }
            selected.set(None);
            outcome.set(Some(SearchOutcome::started(
                source.peek().rows().len(),
                batches.len(),
            )));
            running.set(true);
            spawn(async move {
                let result = DecisionRun::new(ModelClient::current(), batches)
                    .run(|batch, answers| {
                        if let Some(outcome) = outcome.write().as_mut() {
                            outcome.absorb(batch, &answers);
                        }
                    })
                    .await;
                match result {
                    // The evidence column is half of this surface; a finished
                    // ranking leaves its own answer in it rather than an empty
                    // half beside a full list.
                    Ok(()) => {
                        if selected.peek().is_none() {
                            let top = outcome.peek().as_ref().and_then(SearchOutcome::top);
                            selected.set(top);
                        }
                    }
                    Err(problem) => {
                        if let Some(outcome) = outcome.write().as_mut() {
                            outcome.fail(problem);
                        }
                    }
                }
                running.set(false);
            });
        });

    // Asking again happens after `search` exists, and outside the effect that
    // set the flag, so the callback it calls is the one for this node.
    use_effect(move || {
        if reopened() {
            reopened.set(false);
            search.call(());
        }
    });

    let reading = source.read();
    if reading.annotation().is_none() {
        return rsx! {
            section { class: "explore-similar-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This search is not connected to a Break down subtasks node."
                }
            }
        };
    }
    let rows = reading.rows().to_vec();
    let context = reading.context();
    let measured = reading.measured();
    let annotation = reading.annotation().unwrap_or_default().to_owned();
    drop(reading);
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
    let chosen = selected();
    let reported = outcome.read();
    let failure = reported
        .as_ref()
        .and_then(|outcome| outcome.failure.clone());
    let (listed, answering, unanswered, coverage) =
        match reported.as_ref().filter(|outcome| outcome.ranks()) {
            Some(outcome) => (
                outcome.ranked(chosen),
                outcome.progress(),
                outcome.ranking.unanswered(),
                outcome
                    .has_answers()
                    .then(|| outcome.coverage(&measured, &rows)),
            ),
            None => (
                (0..rows.len())
                    .map(|row| (row, SubtaskStanding::plain(chosen == Some(row))))
                    .collect(),
                None,
                0,
                None,
            ),
        };
    drop(reported);
    // A reopened node re-ranks on mount, so the line that says so belongs on
    // the first paint — before the request, not after its first answer.
    let progress = if reopened() {
        Some(format!("Ranking {} subtasks…", rows.len()))
    } else {
        answering
    };
    // An empty box asks nothing, so Search refuses before it can replace the
    // table with a ranking of nothing.
    let asked = Some(query().trim().to_owned()).filter(|text| !text.is_empty());
    let described = chosen
        .and_then(|row| rows.get(row))
        .map(|row| row.description().to_owned());
    let showing = chosen
        .and_then(|row| rows.get(row))
        .map(|row| row.episode().to_owned());
    // Once a ranking exists the coverage line counts the table; until then,
    // say what is waiting to be ranked.
    let waiting = (coverage.is_none() && progress.is_none() && !rows.is_empty())
        .then(|| format!("{} subtasks, waiting for a query", rows.len()));
    // The strip points at places in the ranking; the table row that sits in
    // each of them is this view's own business.
    let ranked_rows = listed.iter().map(|(row, _)| *row).collect::<Vec<_>>();
    rsx! {
        section { class: "explore-similar-workspace explore-similar-search", "aria-label": "Find similar subtasks",
            form {
                class: "explore-similar-query",
                onsubmit: move |event| {
                    event.prevent_default();
                    search.call(());
                },
                TextInput {
                    class: "explore-similar-query-input",
                    kind: TextInputKind::Search,
                    value: query(),
                    "aria-label": "Subtask to match",
                    placeholder: "Describe a subtask, or pick one from the list",
                    oninput: move |value| query.set(value),
                }
                Button {
                    purpose: ActionPurpose::Start,
                    variant: ButtonVariant::Solid,
                    r#type: "submit",
                    disabled: running() || asked.is_none(),
                    title: asked
                        .is_none()
                        .then(|| "Type a subtask to search for, or pick one from the table.".to_owned()),
                    if running() { "Searching…" } else { "Search" }
                }
                Button {
                    purpose: ActionPurpose::Select,
                    variant: ButtonVariant::Outline,
                    r#type: "button",
                    disabled: described.is_none(),
                    title: "Put the selected subtask's own words in the box",
                    onclick: move |_| {
                        if let Some(description) = described.clone() {
                            query.set(description);
                        }
                    },
                    "Use selected"
                }
            }
            if let Some(problem) = failure {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let Some(progress) = progress {
                p { class: "explore-similar-note", role: "status", "{progress}" }
            } else if unanswered > 0 {
                Notice { tone: NoticeTone::Caution,
                    "{unanswered} subtasks came back without an answer and are listed last."
                }
            } else if let Some(waiting) = waiting {
                p { class: "explore-similar-note", role: "status", "{waiting}" }
            }
            if let Some(coverage) = coverage {
                CoverageStrip {
                    coverage,
                    list: SubtaskList::RANKED,
                    showing,
                    on_reveal: move |place: Option<usize>| {
                        selected.set(place.and_then(|place| ranked_rows.get(place).copied()));
                    },
                }
            }
            if listed.is_empty() {
                EmptyState { title: "No subtasks to search",
                    p { "Run Break down subtasks on this dataset first; search reads the rows it saved." }
                }
            } else {
                SubtaskReview {
                    index: rsx! {
                        ol {
                            id: SubtaskList::RANKED.id(),
                            class: "explore-entries",
                            "aria-label": "Subtasks, closest to the query first; each row ends with how likely jev thinks it is the same operation",
                            for (row , standing) in listed {
                                SubtaskEntry {
                                    key: "{rows[row].episode()}-{rows[row].start_time()}",
                                    row: rows[row].clone(),
                                    standing,
                                    episode: context.name(rows[row].episode()),
                                    frames,
                                    on_select: move |()| selected.set(Some(row)),
                                }
                            }
                        }
                    },
                    evidence: match chosen.and_then(|row| rows.get(row)) {
                        Some(row) => rsx! {
                            SubtaskEvidence {
                                key: "{row.episode()}-{row.start_time()}",
                                row: row.clone(),
                                episode_rows: rows
                                    .iter()
                                    .filter(|other| other.episode() == row.episode())
                                    .cloned()
                                    .collect::<Vec<_>>(),
                                context: context.clone(),
                                on_open: {
                                    let focus = EpisodeFocus::new(
                                        annotation.clone(),
                                        row.episode(),
                                        Some((row.start_time(), row.end_time())),
                                    );
                                    move |()| controller.focus_episode(focus.clone())
                                },
                            }
                        },
                        None => rsx! {
                            NoSubtaskChosen {
                                prompt: "Choose a result to watch it play.",
                            }
                        },
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SearchOutcome;

    #[test]
    fn a_refused_search_ranks_nothing_so_the_table_keeps_its_own_rows() {
        let refused = SearchOutcome::refused("Type a subtask to search for.".to_owned());
        assert!(!refused.ranks());
        assert_eq!(
            refused.failure.as_deref(),
            Some("Type a subtask to search for.")
        );
        assert!(SearchOutcome::started(311, 4).ranks());
    }
}
