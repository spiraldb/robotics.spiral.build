//! Top-level Studio navigation and resource requirements.

use dioxus::prelude::*;

/// Every top-level Studio view, including Settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum Activity {
    #[default]
    Data,
    Curation,
    Training,
    Rollout,
    Settings,
}

impl Activity {
    pub(super) const fn all() -> [Self; 5] {
        [
            Self::Data,
            Self::Curation,
            Self::Training,
            Self::Rollout,
            Self::Settings,
        ]
    }

    /// The activities the top bar lists, in shortcut order: the data core
    /// first, what consumes it next, and the bench that drives the arms and
    /// records into it last. Settings sits apart at the end of the bar.
    pub(super) const fn primary() -> [Self; 4] {
        [Self::Data, Self::Curation, Self::Training, Self::Rollout]
    }

    /// The key that opens this activity from anywhere on the page.
    pub(super) const fn shortcut(self) -> &'static str {
        match self {
            Self::Data => "1",
            Self::Curation => "2",
            Self::Training => "3",
            Self::Rollout => "4",
            Self::Settings => ",",
        }
    }

    pub(super) fn from_shortcut(key: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|activity| activity.shortcut() == key)
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Data => "Data",
            Self::Curation => "Curation",
            Self::Training => "Training",
            Self::Rollout => "Rollout",
            Self::Settings => "Settings",
        }
    }
}

/// The selected route determines visibility; each workspace owns its inactivity policy.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WorkspaceActivity {
    activity: Activity,
    is_active: ReadSignal<bool>,
}

impl WorkspaceActivity {
    // Camera and chart primitives are nested through panel/layout components that
    // do not use activity. Direct consumers receive this handle as a prop.
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

    pub(super) const fn activity(self) -> Activity {
        self.activity
    }
}
