//! Top-level Studio navigation and resource requirements.

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
