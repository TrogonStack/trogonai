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
}

impl From<RawSession> for ConsoleSession {
    fn from(r: RawSession) -> Self {
        let input_tokens: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.input_tokens)
            .sum();
        let output_tokens: u32 = r.messages.iter()
            .filter_map(|m| m.usage.as_ref())
            .map(|u| u.output_tokens)
            .sum();

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
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}
