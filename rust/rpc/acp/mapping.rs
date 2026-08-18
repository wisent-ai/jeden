use crate::sdk::SessionEventKind;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
    ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use serde_json::{json, Value};

/// Result of translating one canonical `jeden.session.v1` event to ACP.
/// Terminal events are intentionally not represented by custom notifications: ACP carries
/// completion in the `session/prompt` response.
pub(crate) struct MappedEvent {
    pub(crate) update: Option<SessionUpdate>,
    pub(crate) terminal: bool,
}

pub(crate) fn prompt_text(blocks: Vec<ContentBlock>) -> agent_client_protocol::Result<String> {
    let mut parts = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ContentBlock::Text(text) => parts.push(text.text),
            ContentBlock::ResourceLink(link) => {
                parts.push(format!("[{}]({})", link.name, link.uri))
            }
            ContentBlock::Image(_) => {
                return Err(super::invalid_params(
                    "image prompt content is not supported",
                ))
            }
            ContentBlock::Audio(_) => {
                return Err(super::unsupported(
                    "prompt.audio",
                    "audio prompt content is not supported",
                ))
            }
            ContentBlock::Resource(_) => {
                return Err(super::unsupported(
                    "prompt.embeddedContext",
                    "embedded resource prompt content is not supported",
                ))
            }
            _ => {
                return Err(super::unsupported(
                    "prompt.content",
                    "unsupported prompt content block",
                ))
            }
        }
    }
    let prompt = parts.join("\n");
    if prompt.trim().is_empty() {
        Err(super::invalid_params(
            "prompt must contain text or resource links",
        ))
    } else {
        Ok(prompt)
    }
}

/// Exhaustive canonical-event translation. Interaction events become typed ACP tool/input
/// requests in `AcpInteraction`; the mirrored update here preserves their observable semantic
/// state without inventing custom envelopes.
pub(crate) fn map_session_event(event: SessionEventKind, streamed: &mut bool) -> MappedEvent {
    match event {
        SessionEventKind::Status { message } => MappedEvent {
            update: Some(map_status(message)),
            terminal: false,
        },
        SessionEventKind::TextDelta { text } => {
            *streamed = true;
            MappedEvent {
                update: Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    text.into(),
                ))),
                terminal: false,
            }
        }
        SessionEventKind::Elicitation {
            token,
            question,
            options,
        } => MappedEvent {
            update: Some(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                format!(
                    "Input requested ({token}): {question} [{}]",
                    options.join(", ")
                )
                .into(),
            ))),
            terminal: false,
        },
        SessionEventKind::Approval {
            token,
            tool,
            detail,
        } => MappedEvent {
            update: Some(SessionUpdate::ToolCall(
                ToolCall::new(token, tool)
                    .status(ToolCallStatus::Pending)
                    .raw_input(json!({"detail": detail})),
            )),
            terminal: false,
        },
        // Goal-lifecycle updates have no ACP session-update equivalent; the
        // desktop consumes them over the RPC session-event stream instead.
        SessionEventKind::Goal { .. } => MappedEvent {
            update: None,
            terminal: false,
        },
        SessionEventKind::Result { text } => {
            let update = if *streamed {
                None
            } else {
                *streamed = true;
                Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    text.into(),
                )))
            };
            MappedEvent {
                update,
                terminal: true,
            }
        }
        SessionEventKind::Error { .. } => MappedEvent {
            update: None,
            terminal: true,
        },
    }
}

/// Status is normally free text. Structured canonical status payloads are translated into the
/// richer ACP variants; unknown/free-text status remains a thought chunk rather than being lost.
fn map_status(message: String) -> SessionUpdate {
    let Ok(value) = serde_json::from_str::<Value>(&message) else {
        return SessionUpdate::AgentThoughtChunk(ContentChunk::new(message.into()));
    };
    match value.get("kind").and_then(Value::as_str) {
        Some("toolCall") => {
            let id = string(&value, "toolCallId", "tool");
            let title = string(&value, "title", "Tool call");
            let mut call = ToolCall::new(id, title);
            if let Some(input) = value.get("input") {
                call = call.raw_input(input.clone());
            }
            SessionUpdate::ToolCall(call.status(tool_status(value.get("status"))))
        }
        Some("toolUpdate") | Some("toolResult") => {
            let id = string(&value, "toolCallId", "tool");
            let mut fields = ToolCallUpdateFields::new().status(tool_status(value.get("status")));
            if let Some(output) = value.get("output") {
                fields = fields.raw_output(output.clone());
            }
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id, fields))
        }
        Some("plan") => {
            let entries = value
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|entry| {
                    PlanEntry::new(
                        string(entry, "content", "Task"),
                        plan_priority(entry.get("priority")),
                        plan_status(entry.get("status")),
                    )
                })
                .collect();
            SessionUpdate::Plan(Plan::new(entries))
        }
        _ => SessionUpdate::AgentThoughtChunk(ContentChunk::new(message.into())),
    }
}

fn string(value: &Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn tool_status(value: Option<&Value>) -> ToolCallStatus {
    match value.and_then(Value::as_str) {
        Some("inProgress") | Some("in_progress") => ToolCallStatus::InProgress,
        Some("completed") => ToolCallStatus::Completed,
        Some("failed") => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
    }
}

fn plan_priority(value: Option<&Value>) -> PlanEntryPriority {
    match value.and_then(Value::as_str) {
        Some("high") => PlanEntryPriority::High,
        Some("low") => PlanEntryPriority::Low,
        _ => PlanEntryPriority::Medium,
    }
}

fn plan_status(value: Option<&Value>) -> PlanEntryStatus {
    match value.and_then(Value::as_str) {
        Some("inProgress") | Some("in_progress") => PlanEntryStatus::InProgress,
        Some("completed") => PlanEntryStatus::Completed,
        _ => PlanEntryStatus::Pending,
    }
}
