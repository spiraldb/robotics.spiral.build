//! Preview runs: the queries one curation node's preview takes over the
//! sample, run here so the browser holds no request open while a model
//! answers.
//!
//! The browser starts a run with every query at once and polls it every
//! second, fetching each answer as it lands. Stop ends the run at once: it
//! starts nothing new, and the queries in flight are dropped and read as never
//! run, so the next Run asks them again. Spiral has no way to cancel a
//! statement it is computing, so one in flight finishes the batch it is on
//! and its answer is thrown away. A run nobody polls only starts nothing new,
//! since the page that wanted it has gone. Runs live in memory: a restarted
//! backend holds none, as a reloaded page holds no preview.
use crate::OperationError;
use crate::clock::StableIdSequence;
use crate::data::query::QueryCommand;
use crate::sync::MutexRecovery;
use futures_util::StreamExt as _;
use spiral_robotics_common::curation::{PreviewQuery, PreviewQueryState, PreviewRunStatus};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_storage::StorageManager;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::AbortHandle;
use tokio::time::Instant;

/// How many of a run's queries are in flight at once.
const QUERIES_IN_FLIGHT: usize = 8;
/// How long a run goes unpolled before it starts nothing new, and a settled
/// run before it is dropped.
const ABANDONED_AFTER: Duration = Duration::from_secs(30);

/// One query's answer, in the form it was asked for.
#[derive(Clone)]
pub(super) enum PreviewAnswer {
    Rows(SpiralQuerySnapshot),
    Arrow(Vec<u8>),
}

/// The preview runs one project has started, by identity.
pub(crate) struct PreviewRuns {
    storage: Arc<StorageManager>,
    ids: StableIdSequence,
    runs: Mutex<BTreeMap<String, HeldRun>>,
}

/// A run and the task that works through its queries.
struct HeldRun {
    run: Arc<PreviewRun>,
    task: AbortHandle,
}

impl PreviewRuns {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            storage,
            ids: StableIdSequence::new("preview", 1),
            runs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Start working through `queries`, and say where the new run stands.
    pub(super) fn start(
        &self,
        queries: Vec<PreviewQuery>,
    ) -> Result<PreviewRunStatus, OperationError> {
        if queries.is_empty() {
            return Err(OperationError::invalid_input(
                "A preview run needs at least one query.",
            ));
        }
        self.forget_abandoned();
        let (id, _) = self.ids.next().into_parts();
        let run = Arc::new(PreviewRun::new(queries));
        let task = tokio::spawn(run.clone().answer_all(self.storage.clone())).abort_handle();
        let status = run.status(&id);
        self.runs.recover_lock().insert(id, HeldRun { run, task });
        Ok(status)
    }

    /// Where run `id` stands. Reading it is what keeps the run wanted.
    pub(super) fn status(&self, id: &str) -> Result<PreviewRunStatus, OperationError> {
        self.forget_abandoned();
        let run = self.run(id)?;
        run.polled();
        Ok(run.status(id))
    }

    /// End run `id` now: start none of its remaining queries, and drop the
    /// ones in flight.
    pub(super) fn stop(&self, id: &str) -> Result<(), OperationError> {
        let runs = self.runs.recover_lock();
        let held = runs.get(id).ok_or_else(Self::not_held)?;
        held.run.stop();
        held.task.abort();
        Ok(())
    }

    /// What query `query` of run `id` answered.
    pub(super) fn answer(&self, id: &str, query: usize) -> Result<PreviewAnswer, OperationError> {
        self.run(id)?.answer(query)
    }

    /// Abandon every run, including the queries in flight: the project they
    /// read is closing.
    pub(super) fn shutdown(&self) {
        for held in std::mem::take(&mut *self.runs.recover_lock()).into_values() {
            held.task.abort();
        }
    }

    fn run(&self, id: &str) -> Result<Arc<PreviewRun>, OperationError> {
        self.runs
            .recover_lock()
            .get(id)
            .map(|held| held.run.clone())
            .ok_or_else(Self::not_held)
    }

    fn not_held() -> OperationError {
        OperationError::NotFound("This preview run is no longer held; run the node again.".into())
    }

    /// Drop the settled runs nobody has asked about since they settled.
    fn forget_abandoned(&self) {
        self.runs
            .recover_lock()
            .retain(|_, held| !held.run.is_abandoned());
    }
}

