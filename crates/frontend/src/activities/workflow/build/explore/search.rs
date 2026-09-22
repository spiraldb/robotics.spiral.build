//! Find similar subtasks: one Noul per saved row against one query.
//!
//! The ranking is computed on the fly and kept nowhere — only the query
//! itself is remembered, so reopening the node asks the same question of
//! whatever the annotation has since produced. Every row is shown with its
//! probability, best first; there is no cutoff and no threshold to tune.
use super::decisions::DecisionRun;
use super::questions::{QuestionBatch, SearchQuestions};
use super::ranking::SearchRanking;
use super::slice::{SliceActions, SliceScore, SubtaskCamera, SubtaskSlice};
use super::source::ExplorationSource;
use crate::activities::workflow::build::document::{BuildNodeKind, SubtaskRow};
use crate::activities::workflow::build::state::{BuildController, EpisodeFocus};
use crate::client::model::{DecisionResponse, ModelClient};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;
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
                "Ranking {} subtasks · {} of {} requests answered",
                self.ranking.len(),
                self.answered,
                self.requests
            )
        })
    }
    /// The rows to show, best first, each with its probability.
    fn ranked(&self) -> Vec<(usize, Option<SliceScore>)> {
        self.ranking
            .ranked()
            .iter()
            .map(|ranked| {
                (
                    ranked.row(),
                    Some(SliceScore::new(ranked.probability(), ranked.answered())),
                )
            })
            .collect()
    }
}

/// The workspace shown for a selected Find similar subtasks node.
#[component]
pub(in crate::activities::workflow::build) fn SearchWorkspace(
    controller: BuildController,
    node: String,
) -> Element {
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        ExplorationSource::of(controller, &node)
    }));
    let mut query = use_signal(String::new);
    let mut outcome = use_signal(|| Option::<SearchOutcome>::None);
    let mut selected = use_signal(|| Option::<usize>::None);
    let mut running = use_signal(|| false);
    // The query belongs to the node, and a ranking belongs to the query that
    // produced it, so both start again when the canvas selects another node.
    let mut showing = use_signal(|| Option::<String>::None);
    use_effect(use_reactive((&node,), move |(node,)| {
        if showing.peek().as_deref() == Some(node.as_str()) {
            return;
        }
        showing.set(Some(node.clone()));
        query.set(source.peek().query().to_owned());
        outcome.set(None);
        selected.set(None);
    }));

    let searched_node = node.clone();
    let search =
        use_callback(move |()| {
            if running() {
                return;
            }
            let node = searched_node.clone();
            let text = query.peek().trim().to_owned();
            // The Build document records no task instruction today; when
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
                    Some(BuildNodeKind::FindSimilarSubtasks(search)) => {
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
                if let Err(problem) = result {
                    let mut failed = outcome.write();
                    if let Some(outcome) = failed.as_mut() {
                        outcome.fail(problem);
                    }
                }
                running.set(false);
            });
        });

    let reading = source.read();
    if reading.annotation().is_none() {
        return rsx! {
            section { class: "build-explore-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This search is not connected to a Break down subtasks node."
                }
            }
        };
    }
    let rows = reading.rows().to_vec();
    let dataset = reading.dataset().cloned();
    let annotation = reading.annotation().unwrap_or_default().to_owned();
    drop(reading);
    let (listed, failure, progress, unanswered) = match outcome.read().as_ref() {
        Some(outcome) => (
            outcome.ranked(),
            outcome.failure.clone(),
            outcome.progress(),
            outcome.ranking.unanswered(),
        ),
        None => (
            (0..rows.len()).map(|row| (row, None)).collect(),
            None,
            None,
            0,
        ),
    };
    let chosen = selected();
    let described = chosen
        .and_then(|row| rows.get(row))
        .map(|row| row.description().to_owned());
    rsx! {
        section { class: "build-explore-workspace build-explore-search", "aria-label": "Find similar subtasks",
            div { class: "build-explore-query",
                TextInput {
                    class: "build-explore-query-input",
                    kind: TextInputKind::Search,
                    value: query(),
                    "aria-label": "Subtask to match",
                    placeholder: "Describe a subtask, or select one below",
                    oninput: move |value| query.set(value),
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Enter {
                            search.call(());
                        }
                    },
                }
                Button {
                    purpose: ActionPurpose::Start,
                    variant: ButtonVariant::Solid,
                    disabled: running(),
                    onclick: move |_| search.call(()),
                    if running() { "Searching…" } else { "Search" }
                }
                Button {
                    purpose: ActionPurpose::Select,
                    variant: ButtonVariant::Outline,
                    disabled: described.is_none(),
                    onclick: move |_| {
                        if let Some(description) = described.clone() {
                            query.set(description);
                        }
                    },
                    "Use selected subtask"
                }
            }
            if let Some(problem) = failure {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let Some(progress) = progress {
                p { class: "build-explore-note", role: "status", "{progress}" }
            } else if unanswered > 0 {
                Notice { tone: NoticeTone::Caution,
                    "{unanswered} subtasks came back without an answer and are listed last."
                }
            }
            if listed.is_empty() {
                EmptyState { title: "No subtasks to search",
                    p { "Run Break down subtasks on this dataset first; search reads the rows it saved." }
                }
            } else {
                ol { class: "build-explore-results",
                    for (row , score) in listed {
                        SearchResult {
                            key: "{row}",
                            row: rows[row].clone(),
                            score,
                            selected: chosen == Some(row),
                            dataset: dataset.clone(),
                            actions: SliceActions::new(
                                EventHandler::new(move |()| {
                                    selected.set((chosen != Some(row)).then_some(row));
                                }),
                                {
                                    let focus = EpisodeFocus::new(
                                        annotation.clone(),
                                        rows[row].episode(),
                                        Some((rows[row].start_time(), rows[row].end_time())),
                                    );
                                    EventHandler::new(move |()| controller.focus_episode(focus.clone()))
                                },
                            ),
                        }
                    }
                }
            }
        }
    }
}

/// One row of the ranking, with its camera while it is the selected result.
#[component]
fn SearchResult(
    row: SubtaskRow,
    score: Option<SliceScore>,
    selected: bool,
    dataset: Option<DatasetSelection>,
    actions: SliceActions,
) -> Element {
    let camera = match (selected, dataset) {
        (false, _) => rsx! {},
        (true, Some(dataset)) => rsx! {
            SubtaskCamera {
                dataset,
                episode: row.episode().to_owned(),
                interval: (row.start_time(), row.end_time()),
            }
        },
        (true, None) => rsx! {
            p { class: "build-explore-note", role: "status",
                "This Build document has no dataset, so the recording cannot be opened."
            }
        },
    };
    rsx! {
        li { class: "build-explore-result",
            SubtaskSlice { row, score, selected, actions, {camera} }
        }
    }
}
