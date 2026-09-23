//! Find similar subtasks: one Noul per saved row against one query.
//!
//! The ranking is computed on the fly and kept nowhere — only the query
//! itself is remembered, so reopening the node asks the same question of
//! whatever the annotation has since produced. Every row is shown with its
//! probability, best first; there is no cutoff and no threshold to tune.
use super::coverage::{CoverageMeasure, EpisodeCoverage, MeasuredEpisodes, SubtaskList};
use super::decisions::DecisionRun;
use super::episode_map::{
    EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, MapReveal, SegmentPaint,
};
use super::questions::{QuestionBatch, SearchQuestions};
use super::ranking::SearchRanking;
use super::source::ExplorationSource;
use crate::activities::episode::Episode;
use crate::activities::workflow::explore::document::{ExplorationNodeKind, SubtaskRow};
use crate::activities::workflow::explore::poster::{PosterCut, PosterFrame, PosterFrames};
use crate::activities::workflow::explore::review::{
    Counted, NoSubtaskChosen, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::workflow::explore::state::{EpisodeFocus, ExplorationController};
use crate::client::decisions::DecisionResponse;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Arrangement, Button, ButtonVariant, EmptyState, Motion, Notice, NoticeTone,
    Place, TextInput, TextInputKind,
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
    /// The best-scoring row other than the one asked about, once there is one.
    fn top(&self, about: Option<usize>) -> Option<usize> {
        self.ranking
            .ranked()
            .into_iter()
            .filter(|ranked| ranked.answered() && Some(ranked.row()) != about)
            .map(|ranked| ranked.row())
            .next()
    }
    /// Each row's probability in table order, `None` where jev has not
    /// answered for it.
    fn answers(&self, rows: usize) -> Vec<Option<f64>> {
        let mut answers = vec![None; rows];
        for ranked in self.ranking.ranked() {
            if ranked.answered()
                && let Some(answer) = answers.get_mut(ranked.row())
            {
                *answer = Some(ranked.probability());
            }
        }
        answers
    }
    /// Which episodes the ranking reached.
    fn coverage(&self, episodes: &MeasuredEpisodes, rows: &[SubtaskRow]) -> EpisodeCoverage {
        let ranked = self.ranking.ranked();
        EpisodeCoverage::of(
            episodes,
            ranked
                .iter()
                .filter(|ranked| ranked.answered())
                .filter_map(|ranked| {
                    Some((rows.get(ranked.row())?.episode(), ranked.probability()))
                }),
            CoverageMeasure::BestMatch,
        )
    }
}

/// The question a ranking on screen answers: the words asked, and the row they
/// were taken from when they came from one.
#[derive(Clone, Debug, PartialEq)]
struct AskedQuestion {
    text: String,
    about: Option<usize>,
}

