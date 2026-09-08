//! Keyboard shortcut admission for the mounted studio.
use crate::activities::live::session::LiveSession;
impl LiveSession {
    /// Whether global transport shortcuts may act without stealing a text-editing
    /// keystroke.
    pub(crate) fn shortcuts_enabled(self) -> bool {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return false;
        };
        document
            .active_element()
            .map(|element| {
                !matches!(
                    element.tag_name().to_uppercase().as_str(),
                    "INPUT" | "TEXTAREA" | "SELECT"
                )
            })
            .unwrap_or(true)
    }
}
