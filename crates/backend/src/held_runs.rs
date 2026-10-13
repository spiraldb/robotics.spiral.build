//! The background runs one project starts — a curation's fill runs, a
//! dataset's import — each held under an id until it is forgotten.
//! A run lives in memory: a restarted backend holds none.
use crate::OperationError;
use crate::clock::StableIdSequence;
use crate::sync::MutexRecovery;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::AbortHandle;

/// How long a run goes unpolled before a run that needs a reader starts
/// nothing new, and a settled run before it is dropped.
pub(crate) const ABANDONED_AFTER: Duration = Duration::from_secs(30);

/// What the registry asks of a run it holds.
pub(crate) trait HeldWork: Send + Sync + 'static {
    /// Whether nothing will ask about the run again, so it may be dropped.
    fn is_forgotten(&self) -> bool;
}

/// The runs of one kind a project has started, by id.
pub(crate) struct HeldRuns<R> {
    ids: StableIdSequence,
    runs: Mutex<BTreeMap<String, HeldRun<R>>>,
}

/// A run and the task that works it.
struct HeldRun<R> {
    run: Arc<R>,
    task: AbortHandle,
}

impl<R: HeldWork> HeldRuns<R> {
    pub(crate) fn new(prefix: &'static str) -> Self {
        Self {
            ids: StableIdSequence::new(prefix, 1),
            runs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Hold `run` under a new id, worked by `work` from now on.
    pub(crate) fn start(
        &self,
        run: Arc<R>,
        work: impl Future<Output = ()> + Send + 'static,
    ) -> String {
        self.forget();
        let (id, _) = self.ids.next().into_parts();
        let task = tokio::spawn(work).abort_handle();
        self.runs
            .recover_lock()
            .insert(id.clone(), HeldRun { run, task });
        id
    }

    /// Run `id`, or why it is not held.
    pub(crate) fn get(&self, id: &str) -> Result<Arc<R>, OperationError> {
        self.runs
            .recover_lock()
            .get(id)
            .map(|held| held.run.clone())
            .ok_or_else(|| {
                OperationError::NotFound("This run is no longer held; run it again.".into())
            })
    }

    /// Every run held, by id.
    pub(crate) fn all(&self) -> Vec<(String, Arc<R>)> {
        self.forget();
        self.runs
            .recover_lock()
            .iter()
            .map(|(id, held)| (id.clone(), held.run.clone()))
            .collect()
    }

    /// End and drop every run `replaced` picks.
    pub(crate) fn replace(&self, replaced: impl Fn(&R) -> bool) {
        self.runs.recover_lock().retain(|_, held| {
            let keep = !replaced(&held.run);
            if !keep {
                held.task.abort();
            }
            keep
        });
    }

    /// End and drop every run: the project they work on is closing.
    pub(crate) fn shutdown(&self) {
        for held in std::mem::take(&mut *self.runs.recover_lock()).into_values() {
            held.task.abort();
        }
    }

    /// Drop every run nothing will ask about again.
    pub(crate) fn forget(&self) {
        self.runs
            .recover_lock()
            .retain(|_, held| !held.run.is_forgotten());
    }
}
