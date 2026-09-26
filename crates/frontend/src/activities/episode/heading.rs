//! What an episode inspection is showing, named once above it on one line:
//! the way back when a surface has one, the subject — the episode, or a part
//! of it — the facts that place it, and at the end what can be done from
//! here. Data's episode tabs and every Curation node head the shared
//! inspection with it, so a recording is introduced the same way wherever it
//! is opened.
use super::identity::Episode;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// What a heading names: an episode, or a part of one, and the facts that
/// place it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct HeadingSubject {
    words: String,
    /// The episode the subject belongs to, when the subject is not the
    /// episode itself.
    part_of: Option<EpisodeId>,
    /// What the subject's tooltip says: an episode's whole id, a part's
    /// whole words, since the line may cut either.
    tooltip: String,
    facts: Vec<String>,
}

impl HeadingSubject {
    /// An episode, named as every surface names it; its whole id is the
    /// name's tooltip, since the name already says its tail.
    pub(in crate::activities) fn episode(episode: &Episode) -> Self {
        Self {
            words: episode.name(),
            part_of: None,
            tooltip: episode.id().to_string(),
            facts: Vec::new(),
        }
    }
    /// A part of `episode` — a subtask, a stretch — called `words`, with the
    /// episode's id beside it as text somebody can select and paste.
    pub(in crate::activities) fn part(words: impl Into<String>, episode: &EpisodeId) -> Self {
        let words = words.into();
        Self {
            tooltip: words.clone(),
            words,
            part_of: Some(episode.clone()),
            facts: Vec::new(),
        }
    }
    pub(in crate::activities) fn with_facts(mut self, facts: Vec<String>) -> Self {
        self.facts = facts;
        self
    }
}

/// The one heading over an episode inspection. `lead` is the way back to
/// what holds the episode, when the surface has one; `actions` end the line.
#[component]
pub(in crate::activities) fn EpisodeHeading(
    subject: HeadingSubject,
    #[props(default = VNode::empty())] lead: Element,
    #[props(default = VNode::empty())] actions: Element,
) -> Element {
    let HeadingSubject {
        words,
        part_of,
        tooltip,
        facts,
    } = subject;
    rsx! {
        header { class: "episode-heading",
            div { class: "episode-heading-path",
                {lead}
                h2 { class: "episode-heading-subject", title: "{tooltip}", "{words}" }
            }
            p { class: "episode-heading-facts",
                if let Some(episode) = part_of {
                    code { class: "episode-heading-id", title: "{episode}", "{episode}" }
                }
                for fact in facts {
                    span { "{fact}" }
                }
            }
            div { class: "episode-heading-actions", {actions} }
        }
    }
}
