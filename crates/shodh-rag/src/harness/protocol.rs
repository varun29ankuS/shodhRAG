//! omp RPC frame types (the subset Shodh consumes and produces).
//!
//! omp speaks newline-delimited JSON over stdio (not JSON-RPC). Inbound frames
//! are parsed in two steps: first into a `serde_json::Value`, then by their
//! `type` string into a typed frame. An unknown frame type becomes
//! [`InboundFrame::Other`] and a known type whose shape changed becomes
//! [`InboundFrame::Malformed`], so a protocol change never stops the stream.
//!
//! Reference: omp `docs/rpc.md` (v18.4.10) and the recorded spike frames in
//! `fixtures/omp-spike-events.jsonl`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Inbound (omp stdout) ───────────────────────────────────────────────────

/// One parsed stdout frame.
#[derive(Debug, Clone, PartialEq)]
pub enum InboundFrame {
    Ready(ReadyFrame),
    Response(ResponseFrame),
    PromptResult(PromptResultFrame),
    AgentStart,
    AgentEnd(AgentEndFrame),
    TurnEnd,
    SessionSettled,
    MessageUpdate(MessageUpdateFrame),
    MessageEnd(MessageEndFrame),
    ToolExecutionStart(ToolExecutionStartFrame),
    ToolExecutionUpdate(ToolExecutionUpdateFrame),
    ToolExecutionEnd(ToolExecutionEndFrame),
    HostToolCall(HostToolCallFrame),
    HostToolCancel(HostToolCancelFrame),
    /// A dialog request (Code sessions answer approvals over it).
    ExtensionUiRequest(ExtensionUiRequestFrame),
    /// The session's slash commands (Code sessions look for the guard's).
    AvailableCommands(AvailableCommandsFrame),
    /// A frame type Shodh does not consume (e.g. `turn_start`, `subagent_event`).
    Other {
        frame_type: String,
    },
    /// A known frame type whose shape did not match. Logged and skipped.
    Malformed {
        frame_type: String,
        error: String,
    },
}

impl InboundFrame {
    /// The frame's `type` string, for logging.
    pub fn frame_type(&self) -> &str {
        match self {
            InboundFrame::Ready(_) => "ready",
            InboundFrame::Response(_) => "response",
            InboundFrame::PromptResult(_) => "prompt_result",
            InboundFrame::AgentStart => "agent_start",
            InboundFrame::AgentEnd(_) => "agent_end",
            InboundFrame::TurnEnd => "turn_end",
            InboundFrame::SessionSettled => "session_settled",
            InboundFrame::MessageUpdate(_) => "message_update",
            InboundFrame::MessageEnd(_) => "message_end",
            InboundFrame::ToolExecutionStart(_) => "tool_execution_start",
            InboundFrame::ToolExecutionUpdate(_) => "tool_execution_update",
            InboundFrame::ToolExecutionEnd(_) => "tool_execution_end",
            InboundFrame::HostToolCall(_) => "host_tool_call",
            InboundFrame::HostToolCancel(_) => "host_tool_cancel",
            InboundFrame::ExtensionUiRequest(_) => "extension_ui_request",
            InboundFrame::AvailableCommands(_) => "available_commands_update",
            InboundFrame::Other { frame_type } | InboundFrame::Malformed { frame_type, .. } => {
                frame_type
            }
        }
    }
}

/// Errors that make a line unusable as a frame at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("frame is not a JSON object with a string `type`")]
    MissingType,
}

