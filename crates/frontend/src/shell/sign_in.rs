//! What a browser without a Spiral session sees: one way to sign in, which
//! returns it to the page it was on.

use dioxus::prelude::*;
use spiral_robotics_common::account::SignInRequest;
use spiral_robotics_ui::{ActionPurpose, ButtonVariant, LinkButton};

/// The backend's sign-in page, returning to the page this browser shows now.
struct SignInLink;

impl SignInLink {
    fn here() -> String {
        let return_to = web_sys::window()
            .and_then(|window| {
                let location = window.location();
                Some(format!(
                    "{}{}",
                    location.pathname().ok()?,
                    location.search().ok()?
                ))
            })
            .unwrap_or_else(|| "/".to_owned());
        let query = serde_urlencoded::to_string(SignInRequest::new(return_to)).unwrap_or_default();
        format!("{}?{query}", SignInRequest::PATH)
    }
}

#[component]
pub(super) fn SignInGate() -> Element {
    let href = use_hook(SignInLink::here);
    rsx! {
        div { class: "studio-shell studio-shell-v2",
            div { class: "editor-empty sign-in-gate", role: "status",
                strong { "Sign in to continue" }
                LinkButton {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Solid,
                    href,
                    "Sign in with Spiral"
                }
            }
        }
    }
}
