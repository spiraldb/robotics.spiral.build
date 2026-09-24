//! Find similar subtasks: one Noul per row that reaches the node, against one
//! query.
//!
//! A finished ranking is saved beside the query it answers, so reopening the
//! node shows it again without asking, and a Select after the node keeps rows
//! by it. Every row is shown with its probability, best first; the node has
//! no cutoff of its own — a Select after it draws one.
use super::coverage::{CoverageMeasure, EpisodeCoverage, MeasuredEpisodes, SubtaskList};
use super::decisions::DecisionRun;
use super::questions::{QuestionBatch, SearchQuestions};
use super::ranking::SearchRanking;
use super::source::SubtaskSource;
use crate::activities::curation::document::{CurationNodeKind, SubtaskRanking, SubtaskRow};
use crate::activities::curation::episode_map::{
    EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, MapReveal, SegmentPaint,
};
use crate::activities::curation::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use crate::activities::curation::population::Slice;
use crate::activities::curation::poster::{PosterCut, PosterFrame, PosterFrames};
use crate::activities::curation::review::{
    Counted, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus};
use crate::activities::episode::Episode;
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
    /// A stored ranking, read back as if it had just been answered.
    fn restored(rows: &[SubtaskRow], stored: &SubtaskRanking) -> Self {
        Self {
            ranking: SearchRanking::restored(
                rows.len(),
                rows.iter()
                    .enumerate()
                    .filter_map(|(index, row)| Some((index, stored.probability(row)?))),
            ),
            answered: 1,
            requests: 1,
            failure: None,
        }
    }
    /// What this outcome stores: every answered row's probability, under the
    /// question it answers.
    fn stored(
        &self,
        node: &str,
        query: &str,
        rows: &[SubtaskRow],
    ) -> Result<SubtaskRanking, String> {
        SubtaskRanking::new(
            node,
            query,
            self.ranking
                .ranked()
                .into_iter()
                .filter(|ranked| ranked.answered())
                .filter_map(|ranked| Some((rows.get(ranked.row())?.key(), ranked.probability()))),
        )
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
    /// Which episodes the ranking reached. The row asked about is the
    /// question, and jev finding it the same as itself says nothing about
    /// its episode, so it is not counted.
    fn coverage(
        &self,
        episodes: &MeasuredEpisodes,
        rows: &[SubtaskRow],
        about: Option<usize>,
    ) -> EpisodeCoverage {
        let ranked = self.ranking.ranked();
        EpisodeCoverage::of(
            episodes,
            ranked
                .iter()
                .filter(|ranked| ranked.answered() && Some(ranked.row()) != about)
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
pub(in crate::activities::curation) fn SearchWorkspace(
    controller: CurationController,
    node: String,
) -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        SubtaskSource::of(controller, &node)
    }));
    let spans = controller.sample().as_ref().map(EpisodeSpans::of);
    let mut query = use_signal(|| source.peek().query().to_owned());
    let mut outcome = use_signal(|| Option::<SearchOutcome>::None);
    // The results a click opens, one tab per episode; the row chosen is the
    // one playing in the tab in front.
    let tabs = OpenEpisodes::use_open();
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
            // The curation records no task instruction today; when
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
                    Some(CurationNodeKind::FindSimilarSubtasks(search)) => {
                        search.set_query(text.clone());
                        Ok(())
                    }
                    _ => Err("This node no longer searches subtasks.".to_owned()),
                }
            }) {
                outcome.set(Some(SearchOutcome::refused(problem)));
                return;
            }
            asked.set(Some(AskedQuestion {
                text: text.clone(),
                about,
            }));
            let asked_rows = source.peek().rows().to_vec();
            outcome.set(Some(SearchOutcome::started(
                asked_rows.len(),
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
                        let stored = outcome
                            .peek()
                            .as_ref()
                            .map(|outcome| outcome.stored(&node, &text, &asked_rows));
                        if let Some(Ok(ranking)) = stored
                            && controller
                                .edit(|payload| payload.set_ranking(ranking))
                                .is_ok()
                        {
                            controller.save(studio);
                        }
                        // The best answer opens as soon as there is one, so
                        // the question's result is on screen, not a click away.
                        let top = outcome
                            .peek()
                            .as_ref()
                            .and_then(|outcome| outcome.top(about))
                            .and_then(|top| asked_rows.get(top));
                        if let Some(row) = top {
                            tabs.open(row.episode(), Some((row.start_time(), row.end_time())));
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
    // A search's card and the top bar say it is ranking while it does. Its
    // requests belong to this workspace, so leaving the node ends them and
    // the line with them.
    let reported = node.clone();
    use_effect(move || {
        let line =
            running().then(|| format!("Ranking {}", Counted::subtasks(source.read().rows().len())));
        controller.report_activity(&reported, line);
    });
    let dropped = node.clone();
    use_drop(move || controller.report_activity(&dropped, None));
    let restored_node = node.clone();
    use_effect(move || {
        // The rows arrive with the episode sample, which may still be read
        // when the node opens. A question restored or asked against no rows
        // was answered for nothing and never asked again, so it waits.
        let (saved, no_rows) = {
            let reading = source.read();
            let saved = AskedQuestion {
                text: reading.query().trim().to_owned(),
                about: reading.asked_about(),
            };
            (saved, reading.rows().is_empty())
        };
        if saved.text.is_empty() || no_rows || running() || asked.read().as_ref() == Some(&saved) {
            return;
        }
        // A question already answered and saved is shown, not asked again.
        let stored =
            controller.with_payload(|payload| payload.current_ranking(&restored_node).cloned());
        if let Some(stored) = stored.filter(|stored| stored.query().trim() == saved.text) {
            let restored = SearchOutcome::restored(source.peek().rows(), &stored);
            // A reopened search plays its best answer, as a finished one does,
            // rather than an empty half beside the ranking.
            let top = restored
                .top(saved.about)
                .and_then(|top| source.peek().rows().get(top).cloned());
            tabs.open_first(top.map(|row| {
                (
                    row.episode().clone(),
                    Some((row.start_time(), row.end_time())),
                )
            }));
            outcome.set(Some(restored));
            query.set(saved.text.clone());
            asked.set(Some(saved));
            return;
        }
        query.set(saved.text.clone());
        search.call(());
        if let Some(from) = controller.take_launch() {
            spawn(async move {
                Motion::after_render().await;
                if let Some(poster) = SubtaskList::RANKED
                    .element()
                    .and_then(|list| list.closest(".curation-similar-search").ok().flatten())
                    .and_then(|workspace| {
                        workspace
                            .query_selector(".curation-similar-asked .curation-poster")
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
            section { class: "curation-similar-workspace",
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
        (Some(_), Some(outcome)) => outcome.coverage(&measured, &rows, about).summary(),
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
                p { class: "curation-similar-note", role: "status", "{progress}" }
            } else if unanswered > 0 {
                Notice { tone: NoticeTone::Caution,
                    "{unanswered} subtasks came back without an answer and are listed last."
                }
            }
            if !rows.is_empty() {
                div { class: "curation-similar-overview",
                    EpisodeMap {
                        lanes,
                        paint,
                        summary,
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
                    if let Some(answers) = answered.clone() {
                        MatchStrip {
                            key: "{generation()}",
                            answers,
                            about,
                            chosen,
                            on_select: {
                                let choose = choose.clone();
                                move |row: usize| choose(row)
                            },
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
                            class: "curation-entries",
                            "aria-label": "Subtasks, closest to the query first; each row ends with how likely it is the same operation",
                            for (row , standing) in listed {
                                SubtaskEntry {
                                    key: "{rows[row].episode()}-{rows[row].start_time()}",
                                    row: rows[row].clone(),
                                    standing,
                                    episode: Episode::listed_name_of(rows[row].episode()),
                                    frames,
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
                                        episode_rows: rows
                                            .iter()
                                            .filter(|other| other.episode() == asked.episode())
                                            .cloned()
                                            .collect::<Vec<_>>(),
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

/// The row a search is asking about, pinned above its answers: its still and
/// where it came from, in one line. Its words are already in the box above it.
#[component]
fn AskedAbout(row: SubtaskRow, frames: PosterFrames) -> Element {
    rsx! {
        div { class: "curation-similar-asked",
            PosterFrame { row: row.clone(), frames }
            p { class: "curation-similar-asked-facts",
                "From {Episode::name_of(row.episode())} · {row.start_time().clock_label()} – {row.end_time().clock_label()}"
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
        // A pointer shortcut, as the map is: the ranking beside it is the
        // same answers in words.
        div { class: "curation-match-strip", "aria-hidden": "true",
            span { class: "curation-match-threshold", style: "left: {CoverageMeasure::MATCHED * 100.0}%" }
            for (rank , (row , probability)) in ranked.into_iter().enumerate() {
                button {
                    key: "{row}",
                    r#type: "button",
                    class: "curation-match-dot",
                    tabindex: "-1",
                    "aria-current": (chosen == Some(row)).then_some("true"),
                    // Best first, so the sure matches arrive before the doubt.
                    style: "left: {probability * 100.0:.2}%; --lift: {(row * 7) % 5}; --delay: {rank as f64 / spread * 500.0:.0}ms; --match: {probability * 100.0:.0}%",
                    title: "{probability * 100.0:.1}% same operation",
                    "aria-label": "{probability * 100.0:.1}% same operation",
                    onclick: move |_| on_select.call(row),
                }
            }
            span { class: "curation-match-axis", "aria-hidden": "true",
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
