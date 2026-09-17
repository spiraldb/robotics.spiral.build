//! How a native run's phase is spoken and coloured, the same in every
//! activity that polls one.
use spiral_robotics_common::runs::RunPhase;

pub(crate) trait RunPhasePresentation {
    fn run_label(self) -> &'static str;
    fn class_name(self) -> &'static str;
}

impl RunPhasePresentation for RunPhase {
    fn run_label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Validating => "Validating",
            Self::Running => "Running",
            Self::Completed => "Complete",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
        }
    }
    fn class_name(self) -> &'static str {
        match self {
            Self::Idle => "is-idle",
            Self::Validating => "is-validating",
            Self::Running => "is-running",
            Self::Completed => "is-complete",
            Self::Failed => "is-failed",
            Self::Interrupted => "is-interrupted",
        }
    }
}
