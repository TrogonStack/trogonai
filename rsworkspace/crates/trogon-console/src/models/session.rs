use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Idle,
}

/// Console view of a chat session — deserialized from the SESSIONS KV bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleSession {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub model: Option<String>,
    pub status: SessionStatus,
    pub message_count: usize,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub duration_ms: u64,
    pub agent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Raw session as stored in NATS KV by trogon-agent. We deserialize only
/// the fields we need and derive `status` + token counts from `messages`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawSession {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub model: Option<String>,
    #[serde(default)]
    pub messages: Vec<RawMessage>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Top-level token counts written by xai-runner (cumulative across all turns).
    /// Used as fallback when messages do not carry per-message usage fields.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawMessage {
    pub role: String,
    #[serde(default)]
    pub usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl From<RawSession> for ConsoleSession {
    fn from(r: RawSession) -> Self {
        let per_msg_input: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.input_tokens)
            .sum();
        let per_msg_output: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.output_tokens)
            .sum();
        let cache_read_tokens: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.cache_read_input_tokens)
            .sum();
        let cache_write_tokens: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.cache_creation_input_tokens)
            .sum();

        // Prefer per-message usage (trogon-acp-runner style). Fall back to the
        // top-level cumulative counts written by trogon-xai-runner.
        let input_tokens = if per_msg_input > 0 {
            per_msg_input
        } else {
            r.input_tokens.min(u32::MAX as u64) as u32
        };
        let output_tokens = if per_msg_output > 0 {
            per_msg_output
        } else {
            r.output_tokens.min(u32::MAX as u64) as u32
        };

        // A session is "running" if the last message is from the user
        // (agent hasn't responded yet) or if it was updated very recently.
        let status = r.messages.last()
            .map(|m| if m.role == "user" { SessionStatus::Running } else { SessionStatus::Idle })
            .unwrap_or(SessionStatus::Idle);

        ConsoleSession {
            id: r.id,
            tenant_id: r.tenant_id,
            name: r.name,
            model: r.model,
            status,
            message_count: r.messages.len(),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            duration_ms: r.duration_ms,
            agent_id: r.agent_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}
