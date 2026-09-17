//! One lifecycle for every backend-owned native query run, whether a
//! workflow's SQL or a curation's compiled recipe: the phases a run moves
//! through and how a browser reconciles observations of it.
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    #[default]
    Idle,
    Validating,
    Running,
    Completed,
    Failed,
    Interrupted,
}

impl RunPhase {
    pub const fn active(self) -> bool {
        matches!(self, Self::Validating | Self::Running)
    }
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Interrupted)
    }
    /// Phases only advance; every terminal phase ranks the same.
    const fn monotonic_rank(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Validating => 1,
            Self::Running => 2,
            Self::Completed | Self::Failed | Self::Interrupted => 4,
        }
    }
}

/// What a browser needs from any run snapshot to merge a newer observation
/// without letting a late or stale response regress what it already knows.
pub trait RunObservation {
    /// The document this run judged and the revision it was submitted at.
    fn document(&self) -> (&str, u64);
    fn run_id(&self) -> Option<&str>;
    fn phase(&self) -> RunPhase;
    fn started_ms(&self) -> u64;
    fn updated_ms(&self) -> u64;
    /// Whether `incoming` may replace this observation.
    fn accepts(&self, incoming: &Self) -> bool {
        if incoming.document() != self.document() {
            return false;
        }
        if incoming.run_id() != self.run_id() {
            return match (self.run_id(), incoming.run_id()) {
                (None, Some(_)) | (None, None) => true,
                (Some(_), None) => false,
                (Some(_), Some(_)) => incoming.started_ms() > self.started_ms(),
            };
        }
        if incoming.updated_ms() != self.updated_ms() {
            return incoming.updated_ms() > self.updated_ms();
        }
        incoming.phase().monotonic_rank() >= self.phase().monotonic_rank()
    }
}

/// A run record that carries its observable snapshot.
pub trait ObservedRun {
    type Snapshot: RunObservation;
    fn snapshot(&self) -> &Self::Snapshot;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Probe {
        run_id: Option<&'static str>,
        phase: RunPhase,
        started_ms: u64,
        updated_ms: u64,
    }
    impl RunObservation for Probe {
        fn document(&self) -> (&str, u64) {
            ("doc", 1)
        }
        fn run_id(&self) -> Option<&str> {
            self.run_id
        }
        fn phase(&self) -> RunPhase {
            self.phase
        }
        fn started_ms(&self) -> u64 {
            self.started_ms
        }
        fn updated_ms(&self) -> u64 {
            self.updated_ms
        }
    }

    #[test]
    fn observations_only_move_forward() {
        let running = Probe {
            run_id: Some("a"),
            phase: RunPhase::Running,
            started_ms: 10,
            updated_ms: 20,
        };
        let mut later = running.clone();
        later.phase = RunPhase::Completed;
        later.updated_ms = 30;
        assert!(running.accepts(&later));
        assert!(!later.accepts(&running), "an older observation never wins");
        let mut same_time_regression = later.clone();
        same_time_regression.phase = RunPhase::Running;
        assert!(!later.accepts(&same_time_regression));
        let newer_run = Probe {
            run_id: Some("b"),
            started_ms: 11,
            ..running.clone()
        };
        assert!(running.accepts(&newer_run));
        assert!(!newer_run.accepts(&running));
        let idle = Probe {
            run_id: None,
            ..running.clone()
        };
        assert!(idle.accepts(&running));
        assert!(!running.accepts(&idle));
    }
}
