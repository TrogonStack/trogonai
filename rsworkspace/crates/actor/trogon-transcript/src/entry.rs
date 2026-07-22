use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// The role of the message author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// A single entry in an actor's transcript.
///
/// Every LLM message, tool invocation, routing hop, and sub-agent spawn is
/// stored as one of these variants. The `type` field is used as the serde tag
/// so each entry is self-describing when stored as JSON in JetStream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEntry {
    /// An LLM conversation message (user prompt, assistant reply, system prompt,
    /// or a tool result injected as a user turn).
    Message {
        role: Role,
        content: String,
        timestamp: u64,
        tokens: Option<u32>,
    },
    /// A single tool call with its input, output, and wall-clock duration.
    ToolCall {
        name: String,
        input: Value,
        output: Value,
        duration_ms: u64,
        timestamp: u64,
    },
    /// A routing decision emitted by the Router Agent before dispatching an
    /// event to an Entity Actor.
    RoutingDecision {
        from: String,
        to: String,
        reasoning: String,
        timestamp: u64,
    },
    /// Recorded when an Entity Actor spawns a sub-agent for parallelizable work.
    SubAgentSpawn {
        parent: String,
        child: String,
        capability: String,
        timestamp: u64,
    },
}

/// Current time as milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
