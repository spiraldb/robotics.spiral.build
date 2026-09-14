//! One readable answer per user turn, with the complete activity retained on demand.

use dioxus::prelude::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};
use spiral_robotics_common::workflow::{
    WorkflowAgentMessage, WorkflowAgentMessagePhase, WorkflowAgentRole, WorkflowAgentToolPhase,
};

use super::state::WorkflowController;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};

#[component]
pub(super) fn AssistantConversation(
    messages: Vec<WorkflowAgentMessage>,
    controller: WorkflowController,
) -> Element {
    rsx! {
        ol { class: "workflow-assistant-thread",
            for turn in AssistantTurn::collect(messages) {
                li { key: "{turn.key()}", class: "workflow-assistant-turn",
                    if let Some(request) = &turn.request {
                        div { class: "workflow-agent-message is-user", role: "group", "aria-label": "Your request",
                            AssistantMarkdown { text: request.text().to_owned() }
                        }
                    }
                    if turn.has_activity() {
                        details { class: "workflow-agent-tools-disclosure",
                            "data-failed": turn.failed(),
                            summary { "{turn.activity_label()}" }
                            div { class: "workflow-turn-activity",
                                for message in &turn.responses {
                                    if !message.text().is_empty() && Some(message.id()) != turn.answer().map(WorkflowAgentMessage::id) {
                                        div { role: "group", "aria-label": if message.role() == WorkflowAgentRole::Synthetic { "Workflow notice" } else { "Assistant finding" },
                                            AssistantMarkdown { text: message.text().to_owned() }
                                        }
                                    }
                                    for tool in message.tools() {
                                        div { class: "workflow-agent-tool-detail",
                                            strong { "{tool.summary()}" }
                                            if let Some(error) = tool.error() { p { class: "workflow-tool-error", "{error}" } }
                                            if let Some(receipt) = tool.receipt() {
                                                if controller.receipt_panel(receipt).is_some() {
                                                    {let target = receipt.to_owned();
                                                    rsx! { Action {
                                                        purpose: ActionPurpose::Navigate, variant: ActionVariant::Link,
                                                        onclick: move |_| controller.open_agent_receipt(target.clone()),
                                                        "{receipt.label()}"
                                                    } }}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(answer) = turn.answer() {
                        div { class: "workflow-agent-message is-assistant", role: "group", "aria-label": if answer.role() == WorkflowAgentRole::Synthetic { "Workflow notice" } else { "Assistant response" },
                            AssistantMarkdown { text: answer.text().to_owned() }
                        }
                    }
                }
            }
        }
    }
}

struct AssistantTurn {
    request: Option<WorkflowAgentMessage>,
    responses: Vec<WorkflowAgentMessage>,
}

impl AssistantTurn {
    fn collect(messages: Vec<WorkflowAgentMessage>) -> Vec<Self> {
        let mut turns: Vec<Self> = Vec::new();
        for message in messages {
            if message.role() == WorkflowAgentRole::User {
                turns.push(Self {
                    request: Some(message),
                    responses: Vec::new(),
                });
            } else {
                if turns.is_empty() {
                    turns.push(Self {
                        request: None,
                        responses: Vec::new(),
                    });
                }
                if let Some(turn) = turns.last_mut() {
                    turn.responses.push(message);
                }
            }
        }
        turns
    }

    fn key(&self) -> &str {
        self.request
            .as_ref()
            .or_else(|| self.responses.first())
            .map_or("empty", WorkflowAgentMessage::id)
    }

    fn answer(&self) -> Option<&WorkflowAgentMessage> {
        self.responses
            .iter()
            .rev()
            .find(|message| !message.text().trim().is_empty())
    }

    fn has_activity(&self) -> bool {
        self.responses
            .iter()
            .any(|message| !message.tools().is_empty())
            || self
                .responses
                .iter()
                .filter(|message| !message.text().trim().is_empty())
                .count()
                > 1
    }

    fn failed(&self) -> bool {
        self.responses.last().is_some_and(|message| {
            matches!(
                message.phase(),
                WorkflowAgentMessagePhase::Failed | WorkflowAgentMessagePhase::Cancelled
            )
        })
    }

    fn activity_label(&self) -> String {
        let tools = self
            .responses
            .iter()
            .flat_map(WorkflowAgentMessage::tools)
            .collect::<Vec<_>>();
        if let Some(running) = tools
            .iter()
            .rev()
            .find(|tool| tool.phase() == WorkflowAgentToolPhase::Running)
        {
            return running.summary().to_owned();
        }
        let outcome = if self.failed() {
            "Needs attention"
        } else {
            "Activity"
        };
        match tools.last() {
            Some(last) => format!("{outcome} · {} actions · {}", tools.len(), last.summary()),
            None => "Activity and earlier findings".to_owned(),
        }
    }
}

#[component]
fn AssistantMarkdown(text: String) -> Element {
    let rendered = use_memo(use_reactive((&text,), |(text,)| {
        AssistantMarkup::from(text.as_str()).0
    }));
    rsx! { div { class: "workflow-agent-message-body", dangerous_inner_html: rendered() } }
}

/// This is the sole HTML boundary: parse CommonMark, then sanitize before rendering.
struct AssistantMarkup(String);

impl From<&str> for AssistantMarkup {
    fn from(text: &str) -> Self {
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        let mut rendered = String::new();
        let events = Parser::new_ext(text, options).flat_map(|event| match event {
            Event::Start(Tag::Table(_)) => vec![
                Event::Html("<details><summary>View table</summary>".into()),
                event,
            ],
            Event::End(TagEnd::Table) => vec![event, Event::Html("</details>".into())],
            _ => vec![event],
        });
        html::push_html(&mut rendered, events);
        // Assistant text must never load remote images or execute supplied markup.
        Self(
            ammonia::Builder::default()
                .add_tags(&["details", "summary"])
                .rm_tags(&["img"])
                .clean(&rendered)
                .to_string(),
        )
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn renders_tables_and_headings_without_active_content() {
        let rendered = AssistantMarkup::from("## Finding\n\n| Image | Score |\n|---|---|\n| dark | 0.02 |\n\n<script>alert(1)</script>\n\n[bad](javascript:alert(1))\n\n![remote](https://example.com/track.png)").0;
        assert!(rendered.contains("<h2>Finding</h2>"));
        assert!(rendered.contains("<table>"));
        assert!(rendered.contains("<td>0.02</td>"));
        assert!(!rendered.contains("<script"));
        assert!(!rendered.contains("javascript:"));
        assert!(!rendered.contains("<img"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn groups_activity_by_request_without_losing_earlier_messages() {
        let message = |id, role, text| WorkflowAgentMessage::new(id, role).with_text(text);
        let turns = AssistantTurn::collect(vec![
            message("u1", WorkflowAgentRole::User, "Find dim images"),
            message("a1", WorkflowAgentRole::Assistant, "Checking luminance"),
            message("a2", WorkflowAgentRole::Assistant, "15 images match"),
            message("u2", WorkflowAgentRole::User, "Explain the criterion"),
            message("a3", WorkflowAgentRole::Assistant, "Below 0.03"),
        ]);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].responses.len(), 2);
        assert_eq!(turns[0].answer().unwrap().id(), "a2");
        assert!(turns[0].has_activity());
        assert_eq!(turns[1].answer().unwrap().id(), "a3");
    }
}
