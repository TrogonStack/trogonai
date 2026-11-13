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

/// The initialize response, read the way both upstream SDKs read it: neither validates it, so a field of an
/// unexpected type reads as absent here and stays untouched in [`ConnectionInfo::as_json`].
#[derive(Clone, Debug, Default)]
pub struct ConnectionInfo {
    pub commands: Vec<JsonObject>,
    pub models: Vec<JsonObject>,
    pub agents: Vec<JsonObject>,
    pub account: Option<JsonObject>,
    pub output_style: Option<String>,
    pub available_output_styles: Option<Vec<String>>,
    pub hooks_applied: FeatureSupport,
    pub hook_on_failure_supported: FeatureSupport,
    pub plugins_applied: FeatureSupport,
    /// Whether the managed settings sent in the initialize request were applied.
    pub managed_settings_applied: FeatureSupport,
    /// What became of each `sdkMcpServerManifests` entry, keyed by SDK MCP server name.
    pub sdk_mcp_manifests_parked: Option<std::collections::BTreeMap<String, String>>,
    pub claude_code_version: Option<String>,
    pub fast_mode_state: Option<String>,
    pub fast_mode_disabled_reason: Option<String>,
    /// Response fields this type does not name.
    pub additional_fields: JsonObject,
    raw: serde_json::Value,
}

const CONNECTION_INFO_FIELDS: &[&str] = &[
    "commands",
    "models",
    "agents",
    "account",
    "output_style",
    "available_output_styles",
    "hooks_applied",
    "hook_on_failure_supported",
    "plugins_applied",
    "managed_settings_applied",
    "sdk_mcp_manifests_parked",
    "claude_code_version",
    "fast_mode_state",
    "fast_mode_disabled_reason",
];

impl ConnectionInfo {
    /// Reads an initialize response. Any JSON value is accepted, as upstream accepts it.
    pub fn from_json(value: serde_json::Value) -> Self {
        let empty = JsonObject::new();
        let object = value.as_object().unwrap_or(&empty);
        fn field<T: serde::de::DeserializeOwned>(object: &JsonObject, key: &str) -> Option<T> {
            object.get(key).and_then(|value| T::deserialize(value).ok())
        }
        let support = |key: &str| FeatureSupport::from(object.get(key).and_then(serde_json::Value::as_bool));
        Self {
            commands: field(object, "commands").unwrap_or_default(),
            models: field(object, "models").unwrap_or_default(),
            agents: field(object, "agents").unwrap_or_default(),
            account: field(object, "account"),
            output_style: field(object, "output_style"),
            available_output_styles: field(object, "available_output_styles"),
            hooks_applied: support("hooks_applied"),
            hook_on_failure_supported: support("hook_on_failure_supported"),
            plugins_applied: support("plugins_applied"),
            managed_settings_applied: support("managed_settings_applied"),
            sdk_mcp_manifests_parked: field(object, "sdk_mcp_manifests_parked"),
            claude_code_version: field(object, "claude_code_version"),
            fast_mode_state: field(object, "fast_mode_state"),
            fast_mode_disabled_reason: field(object, "fast_mode_disabled_reason"),
            additional_fields: object
                .iter()
                .filter(|(key, _)| !CONNECTION_INFO_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            raw: value,
        }
    }

    /// The response exactly as the CLI sent it, preserving omitted versus explicitly null fields.
    pub fn as_json(&self) -> &serde_json::Value {
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