/// One run: its queries, and what has become of each.
struct PreviewRun {
    queries: Vec<PreviewQuery>,
    progress: Mutex<RunProgress>,
}

/// The part of a run its task and its polls both change.
struct RunProgress {
    states: Vec<PreviewQueryState>,
    answers: BTreeMap<usize, PreviewAnswer>,
    stopped: bool,
    polled: Instant,
}

impl PreviewRun {
    fn new(queries: Vec<PreviewQuery>) -> Self {
        let progress = RunProgress {
            states: vec![PreviewQueryState::Queued; queries.len()],
            answers: BTreeMap::new(),
            stopped: false,
            polled: Instant::now(),
        };
        Self {
            queries,
            progress: Mutex::new(progress),
        }
    }

    /// Answer every query, a few at a time, until they are done or the run
    /// is stopped or abandoned.
    async fn answer_all(self: Arc<Self>, storage: Arc<StorageManager>) {
        futures_util::stream::iter(0..self.queries.len())
            .take_while(|_| std::future::ready(self.starts_more()))
            .map(|query| self.answer_one(query, &storage))
            .buffer_unordered(QUERIES_IN_FLIGHT)
            .collect::<()>()
            .await;
    }

    /// Whether another query may start. A run nobody has polled for a while
    /// stops, as Stop would.
    fn starts_more(&self) -> bool {
        let mut progress = self.progress.recover_lock();
        if progress.polled.elapsed() > ABANDONED_AFTER {
            progress.stopped = true;
        }
        !progress.stopped
    }

    async fn answer_one(&self, query: usize, storage: &StorageManager) {
        {
            // A query the stream handed over as Stop landed never starts.
            let mut progress = self.progress.recover_lock();
            if progress.stopped {
                return;
            }
            progress.states[query] = PreviewQueryState::Running;
        }
        let answered = Self::ask(&self.queries[query], storage).await;
        let mut progress = self.progress.recover_lock();
        progress.states[query] = match answered {
            Ok(answer) => {
                progress.answers.insert(query, answer);
                PreviewQueryState::Answered
            }
            Err(reason) => PreviewQueryState::Failed(reason),
        };
    }

    /// Run one query the way its own endpoint would.
    async fn ask(query: &PreviewQuery, storage: &StorageManager) -> Result<PreviewAnswer, String> {
        let queries = storage.queries();
        match query {
            PreviewQuery::EpisodeRows(request) => {
                let request = QueryCommand::admitted_for_episode(request.clone())
                    .map_err(|error| error.to_string())?;
                queries
                    .query_for_episode(&request)
                    .await
                    .map(PreviewAnswer::Rows)
            }
            PreviewQuery::EpisodeArrow(request) => queries
                .arrow_for_episode(request)
                .await
                .map(PreviewAnswer::Arrow),
            PreviewQuery::Rows(request) => {
                let command =
                    QueryCommand::admitted(request.clone()).map_err(|error| error.to_string())?;
                queries
                    .query(command.statement(), command.max_rows())
                    .await
                    .map(PreviewAnswer::Rows)
            }
        }
    }

    fn status(&self, id: &str) -> PreviewRunStatus {
        let progress = self.progress.recover_lock();
        PreviewRunStatus::new(id.to_owned(), progress.states.clone(), progress.stopped)
    }

    fn polled(&self) {
        self.progress.recover_lock().polled = Instant::now();
    }

    /// Start nothing more, and read every query in flight as never run: its
    /// task is being dropped, so it will not land.
    fn stop(&self) {
        let mut progress = self.progress.recover_lock();
        progress.stopped = true;
        for state in &mut progress.states {
            if *state == PreviewQueryState::Running {
                *state = PreviewQueryState::Queued;
            }
        }
    }

    fn answer(&self, query: usize) -> Result<PreviewAnswer, OperationError> {
        self.progress
            .recover_lock()
            .answers
            .get(&query)
            .cloned()
            .ok_or_else(|| OperationError::NotFound("That query has not been answered.".into()))
    }

    fn is_abandoned(&self) -> bool {
        let progress = self.progress.recover_lock();
        PreviewQueryState::settled(&progress.states, progress.stopped)
            && progress.polled.elapsed() > ABANDONED_AFTER
    }
}

#[cfg(test)]
mod tests;
