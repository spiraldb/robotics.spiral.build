//! Browser-owned authoring loop. Tools edit the same draft as the canvas.
mod tools;
use super::{WorkflowClient, state::WorkflowController};
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use serde_json::{Value, json};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{
        WorkflowAgentMessage, WorkflowAgentMessagePhase, WorkflowAgentPhase,
        WorkflowAgentReadiness, WorkflowAgentRole, WorkflowAgentSession, WorkflowAgentToolPhase,
        WorkflowAgentToolSummary,
        conversation::{
            MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION, ProviderMessage, ProviderMessageHistory,
            WorkflowConversation,
        },
        document::ModelRequest,
    },
};
use std::collections::BTreeMap;
use tools::{ToolCompletion, WorkflowToolCatalog};
const MAX_WORKFLOW_AGENT_QUERY_ROWS: usize = 50;
const MAX_CELL_CHARS: usize = 512;
const MAX_QUERY_PROMPT_BYTES: usize = 24 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct WorkflowAssistant {
    conversation: Signal<WorkflowConversation>,
    readiness: Signal<Option<WorkflowAgentReadiness>>,
    task: Signal<Option<Task>>,
}
impl WorkflowAssistant {
    pub(super) fn new(id: &str) -> Self {
        Self {
            conversation: Signal::new(WorkflowConversation::new(id)),
            readiness: Signal::new(None),
            task: Signal::new(None),
        }
    }
    pub(super) fn conversation(&self) -> WorkflowConversation {
        self.conversation.read().clone()
    }
    pub(super) fn snapshot(&self, history_available: bool) -> WorkflowAgentState {
        WorkflowAgentState {
            readiness: self.readiness.read().clone().unwrap_or_default(),
            session: self.conversation.read().session().clone(),
            history_available,
        }
    }
    pub(super) fn loaded(&self) -> bool {
        self.readiness.read().is_some()
    }
    pub(super) async fn connect(mut self) {
        let readiness = WorkflowClient::current()
            .model_status()
            .await
            .unwrap_or_else(|error| WorkflowAgentReadiness::error(error.to_string()));
        self.readiness.set(Some(readiness));
    }
    pub(super) fn install(mut self, mut conversation: WorkflowConversation) {
        self.cancel();
        let _ = conversation.interrupt();
        self.conversation.set(conversation);
    }
    fn update(mut self, change: impl FnOnce(&mut WorkflowConversation)) {
        let mut conversation = self.conversation.write();
        change(&mut conversation);
        // A bounded conversation cannot exhaust the revision counter in practice;
        // retain the existing value if an imported revision is already exhausted.
        if let Err(error) = conversation.advance(js_sys::Date::now() as u64) {
            conversation.session_mut().set_error(Some(error));
        }
    }
    pub(super) fn send(mut self, controller: WorkflowController, text: String) {
        if self.task.peek().is_some() || text.trim().is_empty() {
            return;
        }
        if text.chars().count() > 8_000 {
            self.update(|conversation| {
                conversation
                    .session_mut()
                    .set_error(Some("Keep a message under 8,000 characters.".into()))
            });
            return;
        }
        if self.conversation.peek().session().messages().len()
            >= MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION - 1
        {
            self.update(|conversation| {
                conversation.session_mut().set_error(Some(
                    "This conversation is full. Save it and start a new workflow.".into(),
                ))
            });
            return;
        }
        let id = format!(
            "turn-{}-{}",
            js_sys::Date::now(),
            self.conversation.peek().revision()
        );
        self.update(|conversation| {
            conversation
                .session_mut()
                .set_phase(WorkflowAgentPhase::Responding);
            conversation.session_mut().set_error(None);
            conversation
                .session_mut()
                .set_active_turn_id(Some(id.clone()));
            let sequence = conversation.session().messages().len() as u64;
            conversation.session_mut().messages_mut().push(
                WorkflowAgentMessage::new(format!("{id}-user"), WorkflowAgentRole::User)
                    .with_text(&text)
                    .with_sequence(sequence)
                    .with_created_ms(js_sys::Date::now() as u64)
                    .with_phase(WorkflowAgentMessagePhase::Complete),
            );
            conversation
                .provider_messages_mut()
                .push(ProviderMessage::text("user", text));
        });
        let task = dioxus::dioxus_core::Runtime::current().spawn(
            self.conversation.origin_scope(),
            async move {
                let result = self.run(controller, &id).await;
                self.finish(result.err(), false);
                self.task.set(None);
            },
        );
        self.task.set(Some(task));
    }
    pub(super) fn cancel(mut self) {
        if let Some(task) = self.task.take() {
            task.cancel();
            self.finish(None, true);
        }
    }
    fn finish(self, error: Option<String>, cancelled: bool) {
        self.update(|conversation| {
            conversation
                .provider_messages_mut()
                .remove_incomplete_tool_exchange();
            conversation
                .provider_messages_mut()
                .trim_to_provider_bound();
            let phase = if cancelled {
                WorkflowAgentPhase::Cancelled
            } else if error.is_some() {
                WorkflowAgentPhase::Failed
            } else {
                WorkflowAgentPhase::Idle
            };
            conversation.session_mut().set_phase(phase);
            conversation.session_mut().set_error(error);
            conversation.session_mut().set_active_turn_id(None);
            for message in conversation.session_mut().messages_mut() {
                for tool in message.tools_mut() {
                    if tool.phase() == WorkflowAgentToolPhase::Running {
                        tool.set_phase(WorkflowAgentToolPhase::Failed);
                        tool.set_error(Some("The turn ended before this tool completed.".into()));
                    }
                }
                if message.phase() == WorkflowAgentMessagePhase::Streaming {
                    message.set_phase(if cancelled {
                        WorkflowAgentMessagePhase::Cancelled
                    } else if phase == WorkflowAgentPhase::Failed {
                        WorkflowAgentMessagePhase::Failed
                    } else {
                        WorkflowAgentMessagePhase::Complete
                    });
                }
            }
        });
    }
    async fn run(self, controller: WorkflowController, turn: &str) -> Result<(), String> {
        let mut expected = controller.spec();
        let client = WorkflowClient::current();
        let mut calls = 0;
        for iteration in 0..16 {
            if controller.spec() != expected {
                return Err(
                    "The graph changed during this turn. Retry with the current draft.".into(),
                );
            }
            if self.conversation.peek().session().messages().len()
                >= MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION
            {
                return Err("This conversation reached its message limit. Save it before starting another workflow.".into());
            }
            self.update(|conversation| {
                conversation
                    .provider_messages_mut()
                    .trim_to_provider_bound();
                let sequence = conversation.session().messages().len() as u64;
                conversation.session_mut().messages_mut().push(
                    WorkflowAgentMessage::new(
                        format!("{turn}-{iteration}"),
                        WorkflowAgentRole::Assistant,
                    )
                    .with_sequence(sequence)
                    .with_created_ms(js_sys::Date::now() as u64),
                );
            });
            let mut messages = vec![ProviderMessage::text(
                "system",
                format!(
                    "{SYSTEM_PROMPT}\nCurrent workflow: {}",
                    serde_json::to_string(&expected).map_err(|e| e.to_string())?
                ),
            )];
            messages.extend(self.conversation.peek().provider_messages().clone());
            let response = client
                .complete(
                    ModelRequest::new(messages, WorkflowToolCatalog::for_workflow(&expected)),
                    |delta| {
                        self.update(|conversation| {
                            if let Some(message) =
                                conversation.session_mut().messages_mut().last_mut()
                            {
                                message.set_text(format!("{}{delta}", message.text()));
                            }
                        });
                    },
                )
                .await?;
            self.update(|conversation| {
                conversation.provider_messages_mut().push(response.clone());
                if let Some(message) = conversation.session_mut().messages_mut().last_mut() {
                    message.set_text(response.content().unwrap_or_default());
                    message.set_phase(WorkflowAgentMessagePhase::Complete);
                }
            });
            if response.tool_calls().is_empty() {
                return Ok(());
            }
            for call in response.tool_calls() {
                calls += 1;
                if calls > 48 {
                    return Err(
                        "This turn reached its tool-call limit. Continue with another message."
                            .into(),
                    );
                }
                if controller.spec() != expected {
                    return Err(
                        "The graph changed during this turn. Retry with the current draft.".into(),
                    );
                }
                self.update(|conversation| {
                    if let Some(message) = conversation.session_mut().messages_mut().last_mut() {
                        message.tools_mut().push(WorkflowAgentToolSummary::new(
                            call.id(),
                            call.function().name(),
                            WorkflowToolCatalog::presentation(call.function().name())
                                .running_summary(),
                        ));
                    }
                });
                let result = WorkflowToolCatalog::execute(controller, call, &mut expected).await;
                let ok = result.is_ok();
                let output = ToolOutput::bounded_text(
                    result.unwrap_or_else(|error| json!({"ok":false,"error":error}).to_string()),
                );
                let ok = ok
                    && serde_json::from_str::<Value>(&output)
                        .ok()
                        .is_none_or(|value| {
                            value.get("ok").and_then(Value::as_bool) != Some(false)
                        });
                let completion = ToolCompletion::for_tool(call.function().name(), ok, &output);
                let detail = (!ok).then(|| ToolPublicText::detail(&output)).flatten();
                self.update(|conversation| {
                    conversation
                        .provider_messages_mut()
                        .push(ProviderMessage::tool(call.id().to_owned(), output));
                    if let Some(tool) = conversation
                        .session_mut()
                        .messages_mut()
                        .last_mut()
                        .and_then(|message| message.tools_mut().last_mut())
                    {
                        tool.set_phase(if ok {
                            WorkflowAgentToolPhase::Complete
                        } else {
                            WorkflowAgentToolPhase::Failed
                        });
                        tool.set_summary(completion.summary);
                        tool.set_receipt(completion.receipt);
                        tool.set_error(detail);
                    }
                });
            }
        }
        Err("This turn reached its step limit. Continue with another message.".into())
    }
}
const SYSTEM_PROMPT: &str = r#"You are the Workflow Assistant in Spiral Robotics Studio. Author a single executable Arrow DAG in memory, then let the user choose Run. Only the user's Save button persists the graph and conversation. Use inspect_dataset with {} or {"path":[]} to list datasets, then with an actual returned selection.path to inspect topics and camera schema variants. Also inspect relations and model capabilities as needed. Never invent dataset paths, topics, columns, or functions. Keep the workflow's identity and existing intent when editing it.

