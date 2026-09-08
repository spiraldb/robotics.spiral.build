//! Top-level Studio navigation and resource requirements.

use dioxus::prelude::*;

/// Every top-level Studio view, including Settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum Activity {
    #[default]
    Live,
    Data,
    Workflow,
    Training,
    Rollout,
    Settings,
}

impl Activity {
    pub(crate) const fn all() -> [Self; 6] {
        [
            Self::Live,
            Self::Data,
            Self::Workflow,
            Self::Training,
            Self::Rollout,
            Self::Settings,
        ]
    }

    pub(crate) const fn uses_hardware(self) -> bool {
        matches!(self, Self::Live | Self::Rollout | Self::Settings)
    }
    pub(crate) const fn uses_episode_catalog(self) -> bool {
        matches!(self, Self::Live | Self::Training)
    }
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Data => "Data",
            Self::Workflow => "Workflow",
            Self::Training => "Training",
            Self::Rollout => "Rollout",
            Self::Settings => "Settings",
        }
    }
}

/// The selected route determines visibility; each workspace owns its inactivity policy.
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceActivity {
    activity: Activity,
    is_active: ReadSignal<bool>,
}

impl WorkspaceActivity {
    pub(crate) fn provide(activity: Activity, is_active: ReadSignal<bool>) -> Self {
        use_context_provider(|| Self {
            activity,
            is_active,
        })
    }

    pub(crate) fn current() -> Self {
        use_context()
    }

    pub(crate) fn is_active(self) -> bool {
        (self.is_active)()
    }

    pub(crate) const fn is_active_signal(self) -> ReadSignal<bool> {
        self.is_active
    }

    pub(crate) const fn activity(self) -> Activity {
        self.activity
    }
}
