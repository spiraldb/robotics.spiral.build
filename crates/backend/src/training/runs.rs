//! Pure lifecycle policy for native training runs.
//!
//! The manager adapter maps persisted and wire-facing run DTOs into these
//! plain records. Admission, transition legality, and history retention
//! therefore stay independent of serialization, filesystems, task runtimes,
//! and the native trainer.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunPhase {
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl RunPhase {
    const fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    fn may_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Starting => matches!(
                next,
                Self::Running | Self::Stopping | Self::Stopped | Self::Failed
            ),
            Self::Running => matches!(
                next,
                Self::Stopping | Self::Completed | Self::Stopped | Self::Failed
            ),
            Self::Stopping => matches!(next, Self::Stopped | Self::Failed),
            Self::Completed | Self::Stopped | Self::Failed => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunRecord {
    id: String,
    phase: RunPhase,
}

impl RunRecord {
    pub(super) fn new(id: impl Into<String>, phase: RunPhase) -> Self {
        Self {
            id: id.into(),
            phase,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunRegistry {
    runs: Vec<RunRecord>,
    history_limit: usize,
}

impl TrainingRunRegistry {
    pub(super) fn new(
        history_limit: usize,
        runs: impl IntoIterator<Item = RunRecord>,
    ) -> Result<Self, String> {
        if history_limit == 0 {
            return Err("training history limit must be positive".to_owned());
        }
        let mut runs = runs.into_iter().collect::<Vec<_>>();
        let mut ids = BTreeSet::new();
        if let Some(run) = runs
            .iter()
            .find(|run| run.id.trim().is_empty() || !ids.insert(run.id.clone()))
        {
            return Err(format!(
                "training history contains an empty or duplicate run id {:?}",
                run.id
            ));
        }
        if runs.iter().filter(|run| run.phase.active()).count() > 1 {
            return Err("training history contains more than one active run".to_owned());
        }
        runs.truncate(history_limit);
        Ok(Self {
            runs,
            history_limit,
        })
    }

    pub(super) fn admit(&mut self, run: RunRecord) -> Result<(), String> {
        if run.id.trim().is_empty() {
            return Err("training run id must not be empty".to_owned());
        }
        if !run.phase.active() {
            return Err("a newly admitted training run must be active".to_owned());
        }
        if self.runs.iter().any(|candidate| candidate.id == run.id) {
            return Err(format!(
                "training run id {:?} is already in history",
                run.id
            ));
        }
        if let Some(active) = self.runs.iter().find(|candidate| candidate.phase.active()) {
            return Err(format!(
                "training run {:?} is already active; stop it before starting another",
                active.id
            ));
        }
        self.runs.insert(0, run);
        self.runs.truncate(self.history_limit);
        Ok(())
    }

    pub(super) fn transition(&mut self, run_id: &str, next: RunPhase) -> Result<(), String> {
        let run = self
            .runs
            .iter_mut()
            .find(|run| run.id == run_id)
            .ok_or_else(|| format!("training run {run_id:?} is not in history"))?;
        let from = run.phase;
        if !from.may_transition_to(next) {
            return Err(format!(
                "training run {run_id:?} cannot transition from {from:?} to {next:?}"
            ));
        }
        run.phase = next;
        Ok(())
    }

    pub(super) fn retained_ids(&self) -> impl Iterator<Item = &str> {
        self.runs.iter().map(|run| run.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, phase: RunPhase) -> RunRecord {
        RunRecord {
            id: id.to_owned(),
            phase,
        }
    }

    #[test]
    fn admission_is_single_active_and_bounds_history() {
        let mut registry = TrainingRunRegistry::new(
            2,
            [
                run("oldest", RunPhase::Completed),
                run("newer", RunPhase::Failed),
            ],
        )
        .unwrap();
        registry.admit(run("active", RunPhase::Starting)).unwrap();
        assert_eq!(
            registry.retained_ids().collect::<Vec<_>>(),
            vec!["active", "oldest"]
        );
        assert!(
            registry
                .admit(run("other", RunPhase::Starting))
                .unwrap_err()
                .contains("already active")
        );
    }

    #[test]
    fn loaded_history_is_bounded_without_reordering_survivors() {
        let registry = TrainingRunRegistry::new(
            2,
            [
                run("newest", RunPhase::Completed),
                run("middle", RunPhase::Stopped),
                run("oldest", RunPhase::Failed),
            ],
        )
        .unwrap();

        assert_eq!(
            registry.retained_ids().collect::<Vec<_>>(),
            vec!["newest", "middle"]
        );
    }

    #[test]
    fn terminal_runs_cannot_be_resurrected() {
        let mut registry = TrainingRunRegistry::new(
            4,
            [
                run("complete", RunPhase::Completed),
                run("live", RunPhase::Running),
            ],
        )
        .unwrap();
        assert!(
            registry
                .transition("complete", RunPhase::Running)
                .unwrap_err()
                .contains("cannot transition")
        );
        registry.transition("live", RunPhase::Stopping).unwrap();
        assert_eq!(registry.runs[1].phase, RunPhase::Stopping);
        registry.transition("live", RunPhase::Stopped).unwrap();
        assert_eq!(registry.runs[1].phase, RunPhase::Stopped);
    }

    #[test]
    fn malformed_persisted_registry_is_rejected() {
        assert!(
            TrainingRunRegistry::new(
                4,
                [
                    run("same", RunPhase::Completed),
                    run("same", RunPhase::Failed)
                ]
            )
            .unwrap_err()
            .contains("duplicate")
        );
        assert!(
            TrainingRunRegistry::new(
                4,
                [
                    run("one", RunPhase::Running),
                    run("two", RunPhase::Starting)
                ]
            )
            .unwrap_err()
            .contains("more than one active")
        );
    }
}