A Query with no parents is a SQL source. A Query with parents uses {{node:id}} for each connected input, including joins. A Model has one parent, an image_column, and features with name, description, and concept. It appends nullable Float32 SigLIP scores to all input columns. Result consumes one output. There is no preparation phase or feature-cache relation. Every Run executes inference anew; cross-workflow reuse is a future milestone. No fixed episode/frame key schema is required. Keep useful source identifiers as ordinary columns when the question needs grouping or joins. Schema validation runs before execution. Query max_rows limits the displayed preview, not inference; select the intended cohort and frame sampling in SQL.

For MCAP cameras inspect the actual foxglove.CompressedVideo schema variant. Read packet payloads with spql.mcap.read_topic('actual_dataset_path', 'actual_topic', 'actual_variant'); all three arguments are inspected string literals, not per-row source blobs. Its id column is the native member identity; use spql.mcap.episodes('actual_dataset_path') for the episode population and keep those members even when camera evidence is absent. value.data is the encoded payload. Group packets for each member and camera separately. Build a video source with spql.video.from_packets(spql.list.collect(struct_pack(payload := value.data, decode_index := <unique Int64 packet order>)), 'h264', 'annex_b'). Never mix member streams or codecs. Expand selected frames with CROSS JOIN LATERAL spql.video.frames(s.source, spql.video.frame_indices([...])) f, then decode f.frame with spql.video.decode(f.frame); f is a relation alias, not a frame value. Inspect source metadata to determine ordering and sampling; MCAP log_time is not automatically presentation time. Decode after frame selection. Use ordinary SQL to aggregate scores per episode or join other evidence. Null scores are unknown evidence: handle them explicitly rather than treating them as false.