/// The workspace shown for a selected Find similar subtasks node.
///
/// The node's saved question drives it: whenever the question in the document
/// is not the one the ranking on screen answers — the node just opened, or a
/// row elsewhere asked for more like it — the workspace asks it. A typed
/// query becomes the saved question before it is asked, so it never asks twice.
#[component]
pub(in crate::activities::workflow::explore) fn SearchWorkspace(
    controller: ExplorationController,
    node: String,
) -> Element {
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        ExplorationSource::of(controller, &node)
    }));
    let spans = EpisodeSpans::use_spans(source.read().context().dataset().cloned());
    let mut query = use_signal(|| source.peek().query().to_owned());
    let mut outcome = use_signal(|| Option::<SearchOutcome>::None);
    let mut selected = use_signal(|| Option::<usize>::None);
    let mut running = use_signal(|| false);
    let mut asked = use_signal(|| Option::<AskedQuestion>::None);
    // Bumped when a ranking's first answers land, so the map lights up again
    // for every new question and never for a click.
    let mut generation = use_signal(|| 0_u32);

    let searched_node = node.clone();
    let search =
        use_callback(move |()| {
            if *running.peek() {
                return;
            }
            let node = searched_node.clone();
            let text = query.peek().trim().to_owned();
            // The exploration records no task instruction today; when
            // annotation learns one, it belongs in the state beside the query.
            let batches = match SearchQuestions::new(text.clone(), None) {
                Ok(questions) => questions.batches(&source.peek().descriptions()),
                Err(problem) => {
                    outcome.set(Some(SearchOutcome::refused(problem)));
                    return;
                }
            };
            // Words that are still the saved row's own keep asking about it.
            let about = (text == source.peek().query().trim())
                .then(|| source.peek().asked_about())
                .flatten();
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
            asked.set(Some(AskedQuestion { text, about }));
            selected.set(None);
            outcome.set(Some(SearchOutcome::started(
                source.peek().rows().len(),
                batches.len(),
            )));
            running.set(true);
            spawn(async move {
                let result = DecisionRun::new(batches)
                    .run(|batch, answers| {
                        // The rows about to be re-ordered, where they stand now, so
                        // each can be seen moving to its rank.
                        let list = SubtaskList::RANKED.element();
                        let before = list.as_ref().map(Arrangement::measure);
                        let first = outcome
                            .peek()
                            .as_ref()
                            .is_some_and(|outcome| !outcome.has_answers());
                        if let Some(outcome) = outcome.write().as_mut() {
                            outcome.absorb(batch, &answers);
                        }
                        if first {
                            generation += 1;
                        }
                        if let (Some(list), Some(before)) = (list, before) {
                            spawn(async move {
                                Motion::after_render().await;
                                before.settle(&list, 16.0);
                            });
                        }
                    })
                    .await;
                match result {
                    // The evidence column is half of this surface; a finished
                    // ranking leaves its own answer in it rather than an empty
                    // half beside a full list.
                    Ok(()) => {
                        if selected.peek().is_none() {
                            let top = outcome
                                .peek()
                                .as_ref()
                                .and_then(|outcome| outcome.top(about));
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

    // Ask the saved question whenever it is not the one on screen. A row that
    // asked from elsewhere also left where it was, so its poster is carried
    // from there into the question it became.
    use_effect(move || {
        let saved = {
            let reading = source.read();
            AskedQuestion {
                text: reading.query().trim().to_owned(),
                about: reading.asked_about(),
            }
        };
        if saved.text.is_empty() || running() || asked.read().as_ref() == Some(&saved) {
            return;
        }
        query.set(saved.text.clone());
        search.call(());
        if let Some(from) = controller.take_launch() {
            spawn(async move {
                Motion::after_render().await;
                if let Some(poster) = SubtaskList::RANKED
                    .element()
                    .and_then(|list| list.closest(".explore-similar-search").ok().flatten())
                    .and_then(|workspace| {
                        workspace
                            .query_selector(".explore-similar-asked .explore-poster")
                            .ok()
                            .flatten()
                    })
                {
                    Motion::arrive_from(&poster, from);
                }
            });
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
    // The row the ranking on screen asked about is its question, not one of
    // its answers: it would only rank itself first.
    let about = asked.read().as_ref().and_then(|asked| asked.about);
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
    let lanes = spans
        .as_ref()
        .map(|spans| EpisodeLanes::of(&measured, &rows, spans))
        .unwrap_or_default();
    let chosen = selected();
    let reported = outcome.read();
    let failure = reported
        .as_ref()
        .and_then(|outcome| outcome.failure.clone());
    let answering = reported.as_ref().and_then(SearchOutcome::progress);
    let unanswered = reported
        .as_ref()
        .filter(|outcome| outcome.ranks())
        .map_or(0, |outcome| outcome.ranking.unanswered());
    let listed = match reported.as_ref().filter(|outcome| outcome.ranks()) {
        Some(outcome) => outcome.ranked(chosen),
        None => (0..rows.len())
            .map(|row| (row, SubtaskStanding::plain(chosen == Some(row))))
            .collect(),
    }
    .into_iter()
    .filter(|(row, _)| Some(*row) != about)
    .collect::<Vec<_>>();
    let answered = reported
        .as_ref()
        .filter(|outcome| outcome.has_answers())
        .map(|outcome| outcome.answers(rows.len()));
    let summary = match (&answered, reported.as_ref()) {
        (Some(_), Some(outcome)) => outcome.coverage(&measured, &rows).summary(),
        _ => format!(
            "{} across {}",
            Counted::subtasks(rows.len()),
            Counted::episodes(measured.len())
        ),
    };
    drop(reported);
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
        if running() && answered.is_none() {
            MapReveal::Reading
        } else {
            MapReveal::Ripple { origin: about }
        },
        generation(),
    );
    let progress = answering.or_else(|| {
        (asked.read().is_none() && !query().trim().is_empty())
            .then(|| format!("Ranking {} subtasks…", rows.len()))
    });
    // An empty box asks nothing, so Search refuses before it can replace the
    // table with a ranking of nothing.
    let typed = Some(query().trim().to_owned()).filter(|text| !text.is_empty());
    let asked_row = about.and_then(|row| rows.get(row)).cloned();
    let places = listed.iter().map(|(row, _)| *row).collect::<Vec<_>>();
    let find = {
        let annotation = annotation.clone();
        move |row: SubtaskRow, from: Option<Place>| controller.find_similar(&annotation, &row, from)
    };
    rsx! {
        section {
            class: "explore-similar-workspace explore-similar-search",
            "aria-label": "Find similar subtasks",
            onpointerdown: move |_| {
                if let Some(workspace) = SubtaskList::RANKED
                    .element()
                    .and_then(|list| list.closest(".explore-similar-search").ok().flatten())
                {
                    Motion::finish_within(&workspace);
                }
            },
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
                    placeholder: "Describe a subtask, or use Find similar on any row",
                    oninput: move |value| query.set(value),
                }
                Button {
                    purpose: ActionPurpose::Start,
                    variant: ButtonVariant::Solid,
                    r#type: "submit",
                    disabled: running() || typed.is_none(),
                    title: typed
                        .is_none()
                        .then(|| "Type a subtask to search for.".to_owned()),
                    if running() { "Searching…" } else { "Search" }
                }
            }
            if let Some(row) = asked_row {
                AskedAbout { row, frames }
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
            }
            if !rows.is_empty() {
                div { class: "explore-similar-overview",
                    EpisodeMap {
                        lanes,
                        paint,
                        summary,
                        on_select: move |row: usize| {
                            selected.set(Some(row));
                            let place = places.iter().position(|listed| *listed == row);
                            spawn(async move {
                                Motion::after_render().await;
                                if let Some(place) = place {
                                    SubtaskList::RANKED.reveal(place);
                                }
                            });
                        },
                    }
                    if let Some(answers) = answered.clone() {
                        MatchStrip {
                            key: "{generation()}",
                            answers,
                            about,
                            chosen,
                            on_select: move |row: usize| selected.set(Some(row)),
                        }
                    }
                }
            }
            if listed.is_empty() && about.is_none() {
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
                                    episode: Episode::name_of(rows[row].episode()),
                                    frames,
                                    on_select: move |()| selected.set(Some(row)),
                                    on_find_similar: {
                                        let find = find.clone();
                                        let asked = rows[row].clone();
                                        move |from| find(asked.clone(), from)
                                    },
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
                                        row.episode().clone(),
                                        Some((row.start_time(), row.end_time())),
                                    );
                                    move |()| controller.focus_episode(focus.clone())
                                },
                                on_find_similar: (chosen != about).then(|| {
                                    let find = find.clone();
                                    let asked = row.clone();
                                    EventHandler::new(move |()| find(asked.clone(), None))
                                }),
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

/// The row a search is asking about, pinned above its answers: its still and
/// where it came from. Its words are already in the box above it.
#[component]
fn AskedAbout(row: SubtaskRow, frames: PosterFrames) -> Element {
    rsx! {
        div { class: "explore-similar-asked",
            PosterFrame { row: row.clone(), frames }
            p { class: "explore-similar-asked-words",
                span { class: "explore-similar-asked-label", "Asked about" }
                span { class: "explore-similar-asked-facts",
                    "{Episode::name_of(row.episode())} · {row.start_time().clock_label()} – {row.end_time().clock_label()}"
                }
            }
        }
    }
}

/// Every answered row as a dot on one axis from 0 to 100%, so the gap between
/// the rows jev is sure of and the rest shows how many real matches there are
/// without drawing a cutoff. The tick at the half is the line the episode
/// count above uses.
#[component]
fn MatchStrip(
    /// Each row's probability, in table order; `None` is a row jev has not
    /// answered for.
    answers: Vec<Option<f64>>,
    /// The row asked about, which is the question rather than an answer.
    about: Option<usize>,
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
) -> Element {
    let mut ranked = answers
        .iter()
        .enumerate()
        .filter(|(row, _)| Some(*row) != about)
        .filter_map(|(row, probability)| Some((row, (*probability)?)))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    let spread = ranked.len().max(1) as f64;
    rsx! {
        div { class: "explore-match-strip", role: "group", "aria-label": "Every answer by how likely it is the same operation",
            span { class: "explore-match-threshold", style: "left: {CoverageMeasure::MATCHED * 100.0}%" }
            for (rank , (row , probability)) in ranked.into_iter().enumerate() {
                button {
                    key: "{row}",
                    r#type: "button",
                    class: "explore-match-dot",
                    tabindex: "-1",
                    "aria-current": (chosen == Some(row)).then_some("true"),
                    // Best first, so the sure matches arrive before the doubt.
                    style: "left: {probability * 100.0:.2}%; --lift: {(row * 7) % 5}; --delay: {rank as f64 / spread * 500.0:.0}ms; --match: {probability * 100.0:.0}%",
                    title: "{probability * 100.0:.1}% same operation",
                    "aria-label": "{probability * 100.0:.1}% same operation",
                    onclick: move |_| on_select.call(row),
                }
            }
            span { class: "explore-match-axis", "aria-hidden": "true",
                span { "0%" }
                span { "50%" }
                span { "100%" }
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
