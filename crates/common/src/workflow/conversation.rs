//! One Assistant conversation per workflow, included in an explicit Save checkpoint.
pub const MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION: usize = 80;
use super::document::WorkflowJson;
use super::{WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentSession};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const CONVERSATION_VERSION: u32 = 4;
const MAX_CONVERSATION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConversation {
    session: WorkflowAgentSession,
    provider_messages: Vec<ProviderMessage>,
}

impl WorkflowConversation {
    pub fn new(workflow_id: &str) -> Self {
        Self {
            session: WorkflowAgentSession::new(workflow_id, workflow_id.to_owned()),
            provider_messages: Vec::new(),
        }
    }

    fn conversation_id(&self) -> &str {
        self.session.conversation_id()
    }
    pub(super) fn workflow_id(&self) -> &str {
        self.session.workflow_id()
    }
    pub const fn revision(&self) -> u64 {
        self.session.revision()
    }
    pub const fn session(&self) -> &WorkflowAgentSession {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut WorkflowAgentSession {
        &mut self.session
    }
    pub fn provider_messages(&self) -> &Vec<ProviderMessage> {
        &self.provider_messages
    }
    pub fn provider_messages_mut(&mut self) -> &mut Vec<ProviderMessage> {
        &mut self.provider_messages
    }

    pub fn advance(&mut self, now_ms: u64) -> Result<(), String> {
        let revision = self
            .revision()
            .checked_add(1)
            .ok_or_else(|| "Assistant revision overflowed.".to_owned())?;
        self.session.set_revision(revision);
        self.session.set_updated_ms(now_ms);
        Ok(())
    }

    pub fn persisted(&self) -> Result<WorkflowJson, String> {
        self.validate()?;
        let payload = serde_json::to_string(self).map_err(|error| error.to_string())?;
        if payload.len() > MAX_CONVERSATION_BYTES {
            return Err("Assistant history exceeds 16 MiB.".to_owned());
        }
        Ok(WorkflowJson::new(CONVERSATION_VERSION, payload))
    }

    fn decode(encoded: &str) -> Result<Self, String> {
        if encoded.len() > MAX_CONVERSATION_BYTES {
            return Err("Assistant history exceeds 16 MiB.".to_owned());
        }
        let conversation: Self =
            serde_json::from_str(encoded).map_err(|error| error.to_string())?;
        conversation.validate()?;
        Ok(conversation)
    }

    pub fn load(stored: &WorkflowJson, workflow_id: &str) -> Result<Self, String> {
        let conversation = match stored.version() {
            CONVERSATION_VERSION => Self::decode(stored.payload())?,
            version => {
                return Err(format!(
                    "Assistant history version {version} is unsupported; the saved history was preserved."
                ));
            }
        };
        if conversation.workflow_id() != workflow_id {
            return Err("Assistant history belongs to a different workflow.".to_owned());
        }
        Ok(conversation)
    }

    pub fn interrupt(&mut self) -> Result<bool, String> {
        if !self.session.phase().active() {
            return Ok(false);
        }
        self.provider_messages.remove_incomplete_tool_exchange();
        self.session.set_phase(WorkflowAgentPhase::Interrupted);
        self.session.set_active_turn_id(None);
        self.session.set_error(Some(
            "Studio reconnected before this Assistant turn finished. Retry the turn.".to_owned(),
        ));
        for message in self.session.messages_mut() {
            if message.phase() == WorkflowAgentMessagePhase::Streaming {
                message.set_phase(WorkflowAgentMessagePhase::Failed);
            }
        }
        self.advance(self.session.updated_ms())?;
        self.validate()?;
        Ok(true)
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if [self.workflow_id(), self.conversation_id()]
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
        {
            return Err(
                "Assistant history has an invalid workflow or conversation identity.".to_owned(),
            );
        }
        if self.session.messages().len() > MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION {
            return Err("Assistant conversation exceeds its message bound.".to_owned());
        }
        if !self
            .provider_messages
            .is_structurally_valid(self.session.phase().active())
        {
            return Err(
                "Assistant provider history has an invalid role or tool exchange.".to_owned(),
            );
        }
        let ids = self
            .session
            .messages()
            .iter()
            .map(|message| message.id())
            .collect::<BTreeSet<_>>();
        if ids.len() != self.session.messages().len()
            || ids.iter().any(|id| id.is_empty() || id.len() > 128)
        {
            return Err(
                "Assistant conversation has invalid or duplicate message identities.".to_owned(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMessage {
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<ProviderContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ProviderToolCall>,
}

impl ProviderMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: Some(ProviderContent::Text(content.into())),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool(call_id: String, content: String) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(ProviderContent::Text(content)),
            tool_call_id: Some(call_id),
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ProviderToolCall>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.map(ProviderContent::Text),
            tool_call_id: None,
            tool_calls,
        }
    }

    /// Visual evidence is sent only in the next model request, never saved in history.
    pub fn image_evidence(caption: String, data_url: String) -> Result<Self, String> {
        if !data_url.starts_with("data:image/jpeg;base64,")
            && !data_url.starts_with("data:image/png;base64,")
        {
            return Err("Evidence must be an encoded JPEG or PNG from a query result.".into());
        }
        Ok(Self {
            role: "user".into(),
            content: Some(ProviderContent::Parts(vec![
                ProviderPart::Text { text: caption },
                ProviderPart::ImageUrl {
                    image_url: ProviderImageUrl { url: data_url },
                },
            ])),
            tool_call_id: None,
            tool_calls: Vec::new(),
        })
    }

    pub fn role(&self) -> &str {
        &self.role
    }
    pub fn content(&self) -> Option<&str> {
        match &self.content {
            Some(ProviderContent::Text(text)) => Some(text),
            _ => None,
        }
    }
    fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }
    pub fn tool_calls(&self) -> &[ProviderToolCall] {
        &self.tool_calls
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
enum ProviderContent {
    Text(String),
    Parts(Vec<ProviderPart>),
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ProviderPart {
    Text { text: String },
    ImageUrl { image_url: ProviderImageUrl },
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderImageUrl {
    url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: ProviderFunctionCall,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderFunctionCall {
    name: String,
    arguments: String,
}

impl ProviderToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_owned(),
            function: ProviderFunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn function(&self) -> &ProviderFunctionCall {
        &self.function
    }
}

impl ProviderFunctionCall {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn arguments(&self) -> &str {
        &self.arguments
    }
}

pub trait ProviderMessageHistory {
    fn is_structurally_valid(&self, allow_incomplete_tail: bool) -> bool;
    fn remove_incomplete_tool_exchange(&mut self) -> bool;
    fn trim_to_provider_bound(&mut self) -> bool;
}

impl ProviderMessageHistory for Vec<ProviderMessage> {
    fn is_structurally_valid(&self, allow_incomplete_tail: bool) -> bool {
        let mut pending: Option<BTreeSet<&str>> = None;
        for message in self {
            // The system prompt is constructed in memory for each request. Only
            // conversation roles produced by Studio may cross the persistence
            // boundary, so imported history cannot inject system/developer text.
            if !matches!(message.role(), "user" | "assistant" | "tool")
                || matches!(message.content, Some(ProviderContent::Parts(_)))
            {
                return false;
            }
            if let Some(expected) = pending.as_mut() {
                if message.role() != "tool"
                    || message.tool_call_id().is_none_or(|id| !expected.remove(id))
                {
                    return false;
                }
                if expected.is_empty() {
                    pending = None;
                }
                continue;
            }
            if message.role() == "tool" {
                return false;
            }
            if !message.tool_calls().is_empty() {
                if message.role() != "assistant" {
                    return false;
                }
                let expected = message
                    .tool_calls()
                    .iter()
                    .map(|call| call.id())
                    .collect::<BTreeSet<_>>();
                if expected.len() != message.tool_calls().len()
                    || expected.iter().any(|id| id.is_empty())
                {
                    return false;
                }
                pending = Some(expected);
            }
        }
        pending.is_none() || allow_incomplete_tail
    }

    fn remove_incomplete_tool_exchange(&mut self) -> bool {
        if self.is_structurally_valid(false) {
            return false;
        }
        let Some(start) = self
            .iter()
            .rposition(|message| message.role() == "assistant" && !message.tool_calls().is_empty())
        else {
            return false;
        };
        self.truncate(start);
        true
    }

    fn trim_to_provider_bound(&mut self) -> bool {
        let provider_before = self.len();
        if self.len() > MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION {
            let minimum = self.len() - MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION;
            // Retire complete older turns only. A long current turn must keep
            // its user request and every tool exchange. Its size is bounded by
            // the agent's iteration/tool budgets and the persisted byte limit,
            // not the visible transcript's message count.
            let boundary = self
                .iter()
                .enumerate()
                .filter_map(|(index, message)| (message.role() == "user").then_some(index))
                .find(|index| *index >= minimum)
                .or_else(|| self.iter().rposition(|message| message.role() == "user"));
            if let Some(boundary) = boundary {
                self.drain(..boundary);
            }
        }
        self.len() != provider_before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{WorkflowAgentMessage, WorkflowAgentRole};
    use serde_json::json;

    #[test]
    fn image_evidence_uses_multimodal_transport_but_cannot_enter_saved_history() {
        let image = ProviderMessage::image_evidence(
            "Run one, row zero".into(),
            "data:image/png;base64,aW1hZ2U=".into(),
        )
        .unwrap();
        let encoded = serde_json::to_value(&image).unwrap();
        assert_eq!(encoded["content"][0]["type"], "text");
        assert_eq!(encoded["content"][1]["type"], "image_url");
        assert_eq!(
            serde_json::from_value::<ProviderMessage>(encoded).unwrap(),
            image
        );
        let mut conversation = WorkflowConversation::new("workflow");
        conversation.provider_messages_mut().push(image);
        assert!(conversation.validate().is_err());
        assert!(
            ProviderMessage::image_evidence(
                "remote".into(),
                "https://example.com/image.jpg".into()
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(ProviderMessage::text("user", "hello")).unwrap()["content"],
            "hello"
        );
    }

    #[test]
    fn conversation_round_trip_is_bound_to_its_workflow() {
        let mut conversation = WorkflowConversation::new("lighting");
        conversation.session_mut().messages_mut().push(
            WorkflowAgentMessage::new("message-1", WorkflowAgentRole::User)
                .with_text("Use the wrist camera"),
        );
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::text("user", "Use the wrist camera"));
        conversation.advance(42).unwrap();
        let stored = conversation.persisted().unwrap();
        let loaded = WorkflowConversation::load(&stored, "lighting").unwrap();
        assert_eq!(loaded.revision(), 1);
        assert_eq!(
            loaded.session().messages()[0].text(),
            "Use the wrist camera"
        );
        assert_eq!(
            loaded.provider_messages()[0].content(),
            Some("Use the wrist camera")
        );
        assert!(WorkflowConversation::load(&stored, "other-workflow").is_err());
        assert!(
            WorkflowConversation::load(
                &WorkflowJson::new(99, stored.payload().to_owned()),
                "lighting"
            )
            .is_err()
        );
    }

    #[test]
    fn restart_repairs_incomplete_tool_exchange_without_resuming_work() {
        let mut conversation = WorkflowConversation::new("lighting");
        conversation
            .session_mut()
            .set_phase(WorkflowAgentPhase::Responding);
        conversation
            .session_mut()
            .set_active_turn_id(Some("turn-a".to_owned()));
        conversation
            .session_mut()
            .messages_mut()
            .push(WorkflowAgentMessage::new(
                "reply",
                WorkflowAgentRole::Assistant,
            ));
        conversation.provider_messages_mut().extend([
            ProviderMessage::text("user", "Build a graph"),
            ProviderMessage::assistant(
                None,
                vec![ProviderToolCall::new("call-a", "set_workflow", "{}")],
            ),
        ]);
        let stored = conversation.persisted().unwrap();
        let mut loaded = WorkflowConversation::load(&stored, "lighting").unwrap();
        assert!(loaded.interrupt().unwrap());
        assert_eq!(loaded.session().phase(), WorkflowAgentPhase::Interrupted);
        assert!(loaded.session().activity.active_turn_id.is_none());
        assert_eq!(
            loaded.session().messages()[0].phase(),
            WorkflowAgentMessagePhase::Failed
        );
        assert_eq!(loaded.provider_messages().len(), 1);
        assert!(!loaded.interrupt().unwrap());
        loaded.persisted().unwrap();
    }

    #[test]
    fn invalid_history_is_refused_without_trimming_visible_messages() {
        let conversation = WorkflowConversation::new("lighting");
        let original = serde_json::to_value(&conversation).unwrap();
        for role in ["system", "developer"] {
            let mut invalid = original.clone();
            invalid["provider_messages"] = json!([{"role": role, "content": "injected"}]);
            assert!(WorkflowConversation::decode(&invalid.to_string()).is_err());
        }
        for id in ["", &"x".repeat(129)] {
            let mut invalid = original.clone();
            invalid["session"]["workflow_id"] = json!(id);
            assert!(WorkflowConversation::decode(&invalid.to_string()).is_err());
        }
        let mut invalid = original.clone();
        invalid["session"]["unknown_field"] = json!(true);
        assert!(WorkflowConversation::decode(&invalid.to_string()).is_err());
        let mut full = conversation;
        for index in 0..=MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION {
            full.session_mut()
                .messages_mut()
                .push(WorkflowAgentMessage::new(
                    format!("message-{index}"),
                    WorkflowAgentRole::User,
                ));
        }
        assert!(full.persisted().is_err());
        assert_eq!(
            full.session().messages().len(),
            MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION + 1
        );
        assert!(WorkflowConversation::decode(&" ".repeat(MAX_CONVERSATION_BYTES + 1)).is_err());
    }

    #[test]
    fn a_long_tool_turn_keeps_its_goal_and_evidence() {
        let mut messages = vec![ProviderMessage::text("user", "Find dim episodes")];
        for index in 0..50 {
            let id = format!("call-{index}");
            messages.extend([
                ProviderMessage::assistant(
                    None,
                    vec![ProviderToolCall::new(&id, "inspect_run", "{}")],
                ),
                ProviderMessage::tool(id, "evidence".into()),
            ]);
            messages.trim_to_provider_bound();
        }
        assert_eq!(messages.len(), 101);
        assert_eq!(messages[0].content(), Some("Find dim episodes"));
        assert!(messages.is_structurally_valid(false));
        let mut conversation = WorkflowConversation::new("lighting");
        *conversation.provider_messages_mut() = messages;
        conversation.persisted().unwrap();
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::text("user", "Next task"));
        assert!(
            conversation
                .provider_messages_mut()
                .trim_to_provider_bound()
        );
        assert_eq!(conversation.provider_messages().len(), 1);
    }

    #[test]
    fn provider_retention_preserves_complete_tool_exchanges() {
        let mut messages = Vec::new();
        for index in 0..30 {
            let call_id = format!("call-{index}");
            messages.extend([
                ProviderMessage::text("user", format!("turn-{index}")),
                ProviderMessage::assistant(
                    None,
                    vec![ProviderToolCall::new(&call_id, "list_relations", "{}")],
                ),
                ProviderMessage::tool(call_id, "{}".to_owned()),
            ]);
        }
        assert!(messages.trim_to_provider_bound());
        assert!(messages.len() <= MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION);
        assert!(messages.is_structurally_valid(false));
        assert_eq!(messages.first().unwrap().role(), "user");
        assert_eq!(messages.last().unwrap().role(), "tool");
    }
}