SigLIP scores measure visual text/image matching, not measured brightness or calibrated probability. Explain provisional scoring concepts and thresholds. Inspect only bounded data through run_query. Do not include images, tensors, binary payloads, or credentials in the conversation. Do not execute inference through tools. Save with set_workflow and concisely explain the graph and next Run action."#;

trait SpiralQuerySnapshotAgentExt {
    fn serialize_for_agent_prompt(self) -> String;
}

impl SpiralQuerySnapshotAgentExt for SpiralQuerySnapshot {
    fn serialize_for_agent_prompt(self) -> String {
        let row_count = self.rows().len();
        let source_truncated = self.truncated();
        let elapsed_ms = self.elapsed_ms();
        let semantic = self
            .columns()
            .iter()
            .map(|column| {
                (
                    column.name().to_owned(),
                    column.semantic_type().map(str::to_owned),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut rows = Vec::new();
        let mut used = 0_usize;
        for row in self.rows().iter().take(MAX_WORKFLOW_AGENT_QUERY_ROWS) {
            let mut safe = serde_json::Map::new();
            for (name, value) in row {
                let converted = value.clone().safe_for_agent_prompt(
                    semantic
                        .get(name.as_str())
                        .and_then(|value| value.as_deref()),
                );
                let encoded = converted.to_string();
                if used.saturating_add(encoded.len()) > MAX_QUERY_PROMPT_BYTES {
                    safe.insert(name.clone(), json!({"type":"truncated"}));
                    break;
                }
                used = used.saturating_add(encoded.len());
                safe.insert(name.clone(), converted);
            }
            rows.push(Value::Object(safe));
            if used >= MAX_QUERY_PROMPT_BYTES {
                break;
            }
        }
        let mut columns = self.columns().to_vec();
        let mut truncated = source_truncated || used >= MAX_QUERY_PROMPT_BYTES;
        loop {
            let encoded = json!({
                "columns": &columns,
                "rows": &rows,
                "row_count": row_count,
                "truncated": truncated,
                "elapsed_ms": elapsed_ms,
            })
            .to_string();
            if encoded.len() <= MAX_QUERY_PROMPT_BYTES {
                return encoded;
            }
            truncated = true;
            if rows.pop().is_some() {
                continue;
            }
            if columns.pop().is_some() {
                continue;
            }
            return json!({
                "row_count": row_count,
                "truncated": true,
                "error": "Query metadata exceeded the bounded prompt payload limit."
            })
            .to_string();
        }
    }
}

trait AgentPromptCell {
    fn safe_for_agent_prompt(self, semantic_type: Option<&str>) -> Value;
}

impl AgentPromptCell for Value {
    fn safe_for_agent_prompt(self, semantic_type: Option<&str>) -> Value {
        if let Some(semantic_type) = semantic_type {
            return json!({"type":"media_or_tensor","semantic_type":semantic_type});
        }
        match self {
            Value::String(value) if value.starts_with("data:") => json!({"type":"media"}),
            Value::String(value) => Value::String(value.chars().take(MAX_CELL_CHARS).collect()),
            Value::Array(value) if value.len() > 32 => json!({"type":"array","length":value.len()}),
            Value::Object(value) if value.len() > 32 => {
                json!({"type":"object","fields":value.len()})
            }
            value => value,
        }
    }
}

struct ToolPublicText;

impl ToolPublicText {
    fn label(value: &str) -> String {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect()
    }

    fn error(error: &str) -> String {
        error
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(800)
            .collect()
    }

    fn detail(detail: &str) -> Option<String> {
        let value = serde_json::from_str::<Value>(detail).unwrap_or(Value::Null);
        if let Some(error) = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.trim().is_empty())
        {
            return Some(Self::error(error));
        }
        if let Some(issue) = value
            .get("validation")
            .and_then(|validation| validation.get("errors"))
            .and_then(Value::as_array)
            .and_then(|errors| errors.first())
        {
            let message = issue
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The graph is not valid.");
            let recovery = issue
                .get("recovery")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Some(Self::error(&format!("{message} {recovery}")));
        }
        match value.get("phase").and_then(Value::as_str) {
            Some("cancelled" | "interrupted") => None,
            Some("failed") => Some("Workflow run failed without a public error detail.".to_owned()),
            _ if value.is_null() => Some(Self::error(detail)),
            _ => Some("The tool failed without a public error detail.".to_owned()),
        }
    }
}

struct ToolOutput;

impl ToolOutput {
    fn bounded_text(output: String) -> String {
        if output.len() <= MAX_TOOL_OUTPUT_BYTES {
            output
        } else {
            json!({
                "ok": false,
                "error": "Tool output exceeded the bounded prompt payload limit.",
                "truncated": true,
            })
            .to_string()
        }
    }
}
pub(crate) struct WorkflowAgentState {
    readiness: WorkflowAgentReadiness,
    session: WorkflowAgentSession,
    history_available: bool,
}
impl WorkflowAgentState {
    pub(crate) fn readiness(&self) -> &WorkflowAgentReadiness {
        &self.readiness
    }
    pub(crate) fn session(&self) -> &WorkflowAgentSession {
        &self.session
    }
    pub(crate) fn history_available(&self) -> bool {
        self.history_available
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::workflow::WorkflowSpec;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn media_and_long_cells_never_enter_prompt_payloads() {
        assert_eq!(
            Value::String("data:image/png;base64,secret".to_owned()).safe_for_agent_prompt(None),
            json!({"type":"media"})
        );
        assert_eq!(
            json!([1, 2, 3]).safe_for_agent_prompt(Some("spql.image.Image")),
            json!({"type":"media_or_tensor","semantic_type":"spql.image.Image"})
        );
        let long = "x".repeat(MAX_CELL_CHARS + 10);
        assert_eq!(
            Value::String(long)
                .safe_for_agent_prompt(None)
                .as_str()
                .unwrap()
                .len(),
            MAX_CELL_CHARS
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn serialized_query_prompt_is_hard_bounded_including_schema_and_json_overhead() {
        let columns = (0..40)
            .map(|index| {
                spiral_robotics_common::relations::QueryColumn::new(
                    format!("field_{index}_{}", "x".repeat(900)),
                    format!("Struct<{}>", "y".repeat(900)),
                    true,
                )
            })
            .collect::<Vec<_>>();
        let row = columns
            .iter()
            .map(|column| (column.name().to_owned(), Value::String("value".repeat(100))))
            .collect();
        let encoded = SpiralQuerySnapshot::new(columns, vec![row], false)
            .with_elapsed_ms(1)
            .serialize_for_agent_prompt();
        assert!(encoded.len() <= MAX_QUERY_PROMPT_BYTES);
        let payload = serde_json::from_str::<Value>(&encoded).unwrap();
        assert_eq!(payload["row_count"], 1);
        assert_eq!(payload["truncated"], true);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn tool_schema_exposes_only_authoring_and_inspection() {
        let definitions =
            WorkflowToolCatalog::for_workflow(&WorkflowSpec::new("schema-test", "Schema test"));
        let names = definitions
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "inspect_dataset",
                "list_relations",
                "describe_relations",
                "list_model_capabilities",
                "set_workflow",
                "run_query",
            ]
        );
        assert_eq!(
            definitions[5]["function"]["parameters"]["properties"]["max_rows"]["maximum"],
            MAX_WORKFLOW_AGENT_QUERY_ROWS
        );
        let workflow = &definitions[4]["function"]["parameters"]["properties"]["workflow"];
        assert!(workflow["properties"].get("id").is_none());
        assert!(workflow["properties"].get("revision").is_none());
        assert_eq!(workflow["required"], json!(["name", "nodes", "edges"]));
        let variants = workflow["properties"]["nodes"]["items"]["oneOf"]
            .as_array()
            .expect("set_workflow nodes have discriminated variants");
        assert_eq!(variants.len(), 4);
        assert_eq!(variants[0]["properties"]["kind"]["const"], "table");
        assert_eq!(variants[1]["properties"]["kind"]["const"], "model");
        assert_eq!(variants[1]["properties"]["image_column"]["type"], "string");
        assert_eq!(variants[2]["properties"]["kind"]["const"], "query");
        assert!(variants[1]["properties"].get("sample_every_ms").is_none());
        assert!(variants[1]["properties"].get("media_column").is_none());
        assert!(variants[1]["properties"].get("concepts").is_none());
        let feature = &variants[1]["properties"]["features"]["items"];
        assert_eq!(
            feature["properties"]["name"]["pattern"],
            "^[a-z_][a-z0-9_]*$"
        );
        assert!(feature["properties"].get("applicability").is_none());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn public_tool_receipts_are_specific_bounded_and_secret_free() {
        let saved = ToolCompletion::for_tool(
            "set_workflow",
            true,
            &json!({
                "ok": true,
                "workflow": {"name": "Holding evidence", "revision": 7},
                "provider_secret": "must-not-leak"
            })
            .to_string(),
        );
        assert_eq!(saved.summary, "Updated Holding evidence · r7");
        let receipt = saved.receipt.expect("saved graph receipt");
        assert_eq!(receipt.revision(), 7);
        assert!(!serde_json::to_string(&receipt).unwrap().contains("secret"));

        assert_eq!(
            ToolCompletion::for_tool(
                "list_relations",
                true,
                &json!({"relations": ["a.b", "c.d"]}).to_string()
            )
            .summary,
            "Found 2 Spiral relations"
        );
        assert_eq!(
            ToolCompletion::for_tool(
                "run_query",
                true,
                &json!({"row_count": 12, "rows": [1], "truncated": true}).to_string()
            )
            .summary,
            "Inspected 12 rows · bounded preview"
        );
    }
}

#[cfg(test)]
mod browser_tests;
