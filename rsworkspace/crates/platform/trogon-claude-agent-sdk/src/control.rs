use crate::{JsonObject, MessageId};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RewindMode {
    Apply,
    DryRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpEnabled {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FeatureSupport {
    #[default]
    NotReported,
    Supported,
    Unsupported,
}

impl From<Option<bool>> for FeatureSupport {
    fn from(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Supported,
            Some(false) => Self::Unsupported,
            None => Self::NotReported,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionInfo {
    pub commands: Vec<JsonObject>,
    pub models: Vec<JsonObject>,
    pub agents: Vec<JsonObject>,
    pub account: Option<JsonObject>,
    pub hooks_applied: FeatureSupport,
    pub hook_on_failure_supported: FeatureSupport,
    pub claude_code_version: Option<String>,
    pub additional_fields: JsonObject,
    raw: JsonObject,
}

#[derive(Deserialize)]
struct ConnectionInfoWire {
    #[serde(default)]
    commands: Vec<JsonObject>,
    #[serde(default)]
    models: Vec<JsonObject>,
    #[serde(default)]
    agents: Vec<JsonObject>,
    account: Option<JsonObject>,
    hooks_applied: Option<bool>,
    hook_on_failure_supported: Option<bool>,
    claude_code_version: Option<String>,
    #[serde(flatten)]
    additional_fields: JsonObject,
}

impl ConnectionInfo {
    pub fn from_json(value: JsonObject) -> Result<Self, serde_json::Error> {
        let wire: ConnectionInfoWire = serde_json::from_value(value.clone().into())?;
        Ok(Self {
            commands: wire.commands,
            models: wire.models,
            agents: wire.agents,
            account: wire.account,
            hooks_applied: wire.hooks_applied.into(),
            hook_on_failure_supported: wire.hook_on_failure_supported.into(),
            claude_code_version: wire.claude_code_version,
            additional_fields: wire.additional_fields,
            raw: value,
        })
    }

    /// The original handshake snapshot, preserving omitted versus explicitly null fields.
    pub fn as_json(&self) -> &JsonObject {
        &self.raw
    }
}

#[derive(Clone, Debug)]
pub enum InterruptReceipt {
    NotReported {
        additional_fields: JsonObject,
    },
    Reported {
        still_queued: Vec<MessageId>,
        cancelled: Option<Vec<MessageId>>,
        additional_fields: JsonObject,
    },
}

impl InterruptReceipt {
    /// Reads the queue report the CLI attaches to an interrupt response.
    ///
    /// A missing or non-array `still_queued` means the CLI did not report its queue. Non-string ids are dropped and a
    /// non-array `cancelled` is ignored, so a malformed report never fails the interrupt.
    pub fn from_json(mut value: JsonObject) -> Self {
        if !value.get("still_queued").is_some_and(serde_json::Value::is_array) {
            return Self::NotReported {
                additional_fields: value,
            };
        }
        let message_ids = |field: Option<serde_json::Value>| match field {
            Some(serde_json::Value::Array(values)) => Some(
                values
                    .into_iter()
                    .filter_map(|value| match value {
                        serde_json::Value::String(id) => Some(MessageId::from(id)),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        };
        let still_queued = message_ids(value.remove("still_queued")).unwrap_or_default();
        let cancelled = message_ids(value.remove("cancelled"));
        Self::Reported {
            still_queued,
            cancelled,
            additional_fields: value,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindFilesResult {
    pub can_rewind: bool,
    pub error: Option<String>,
    pub files_changed: Option<Vec<String>>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    #[serde(flatten)]
    pub additional_fields: JsonObject,
}

#[derive(Clone, Debug, Deserialize)]
pub struct McpStatusResponse {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: Vec<JsonObject>,
    #[serde(flatten)]
    pub additional_fields: JsonObject,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageCategory {
    pub name: String,
    pub tokens: u64,
    pub color: String,
    pub is_deferred: Option<bool>,
    #[serde(flatten)]
    pub additional_fields: JsonObject,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageResponse {
    pub categories: Vec<ContextUsageCategory>,
    pub total_tokens: u64,
    pub max_tokens: u64,
    pub raw_max_tokens: u64,
    pub percentage: f64,
    pub model: String,
    pub is_auto_compact_enabled: bool,
    pub memory_files: Vec<JsonObject>,
    pub mcp_tools: Vec<JsonObject>,
    #[serde(flatten)]
    pub additional_fields: JsonObject,
}

#[derive(Clone, Debug, Deserialize)]
pub struct McpSetServersResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: std::collections::BTreeMap<String, String>,
    #[serde(flatten)]
    pub additional_fields: JsonObject,
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
