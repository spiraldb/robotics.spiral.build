//! Shell utility-view transitions. Primary activity navigation belongs to the router.

use dioxus::prelude::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShellState {
    settings_open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShellAction {
    OpenSettings,
    CloseSettings,
}

impl ShellState {
    pub(crate) fn reduce(&self, action: ShellAction) -> Self {
        let mut next = self.clone();
        match action {
            ShellAction::OpenSettings => next.settings_open = true,
            ShellAction::CloseSettings => next.settings_open = false,
        }
        next
    }
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
    pub(super) fn settings_open(&self) -> bool {
        self.state.read().settings_open
    }

    pub(super) fn dispatch(mut self, action: ShellAction) {
        let current = self.state.peek();
        let next = current.reduce(action);
        let changed = *current != next;
        drop(current);
        if changed {
            self.state.set(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_view_state_is_independent_from_routing() {
        let initial = ShellState::default();
        let open = initial.reduce(ShellAction::OpenSettings);
        let closed = open.reduce(ShellAction::CloseSettings);

        assert!(open.settings_open);
        assert!(!closed.settings_open);
    }
}
