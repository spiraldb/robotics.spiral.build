//! Find similar subtasks: one Noul per saved row against one query.
//!
//! The ranking is computed on the fly and kept nowhere — only the query
//! itself is remembered, so reopening the node asks the same question of
//! whatever the annotation has since produced. Every row is shown with its
//! probability, best first; there is no cutoff and no threshold to tune.
use super::decisions::DecisionRun;
use super::questions::{QuestionBatch, SearchQuestions};
use super::ranking::SearchRanking;
use super::source::ExplorationSource;
use crate::activities::workflow::build::document::BuildNodeKind;
use crate::activities::workflow::build::poster::{PosterCut, PosterFrames};
use crate::activities::workflow::build::review::{
    NoSubtaskChosen, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::workflow::build::state::{BuildController, EpisodeFocus};
use crate::client::model::{DecisionResponse, ModelClient};
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
    let mut reopened = use_signal(|| false);
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
            section { class: "build-explore-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This search is not connected to a Break down subtasks node."
                }
            }
        };
    }
    let rows = reading.rows().to_vec();
    let context = reading.context();
    let annotation = reading.annotation().unwrap_or_default().to_owned();
    drop(reading);
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
    let chosen = selected();
    let (listed, failure, progress, unanswered) = match outcome.read().as_ref() {
        Some(outcome) => (
            outcome.ranked(chosen),
            outcome.failure.clone(),
            outcome.progress(),
            outcome.ranking.unanswered(),
        ),
        None => (
            (0..rows.len())
                .map(|row| (row, SubtaskStanding::plain(chosen == Some(row))))
                .collect(),
            None,
            None,
            0,
        ),
    };
    let described = chosen
        .and_then(|row| rows.get(row))
        .map(|row| row.description().to_owned());
    let ranked = outcome.read().is_some();
    let counted = match (ranked, rows.len()) {
        (_, 0) => String::new(),
        (true, count) => format!("{count} subtasks ranked, closest first"),
        (false, count) => format!("{count} subtasks, waiting for a query"),
    };
    rsx! {
        section { class: "build-explore-workspace build-explore-search", "aria-label": "Find similar subtasks",
            form {
                class: "build-explore-query",
                onsubmit: move |event| {
                    event.prevent_default();
                    search.call(());
                },
                TextInput {
                    class: "build-explore-query-input",
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
                    disabled: running(),
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
                p { class: "build-explore-note", role: "status", "{progress}" }
            } else if unanswered > 0 {
                Notice { tone: NoticeTone::Caution,
                    "{unanswered} subtasks came back without an answer and are listed last."
                }
            } else if !counted.is_empty() {
                p { class: "build-explore-note", role: "status", "{counted}" }
            }
            if listed.is_empty() {
                EmptyState { title: "No subtasks to search",
                    p { "Run Break down subtasks on this dataset first; search reads the rows it saved." }
                }
            } else {
                SubtaskReview {
                    index: rsx! {
                        ol { class: "build-entries",
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
                                prompt: "Choose a result to watch it play, on the camera the annotation read.",
                            }
                        },
                    },
                }
            }
        }
    }
}
