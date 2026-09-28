//! The provider keys this Studio's backend holds. The page writes new keys and
//! learns only which ones are saved.

use dioxus::prelude::*;
use spiral_robotics_common::providers::ProviderKeysUpdate;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Field, Notice, NoticeTone, TextInput, TextInputKind,
    Toggle,
};

use crate::activities::settings::SettingsClient;
use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::client::transport::CommandError;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

/// What names a key on the page.
#[derive(Clone, Copy, PartialEq)]
struct KeyCopy {
    label: &'static str,
    placeholder: &'static str,
    help: &'static str,
}

impl KeyCopy {
    const LLM: Self = Self {
        label: "OpenRouter API key",
        placeholder: "sk-or-…",
        help: "Used for Studio's model calls, including Curation's model functions.",
    };
    const HUGGING_FACE: Self = Self {
        label: "Hugging Face token",
        placeholder: "hf_…",
        help: "Used by Spiral and Studio's in-memory HTTP clients to download models and datasets from Hugging Face.",
    };
}

/// A key being edited: what was typed, or that the saved key goes.
#[derive(Clone, Copy, PartialEq)]
struct KeyDraft {
    typed: Signal<String>,
    remove: Signal<bool>,
}

impl KeyDraft {
    fn use_draft() -> Self {
        Self {
            typed: use_signal(String::new),
            remove: use_signal(|| false),
        }
    }

    fn changed(&self) -> bool {
        (self.remove)() || !(self.typed)().trim().is_empty()
    }

    /// The change to send: none keeps the saved key, an empty one removes it.
    fn change(&self) -> Option<String> {
        if *self.remove.peek() {
            return Some(String::new());
        }
        let typed = self.typed.peek().clone();
        (!typed.trim().is_empty()).then_some(typed)
    }

    fn clear(mut self) {
        self.typed.set(String::new());
        self.remove.set(false);
    }
}

#[component]
pub(super) fn ProviderKeysSection(project: StudioProjectState) -> Element {
    let status = QueryPolicy::on_demand(QueryDomain::Settings)
        .use_resource(|| SettingsClient::current().provider_keys());
    let llm_api_key = KeyDraft::use_draft();
    let huggingface_token = KeyDraft::use_draft();
    let mut saving = use_signal(|| false);
    let mut failure = use_signal(|| None::<CommandError>);
    let save = move |_| {
        if saving() {
            return;
        }
        let update = ProviderKeysUpdate::default()
            .with_llm_api_key(llm_api_key.change())
            .with_huggingface_token(huggingface_token.change());
        saving.set(true);
        failure.set(None);
        ProjectClient::current().spawn(async move {
            match SettingsClient::current().update_provider_keys(update).await {
                Ok(_) => {
                    llm_api_key.clear();
                    huggingface_token.clear();
                    project.report_success_from(MessageSource::Settings, "Keys saved.");
                }
                Err(error) => failure.set(Some(error)),
            }
            saving.set(false);
        });
    };
    let changed = llm_api_key.changed() || huggingface_token.changed();
    let current = status.read().clone();
    rsx! {
        match current {
            None => rsx! { p { class: "settings-help", role: "status", "Loading saved keys…" } },
            Some(Err(error)) => rsx! { Notice { tone: NoticeTone::Caution, "{error}" } },
            Some(Ok(saved)) => rsx! {
                KeyField { copy: KeyCopy::LLM, saved: saved.llm_api_key_configured(), draft: llm_api_key, disabled: saving() }
                KeyField { copy: KeyCopy::HUGGING_FACE, saved: saved.huggingface_token_configured(), draft: huggingface_token, disabled: saving() }
                Button {
                    purpose: ActionPurpose::Apply,
                    variant: ButtonVariant::Solid,
                    disabled: saving() || !changed,
                    aria_busy: saving(),
                    onclick: save,
                    if saving() { "Saving…" } else { "Save keys" }
                }
            },
        }
        if let Some(error) = failure() {
            Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
        }
    }
}

/// One write-only key: typing replaces the saved key, and a saved key can be removed.
#[component]
fn KeyField(copy: KeyCopy, saved: bool, draft: KeyDraft, disabled: bool) -> Element {
    let KeyDraft {
        mut typed,
        mut remove,
    } = draft;
    rsx! {
        div { class: "settings-key",
            Field { label: copy.label, class: "settings-field",
                TextInput {
                    kind: TextInputKind::Password,
                    value: typed(),
                    placeholder: if saved { "Saved — leave blank to keep" } else { copy.placeholder },
                    disabled,
                    oninput: move |value| {
                        typed.set(value);
                        remove.set(false);
                    },
                }
            }
            p { class: "settings-help", "{copy.help}" }
            if saved {
                Toggle {
                    label: format!("Remove saved {}", copy.label),
                    checked: remove(),
                    disabled,
                    onchange: move |value| {
                        remove.set(value);
                        typed.set(String::new());
                    },
                }
                if remove() {
                    p { class: "settings-help", role: "status", "It will be removed when you save keys." }
                }
            }
        }
    }
}
