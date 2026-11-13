use crate::{JsonObject, MessageId, constants::CONNECTION_INFO_FIELDS};
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
    raw: Option<serde_json::Value>,
}

impl ConnectionInfo {
    /// Reads an initialize response. Any JSON value is accepted, as upstream accepts it.
    pub fn from_json(value: serde_json::Value) -> Self {
        Self::from_response(Some(value))
    }

    /// Reads the `response` field of an initialize reply, `None` when the reply has none.
    pub(crate) fn from_response(value: Option<serde_json::Value>) -> Self {
        let empty = JsonObject::new();
        let object = value.as_ref().and_then(serde_json::Value::as_object).unwrap_or(&empty);
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

    /// The response exactly as the CLI sent it, preserving omitted versus explicitly null fields. `None` when the
    /// reply had no `response` field, where TypeScript `initializationResult()` resolves to `undefined`.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        self.raw.as_ref()
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

/// A control response kept exactly as the CLI sent it. Neither upstream SDK validates control responses, so
/// reading one never fails here; [`ControlReply::decode`] reads it as the pinned upstream type on request.
pub struct ControlReply<T> {
    value: Option<serde_json::Value>,
    shape: std::marker::PhantomData<fn() -> T>,
}

impl<T> ControlReply<T> {
    pub(crate) fn new(value: Option<serde_json::Value>) -> Self {
        Self {
            value,
            shape: std::marker::PhantomData,
        }
    }

    /// The response as sent; `None` when the CLI sent none, where TypeScript returns `undefined`.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        self.value.as_ref()
    }

    pub fn into_json(self) -> Option<serde_json::Value> {
        self.value
    }
}

impl<T: serde::de::DeserializeOwned> ControlReply<T> {
    /// Reads the response as the type the pinned TypeScript declarations give it; an absent response reads as
    /// `null`.
    pub fn decode(&self) -> Result<T, serde_json::Error> {
        T::deserialize(self.value.as_ref().unwrap_or(&serde_json::Value::Null))
    }
}

impl<T> Clone for ControlReply<T> {
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

impl<T> std::fmt::Debug for ControlReply<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ControlReply").field(&self.value).finish()
    }
}

impl<T> PartialEq for ControlReply<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// `setMcpPermissionModeOverride` modes; `None` clears the override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpPermissionModeOverride {
    Default,
    Auto,
}

impl McpPermissionModeOverride {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
        }
    }
}

/// `setMaxThinkingTokens` display modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxThinkingTokensDisplay {
    Summarized,
    Omitted,
    Highlights,
}

impl MaxThinkingTokensDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summarized => "summarized",
            Self::Omitted => "omitted",
            Self::Highlights => "highlights",
        }
    }
}

/// The settings file `updateSettings` writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsUpdateSource {
    LocalSettings,
    UserSettings,
}

impl SettingsUpdateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalSettings => "localSettings",
            Self::UserSettings => "userSettings",
        }
    }
}

/// `getContextUsage({detail})`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextUsageDetail {
    Summary,
    Full,
}

impl ContextUsageDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Full => "full",
        }
    }
}

/// `readFile` encodings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFileEncoding {
    Utf8,
    Base64,
}

impl ReadFileEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Base64 => "base64",
        }
    }
}

/// `readFile(path, {maxBytes, encoding})`; an unset field is left out of the request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadFileOptions {
    pub max_bytes: Option<u64>,
    pub encoding: Option<ReadFileEncoding>,
}

/// `reloadPlugins({holdOnCacheImpact, addPlugins})`. Like TypeScript, `hold_on_cache_impact` is sent only when
/// set and `add_plugins` only when it lists a plugin.
#[derive(Clone, Debug, Default)]
pub struct ReloadPluginsOptions {
    pub hold_on_cache_impact: bool,
    pub add_plugins: Vec<crate::LocalPlugin>,
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
