//! The line above a result: which run produced it, how that run ended, and
//! what it measured.
//!
//! Workflow and Curation submit the same run lifecycle and print the same strip
//! over their results; Rollout and Training say the same thing about their own
//! jobs. The phase dot, the run list, and their wording live here once, and each
//! activity supplies only the facts that are its own — rows, judged episodes,
//! elapsed time, the SQL a run executed.

use dioxus::prelude::*;
use spiral_robotics_common::runs::RunPhase;
use spiral_robotics_ui::Select;

/// How a run's phase is spoken, the same in every activity that runs one —
/// in the strip, and wherever else a phase is reported on its own.
pub(crate) trait RunPhasePresentation {
    fn run_label(self) -> &'static str;
}

impl RunPhasePresentation for RunPhase {
    fn run_label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Running => "Running",
            Self::Completed => "Complete",
            Self::Failed => "Failed",
        }
    }
}

/// How long a run took, in the unit a person would say.
pub(crate) struct RunDuration(u64);

impl RunDuration {
    pub(crate) const fn millis(millis: u64) -> Self {
        Self(millis)
    }
}

impl std::fmt::Display for RunDuration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 < 1_000 {
            write!(formatter, "{} ms", self.0)
        } else if self.0 < 60_000 {
            write!(formatter, "{:.1} s", self.0 as f64 / 1000.0)
        } else {
            let seconds = self.0 / 1000;
            write!(formatter, "{}m {:02}s", seconds / 60, seconds % 60)
        }
    }
}

/// One run as the strip lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunChoice {
    id: String,
    /// Which run of how many, one-based, as `Run 2 of 5` reads.
    place: (usize, usize),
    phase: RunPhase,
}

impl RunChoice {
    /// `place` is `(this run's number, how many runs there are)`, counting from
    /// one, as the operator reads them: oldest run first.
    pub(crate) fn new(id: impl Into<String>, place: (usize, usize), phase: RunPhase) -> Self {
        Self {
            id: id.into(),
            place,
            phase,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    fn phase_label(&self) -> &'static str {
        self.phase.run_label()
    }

    /// How a run's phase is coloured.
    const fn phase_class(&self) -> &'static str {
        match self.phase {
            RunPhase::Idle => "is-idle",
            RunPhase::Running => "is-running",
            RunPhase::Completed => "is-complete",
            RunPhase::Failed => "is-failed",
        }
    }

    /// One line of the run list. The phase dot already names the shown run's
    /// state, so only runs that did not complete repeat it here.
    fn option_label(&self) -> String {
        let (number, total) = self.place;
        if self.phase == RunPhase::Completed {
            format!("Run {number} of {total}")
        } else {
            format!(
                "Run {number} of {total} · {}",
                self.phase_label().to_ascii_lowercase()
            )
        }
    }
}

/// The strip over a result: the shown run's phase, the list of runs when there
/// is more than one, and this activity's own facts about the shown run.
///
/// Draw it only when there is a run to describe; an activity with none shows its
/// empty state instead.
#[component]
pub(crate) fn RunStrip(
    /// The run list's element id, so its label points at it.
    id: String,
    /// Every run of the open document, oldest first.
    runs: Vec<RunChoice>,
    /// The run the strip describes — the selected one, or the latest.
    current: RunChoice,
    on_select: EventHandler<String>,
    /// What this activity measured about `current`: counts, elapsed time, a
    /// truncation badge, the executed SQL. Laid out after the run list.
    facts: Element,
) -> Element {
    rsx! {
        div { class: "run-strip",
            span { class: "run-phase {current.phase_class()}", role: "status",
                span { class: "run-phase-dot", "aria-hidden": "true" }
                "{current.phase_label()}"
            }
            if runs.len() > 1 {
                label { r#for: "{id}", class: "sr-only", "Run" }
                Select {
                    id: "{id}",
                    class: "run-select",
                    value: current.id().to_owned(),
                    onchange: move |chosen| on_select.call(chosen),
                    for choice in runs.iter() {
                        option {
                            key: "{choice.id()}",
                            value: "{choice.id()}",
                            selected: choice.id() == current.id(),
                            "{choice.option_label()}"
                        }
                    }
                }
            }
            {facts}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_run_that_did_not_complete_repeats_its_phase_in_the_list() {
        let completed = RunChoice::new("a", (2, 5), RunPhase::Completed);
        assert_eq!(completed.option_label(), "Run 2 of 5");
        assert_eq!(completed.phase_label(), "Complete");
        let running = RunChoice::new("b", (5, 5), RunPhase::Running);
        assert_eq!(running.option_label(), "Run 5 of 5 · running");
        let failed = RunChoice::new("c", (1, 5), RunPhase::Failed);
        assert_eq!(failed.option_label(), "Run 1 of 5 · failed");
    }

    #[test]
    fn a_run_reports_its_time_in_the_unit_a_person_would_say() {
        assert_eq!(RunDuration::millis(640).to_string(), "640 ms");
        assert_eq!(RunDuration::millis(12_400).to_string(), "12.4 s");
        assert_eq!(RunDuration::millis(125_000).to_string(), "2m 05s");
    }
}