/// Parse one stdout line into a frame.
pub fn parse_frame(line: &str) -> Result<InboundFrame, FrameError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| FrameError::InvalidJson(e.to_string()))?;
    let frame_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or(FrameError::MissingType)?
        .to_string();

    fn typed<T: serde::de::DeserializeOwned>(
        value: Value,
        frame_type: &str,
        wrap: fn(T) -> InboundFrame,
    ) -> InboundFrame {
        match serde_json::from_value::<T>(value) {
            Ok(frame) => wrap(frame),
            Err(e) => InboundFrame::Malformed {
                frame_type: frame_type.to_string(),
                error: e.to_string(),
            },
        }
    }

    let frame = match frame_type.as_str() {
        "ready" => typed(value, &frame_type, InboundFrame::Ready),
        "response" => typed(value, &frame_type, InboundFrame::Response),
        "prompt_result" => typed(value, &frame_type, InboundFrame::PromptResult),
        "agent_start" => InboundFrame::AgentStart,
        "agent_end" => typed(value, &frame_type, InboundFrame::AgentEnd),
        "turn_end" => InboundFrame::TurnEnd,
        "session_settled" => InboundFrame::SessionSettled,
        "message_update" => typed(value, &frame_type, InboundFrame::MessageUpdate),
        "message_end" => typed(value, &frame_type, InboundFrame::MessageEnd),
        "tool_execution_start" => typed(value, &frame_type, InboundFrame::ToolExecutionStart),
        "tool_execution_update" => typed(value, &frame_type, InboundFrame::ToolExecutionUpdate),
        "tool_execution_end" => typed(value, &frame_type, InboundFrame::ToolExecutionEnd),
        "host_tool_call" => typed(value, &frame_type, InboundFrame::HostToolCall),
        "host_tool_cancel" => typed(value, &frame_type, InboundFrame::HostToolCancel),
        "extension_ui_request" => typed(value, &frame_type, InboundFrame::ExtensionUiRequest),
        "available_commands_update" => typed(value, &frame_type, InboundFrame::AvailableCommands),
        _ => InboundFrame::Other { frame_type },
    };
    Ok(frame)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyFrame {
    pub protocol_version: u32,
    #[serde(default)]
    pub max_frame_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseFrame {
    #[serde(default)]
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ResponseFrame {
    /// A `prompt` response with `data.agentInvoked: false` completes the prompt
    /// locally; no `prompt_result` follows.
    pub fn completed_locally(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|d| d.get("agentInvoked"))
            .and_then(Value::as_bool)
            == Some(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStatus {
    Completed,
    Aborted,
    Error,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptError {
    pub message: String,
    #[serde(default)]
    pub retryable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResultFrame {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub agent_invoked: Option<bool>,
    pub status: PromptStatus,
    #[serde(default)]
    pub error: Option<PromptError>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndFrame {
    #[serde(default)]
    pub yielded: Option<bool>,
    #[serde(default)]
    pub is_terminal: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUpdateFrame {
    pub message_id: String,
    pub assistant_message_event: AssistantMessageEvent,
}

/// The streaming sub-event inside `message_update`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolcallEnd {
        #[serde(rename = "toolCall")]
        tool_call: ToolCallBlock,
    },
    #[serde(other)]
    Other,
}

/// A tool call as the model emitted it. `arguments.i` carries omp's
/// plain-language intent, which omp strips before `host_tool_call`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolCallBlock {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub intent: Option<String>,
}

impl ToolCallBlock {
    /// The intent label, from the explicit field or the `i` argument.
    pub fn intent(&self) -> Option<String> {
        self.intent
            .clone()
            .or_else(|| {
                self.arguments
                    .get("i")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEndFrame {
    #[serde(default)]
    pub message_id: Option<String>,
    pub message: EndedMessage,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndedMessage {
    pub role: String,
    /// Content blocks; for assistant messages, `toolCall` blocks carry `intent`.
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub usage: Option<MessageUsage>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
}

impl EndedMessage {
    /// Tool-call blocks found in the content array.
    pub fn tool_calls(&self) -> Vec<ToolCallBlock> {
        self.content
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("toolCall"))
                    .filter_map(|b| serde_json::from_value::<ToolCallBlock>(b.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUsage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: Option<UsageCost>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UsageCost {
    #[serde(default)]
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionStartFrame {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub intent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionUpdateFrame {
    pub tool_call_id: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub partial_result: Option<ToolResultPayload>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionEndFrame {
    pub tool_call_id: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub result: Option<ToolResultPayload>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostToolCallFrame {
    /// Request id; used by `host_tool_update`, `host_tool_result` and
    /// `host_tool_cancel.targetId`.
    pub id: String,
    /// The model's tool-call id; Shodh uses it as the step id.
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostToolCancelFrame {
    pub id: String,
    pub target_id: String,
}

/// `extension_ui_request`: a dialog (`select`, `confirm`, `input`, `editor`,
/// `ask`), its cancellation (`cancel` with `targetId`), or a presentation
/// update (`notify`, `setStatus`, `setWidget`, …) that needs no answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionUiRequestFrame {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// `input` dialogs carry their text here.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// For `cancel`: the dialog being closed.
    #[serde(default)]
    pub target_id: Option<String>,
}

impl ExtensionUiRequestFrame {
    /// Whether the request waits for an `extension_ui_response`.
    pub fn is_dialog(&self) -> bool {
        matches!(
            self.method.as_str(),
            "select" | "confirm" | "input" | "editor" | "ask"
        )
    }
}

/// `available_commands_update`: the session's slash commands.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AvailableCommandsFrame {
    #[serde(default)]
    pub commands: Vec<AvailableCommand>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AvailableCommand {
    pub name: String,
    /// `builtin`, `extension`, `file`, …
    #[serde(default)]
    pub source: Option<String>,
}

// ── Shared payloads ────────────────────────────────────────────────────────

/// A text content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text".to_string(),
            text: Some(text.into()),
        }
    }
}

/// Tool result content (`{ content: [{ type: "text", text }] }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultPayload {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
}

impl ToolResultPayload {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
        }
    }

    /// All text blocks joined by newlines.
    pub fn joined_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── Outbound (omp stdin) ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLoadMode {
    Essential,
    Discoverable,
}

/// A host tool definition for `set_host_tools`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostToolDefinition {
    pub name: String,
    pub label: String,
    pub description: String,
    pub parameters: Value,
    pub load_mode: ToolLoadMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentLevel {
    Off,
    Progress,
    Events,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageUpdateMode {
    Full,
    Delta,
}

/// A frame written to omp's stdin.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OutboundFrame {
    Prompt {
        id: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    Abort {
        id: String,
    },
    SetHostTools {
        id: String,
        tools: Vec<HostToolDefinition>,
    },
    SetEventFilter {
        id: String,
        /// `None` serialises as `null` (forward every event type).
        events: Option<Vec<String>>,
        message_updates: MessageUpdateMode,
    },
    SetSubagentSubscription {
        id: String,
        level: SubagentLevel,
    },
    GetSessionStats {
        id: String,
    },
    /// The session state, including its tool inventory (`dumpTools`).
    GetState {
        id: String,
    },
    /// The answer to a dialog: `value` for `input`/`select`, or `cancelled`.
    ExtensionUiResponse {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        cancelled: bool,
    },
    HostToolUpdate {
        id: String,
        partial_result: ToolResultPayload,
    },
    HostToolResult {
        id: String,
        result: ToolResultPayload,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

impl OutboundFrame {
    /// The command id, when this frame is a command that gets a `response`.
    pub fn command_id(&self) -> Option<&str> {
        match self {
            OutboundFrame::Prompt { id, .. }
            | OutboundFrame::Abort { id }
            | OutboundFrame::SetHostTools { id, .. }
            | OutboundFrame::SetEventFilter { id, .. }
            | OutboundFrame::SetSubagentSubscription { id, .. }
            | OutboundFrame::GetSessionStats { id }
            | OutboundFrame::GetState { id } => Some(id),
            OutboundFrame::HostToolUpdate { .. }
            | OutboundFrame::HostToolResult { .. }
            | OutboundFrame::ExtensionUiResponse { .. } => None,
        }
    }

    /// Serialise as one JSONL line (without the trailing newline).
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str = include_str!("fixtures/omp-spike-events.jsonl");

    #[test]
    fn every_fixture_line_parses_without_malformed_frames() {
        let mut count = 0;
        for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
            let frame = parse_frame(line).unwrap();
            assert!(
                !matches!(frame, InboundFrame::Malformed { .. }),
                "malformed: {frame:?}"
            );
            count += 1;
        }
        assert!(count > 300);
    }

    #[test]
    fn unknown_frames_and_events_are_tolerated() {
        let frame = parse_frame(r#"{"type":"brand_new_frame","x":1}"#).unwrap();
        assert_eq!(
            frame,
            InboundFrame::Other {
                frame_type: "brand_new_frame".into()
            }
        );
        let frame = parse_frame(
            r#"{"type":"message_update","messageId":"m","assistantMessageEvent":{"type":"future_delta","delta":"x"}}"#,
        )
        .unwrap();
        assert!(matches!(
            frame,
            InboundFrame::MessageUpdate(MessageUpdateFrame {
                assistant_message_event: AssistantMessageEvent::Other,
                ..
            })
        ));
        let frame = parse_frame(r#"{"type":"host_tool_call","id":5}"#).unwrap();
        assert!(matches!(frame, InboundFrame::Malformed { .. }));
        assert!(parse_frame("not json").is_err());
        assert_eq!(parse_frame("[1,2]"), Err(FrameError::MissingType));
    }

    #[test]
    fn host_tool_call_shape_matches_fixture() {
        let line = FIXTURE
            .lines()
            .find(|l| {
                l.contains("\"type\": \"host_tool_call\"")
                    || l.contains("\"type\":\"host_tool_call\"")
            })
            .unwrap();
        match parse_frame(line).unwrap() {
            InboundFrame::HostToolCall(call) => {
                assert_eq!(call.tool_name, "search_documents");
                assert!(call.tool_call_id.contains('|'));
                assert!(call.arguments.get("query").is_some());
                assert!(call.arguments.get("i").is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn outbound_frames_serialise_to_omp_shapes() {
        let prompt = OutboundFrame::Prompt {
            id: "p1".into(),
            message: "hi".into(),
            streaming_behavior: Some(StreamingBehavior::Steer),
        };
        assert_eq!(
            serde_json::to_value(&prompt).unwrap(),
            json!({"type":"prompt","id":"p1","message":"hi","streamingBehavior":"steer"})
        );
        let plain = OutboundFrame::Prompt {
            id: "p2".into(),
            message: "hi".into(),
            streaming_behavior: None,
        };
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            json!({"type":"prompt","id":"p2","message":"hi"})
        );
        let filter = OutboundFrame::SetEventFilter {
            id: "c1".into(),
            events: None,
            message_updates: MessageUpdateMode::Delta,
        };
        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({"type":"set_event_filter","id":"c1","events":null,"messageUpdates":"delta"})
        );
        let result = OutboundFrame::HostToolResult {
            id: "h1".into(),
            result: ToolResultPayload::text("done"),
            is_error: false,
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"type":"host_tool_result","id":"h1","result":{"content":[{"type":"text","text":"done"}]}})
        );
        let error = OutboundFrame::HostToolResult {
            id: "h1".into(),
            result: ToolResultPayload::text("no"),
            is_error: true,
        };
        assert_eq!(
            serde_json::to_value(&error).unwrap()["isError"],
            json!(true)
        );
        let update = OutboundFrame::HostToolUpdate {
            id: "h1".into(),
            partial_result: ToolResultPayload::text("working"),
        };
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({"type":"host_tool_update","id":"h1","partialResult":{"content":[{"type":"text","text":"working"}]}})
        );
        let tools = OutboundFrame::SetHostTools {
            id: "c3".into(),
            tools: vec![HostToolDefinition {
                name: "search_documents".into(),
                label: "Search documents".into(),
                description: "d".into(),
                parameters: json!({"type":"object"}),
                load_mode: ToolLoadMode::Essential,
            }],
        };
        assert_eq!(
            serde_json::to_value(&tools).unwrap()["tools"][0]["loadMode"],
            json!("essential")
        );
        assert_eq!(
            serde_json::to_value(OutboundFrame::SetSubagentSubscription {
                id: "c2".into(),
                level: SubagentLevel::Off
            })
            .unwrap(),
            json!({"type":"set_subagent_subscription","id":"c2","level":"off"})
        );
    }

    #[test]
    fn prompt_response_completed_locally() {
        let frame = parse_frame(
            r#"{"id":"p","type":"response","command":"prompt","success":true,"data":{"agentInvoked":false}}"#,
        )
        .unwrap();
        match frame {
            InboundFrame::Response(r) => assert!(r.completed_locally()),
            other => panic!("unexpected {other:?}"),
        }
    }
}
