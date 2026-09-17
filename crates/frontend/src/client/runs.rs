//! Browser-side reconciliation of polled run records. A poll may answer
//! late, out of order, or with a stale copy; the ledger only ever moves each
//! run forward, as the shared observation contract defines.
use super::CommandError;
use dioxus::prelude::*;
use spiral_robotics_common::runs::{ObservedRun, RunObservation};

pub(crate) trait RunLedger<Run> {
    /// Merge polled records: new runs are added, known runs advance only when
    /// the observation contract accepts the incoming snapshot.
    fn accept_runs(&mut self, incoming: Vec<Run>);
    /// Subscribe to a polling resource and merge every finished read.
    fn observe(self, reads: Resource<Result<Vec<Run>, CommandError>>);
}

impl<Run> RunLedger<Run> for Signal<Vec<Run>>
where
    Run: ObservedRun + Clone + PartialEq + 'static,
{
    fn accept_runs(&mut self, incoming: Vec<Run>) {
        let changed = incoming.iter().any(|run| {
            self.peek()
                .iter()
                .find(|current| current.snapshot().run_id() == run.snapshot().run_id())
                .is_none_or(|current| current != run && current.snapshot().accepts(run.snapshot()))
        });
        if !changed {
            return;
        }
        let mut runs = self.write();
        for run in incoming {
            match runs
                .iter_mut()
                .find(|current| current.snapshot().run_id() == run.snapshot().run_id())
            {
                Some(current) if current.snapshot().accepts(run.snapshot()) => *current = run,
                Some(_) => {}
                None => runs.push(run),
            }
        }
    }

    fn observe(mut self, reads: Resource<Result<Vec<Run>, CommandError>>) {
        use_effect(move || {
            // Subscribe even while the first request is pending; finished() is nonreactive.
            let records = reads.read();
            if reads.finished()
                && let Some(Ok(records)) = records.as_ref()
            {
                self.accept_runs(records.clone());
            }
        });
    }
}
