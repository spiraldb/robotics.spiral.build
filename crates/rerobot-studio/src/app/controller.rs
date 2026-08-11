//! Pure shell transitions plus their Dioxus-owned state handle.

use dioxus::prelude::*;

use crate::app::activity::Activity;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShellState {
    pub activity: Activity,
    pub settings_open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShellAction {
    SelectActivity(Activity),
    OpenSettings,
    CloseSettings,
    OpenEpisode(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShellEffect {
    SelectEpisode(String),
}

pub(crate) fn reduce_shell(
    state: &ShellState,
    action: ShellAction,
) -> (ShellState, Option<ShellEffect>) {
    let mut next = state.clone();
    let effect = match action {
        ShellAction::SelectActivity(activity) => {
            next.activity = activity;
            None
        }
        ShellAction::OpenSettings => {
            next.settings_open = true;
            None
        }
        ShellAction::CloseSettings => {
            next.settings_open = false;
            None
        }
        ShellAction::OpenEpisode(episode) => {
            next.activity = Activity::Data;
            Some(ShellEffect::SelectEpisode(episode))
        }
    };
    (next, effect)
}

#[derive(Clone, Copy)]
pub(crate) struct ShellController {
    state: Signal<ShellState>,
}

pub(crate) fn use_shell_controller() -> ShellController {
    let state = use_signal(ShellState::default);
    use_context_provider(|| ShellController { state })
}

impl ShellController {
    pub fn activity(&self) -> Activity {
        self.state.read().activity
    }

    pub fn settings_open(&self) -> bool {
        self.state.read().settings_open
    }

    pub fn dispatch(mut self, action: ShellAction) -> Option<ShellEffect> {
        let current = self.state.peek();
        let (next, effect) = reduce_shell(&current, action);
        let changed = *current != next;
        drop(current);
        if changed {
            self.state.set(next);
        }
        effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_in_data_is_one_atomic_transition_with_one_effect() {
        let initial = ShellState::default();
        let (next, effect) = reduce_shell(&initial, ShellAction::OpenEpisode("take-42".to_owned()));

        assert_eq!(next.activity, Activity::Data);
        assert_eq!(effect, Some(ShellEffect::SelectEpisode("take-42".into())));
        assert!(!next.settings_open);
    }

    #[test]
    fn activity_switches_do_not_rewrite_settings_state() {
        let open = ShellState {
            settings_open: true,
            ..ShellState::default()
        };
        let (next, effect) = reduce_shell(&open, ShellAction::SelectActivity(Activity::Training));

        assert_eq!(next.activity, Activity::Training);
        assert!(next.settings_open);
        assert_eq!(effect, None);
    }
}
