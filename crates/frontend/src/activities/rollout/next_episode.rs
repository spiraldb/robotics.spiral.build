//! What Record starts next: the dataset the episode joins, whether it films the
//! arms, and what drives them.

use std::collections::BTreeSet;

use dioxus::prelude::*;

use crate::components::dataset_catalog::DatasetCatalog;

/// What moves the follower through the next episode. Record starts it with the
/// episode, and the episode ends with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlKind {
    /// The operator moves the leader; the follower mirrors it.
    Teleop,
    /// The chosen checkpoint drives the follower for its duration.
    Policy,
}

impl ControlKind {
    /// What the Record button says it starts.
    pub(super) const fn record_label(self) -> &'static str {
        match self {
            Self::Teleop => "Record teleop",
            Self::Policy => "Record policy",
        }
    }

    /// What Record does when nothing blocks it.
    pub(super) const fn record_description(self) -> &'static str {
        match self {
            Self::Teleop => "Start teleoperation and record a new episode",
            Self::Policy => "Start the policy and record a new episode",
        }
    }
}

/// The next episode as the operator sets it up.
///
/// The dataset is not a choice from the catalog: recording into a dataset that
/// does not exist yet is how most datasets start, so the operator types a name
/// and the catalog only offers what is already there as suggestions.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct NextEpisode {
    dataset: Signal<String>,
    video: Signal<bool>,
    control: Signal<Option<ControlKind>>,
    starting: Signal<bool>,
    catalog: DatasetCatalog,
}

impl NextEpisode {
    pub(super) fn use_next_episode() -> Self {
        Self {
            dataset: use_signal(|| "default".to_owned()),
            video: use_signal(|| true),
            control: use_signal(|| None),
            starting: use_signal(|| false),
            catalog: DatasetCatalog::use_catalog(),
        }
    }

    /// The dataset name new episodes are recorded into.
    pub(super) const fn dataset(self) -> Signal<String> {
        self.dataset
    }

    /// Whether new episodes record the cameras bound to their arms.
    pub(super) const fn video(self) -> Signal<bool> {
        self.video
    }

    pub(super) const fn catalog(self) -> DatasetCatalog {
        self.catalog
    }

    /// The control source the operator last started, or `default` before they have.
    pub(super) fn control(self, default: ControlKind) -> ControlKind {
        self.control.read().unwrap_or(default)
    }

    /// Starting a control source by hand makes it the one Record starts next.
    pub(super) fn choose_control(mut self, control: ControlKind) {
        self.control.set(Some(control));
    }

    /// Whether Record is waiting for its control source to take the follower.
    pub(super) fn starting(self) -> bool {
        *self.starting.read()
    }

    pub(super) fn set_starting(mut self, starting: bool) {
        self.starting.set(starting);
    }

    /// The datasets an episode may join, plus whatever is currently typed.
    pub(super) fn suggestions(self) -> BTreeSet<String> {
        self.catalog
            .recording_destinations()
            .into_iter()
            .chain(std::iter::once(self.dataset.read().to_owned()))
            .collect()
    }
}
