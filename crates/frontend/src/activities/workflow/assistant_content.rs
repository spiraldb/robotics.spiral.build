//! One readable answer per user turn, with the complete activity retained on demand.

use dioxus::prelude::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use spiral_robotics_common::workflow::{
    WorkflowAgentMessage, WorkflowAgentMessagePhase, WorkflowAgentRole, WorkflowAgentToolPhase,
};

use super::highlight::Tokens;
use super::state::WorkflowController;
use crate::components::icons::ConversationIcon;
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
                            span { class: "workflow-agent-role", "You" }
                            AssistantMarkdown { text: request.text().to_owned() }
                        }
                    }
                    if turn.has_activity() {
                        details { class: "workflow-agent-tools-disclosure",
                            "data-failed": turn.failed(),
                            "data-running": turn.running(),
                            summary {
                                span { class: "workflow-activity-dot", "aria-hidden": "true" }
                                "{turn.activity_label()}"
                            }
                            div { class: "workflow-turn-activity",
                                for message in &turn.responses {
                                    if !message.text().is_empty() && Some(message.id()) != turn.answer().map(WorkflowAgentMessage::id) {
                                        div { class: "workflow-agent-finding", role: "group", "aria-label": if message.role() == WorkflowAgentRole::Synthetic { "Workflow notice" } else { "Assistant finding" },
                                            span { class: "workflow-agent-role", if message.role() == WorkflowAgentRole::Synthetic { "Studio" } else { "Assistant" } }
                                            AssistantMarkdown { text: message.text().to_owned() }
                                        }
                                    }
                                    for tool in message.tools() {
                                        div { class: "workflow-agent-tool-detail {tool.phase().class_name()}",
                                            span { class: "workflow-tool-step-dot", "aria-hidden": "true" }
                                            div { class: "workflow-tool-step-copy",
                                                strong { "{tool.summary()}" }
                                                if let Some(error) = tool.error() { p { class: "workflow-tool-error", "{error}" } }
                                                if let Some(receipt) = tool.receipt() {
                                                    if controller.receipt_panel(receipt).is_some() {
                                                        {let target = receipt.to_owned();
                                                        rsx! { Action {
                                                            purpose: ActionPurpose::Navigate, variant: ActionVariant::Link,
                                                            class: "workflow-agent-receipt-action",
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
                    }
                    if let Some(answer) = turn.answer() {
                        div { class: "workflow-agent-message is-assistant",
                            class: if answer.role() == WorkflowAgentRole::Synthetic { "is-synthetic" },
                            role: "group", "aria-label": if answer.role() == WorkflowAgentRole::Synthetic { "Workflow notice" } else { "Assistant response" },
                            span { class: "workflow-agent-avatar", "aria-hidden": "true", ConversationIcon {} }
                            div { class: "workflow-agent-answer",
                                span { class: "workflow-agent-role", if answer.role() == WorkflowAgentRole::Synthetic { "Studio" } else { "Assistant" } }
                                AssistantMarkdown { text: answer.text().to_owned() }
                            }
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

    fn running(&self) -> bool {
        self.responses.iter().any(|message| {
            message
                .tools()
                .iter()
                .any(|tool| tool.phase() == WorkflowAgentToolPhase::Running)
        }) || self
            .responses
            .last()
            .is_some_and(|message| message.phase() == WorkflowAgentMessagePhase::Streaming)
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

trait ToolPhasePresentation {
    fn class_name(self) -> &'static str;
}

impl ToolPhasePresentation for WorkflowAgentToolPhase {
    fn class_name(self) -> &'static str {
        match self {
            Self::Running => "is-running",
            Self::Complete => "is-complete",
            Self::Failed => "is-failed",
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
        let mut sql_block: Option<String> = None;
        let mut events = Vec::new();
        for event in Parser::new_ext(text, options) {
            match event {
                Event::Start(Tag::Table(_)) => {
                    events.push(Event::Html("<details><summary>View table</summary>".into()));
                    events.push(event);
                }
                Event::End(TagEnd::Table) => {
                    events.push(event);
                    events.push(Event::Html("</details>".into()));
                }
                // SQL fences are coloured by the same lexer as the editor. The
                // text is escaped by the lexer and the result is still sanitized.
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language)))
                    if Self::is_sql(&language) =>
                {
                    sql_block = Some(String::new());
                }
                Event::Text(text) if sql_block.is_some() => {
                    if let Some(block) = sql_block.as_mut() {
                        block.push_str(&text);
                    }
                }
                Event::End(TagEnd::CodeBlock) if sql_block.is_some() => {
                    if let Some(block) = sql_block.take() {
                        events.push(Event::Html(
                            format!(
                                "<pre><code class=\"language-sql\">{}</code></pre>",
                                Tokens::sql(block.trim_end_matches('\n')).html()
                            )
                            .into(),
                        ));
                    }
                }
                _ => events.push(event),
            }
        }
        html::push_html(&mut rendered, events.into_iter());
        // Assistant text must never load remote images or execute supplied markup.
        Self(
            ammonia::Builder::default()
                .add_tags(&["details", "summary"])
                .add_allowed_classes(
                    "span",
                    &[
                        "tok-keyword",
                        "tok-function",
                        "tok-field",
                        "tok-literal",
                        "tok-operator",
                        "tok-punct",
                        "tok-comment",
                    ],
                )
                .add_allowed_classes("code", &["language-sql"])
                .rm_tags(&["img"])
                .clean(&rendered)
                .to_string(),
        )
    }
}

impl AssistantMarkup {
    fn is_sql(language: &str) -> bool {
        matches!(
            language.trim().to_ascii_lowercase().as_str(),
            "sql" | "spql" | "postgresql" | "mysql"
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
    fn sql_fences_are_coloured_and_still_sanitized() {
        let rendered =
            AssistantMarkup::from("```sql\nSELECT id FROM t WHERE x < 1 -- <script>\n```").0;
        assert!(rendered.contains("<code class=\"language-sql\">"));
        assert!(rendered.contains("<span class=\"tok-keyword\">SELECT</span>"));
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(!rendered.contains("<script>"));
        let plain = AssistantMarkup::from("```python\nprint(1)\n```").0;
        assert!(plain.contains("<code") && plain.contains("print(1)"));
        assert!(!plain.contains("tok-"));
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
