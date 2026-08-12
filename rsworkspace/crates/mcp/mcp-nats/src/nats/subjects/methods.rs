//! MCP method ↔ subject-terminal mapping (ADR#0055: total and bidirectional).
//!
//! Known MCP methods map to dotted subject terminals. Unknown methods use the
//! `custom.{base64url}` escape so the mapping stays total.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::{McpPeerId, McpPrefix};

#[derive(Debug, thiserror::Error)]
pub enum MethodMapError {
    #[error("unsupported MCP method `{method}`")]
    UnsupportedMethod { method: String },
    #[error("invalid custom method suffix `{suffix}`")]
    InvalidCustomMethodSuffix { suffix: String },
}

macro_rules! method_table {
    ($(($method:literal, $suffix:literal)),+ $(,)?) => {
        /// Map a JSON-RPC method string to its NATS subject terminal.
        pub fn method_suffix(method: &str) -> Result<String, MethodMapError> {
            match method {
                $($method => Ok($suffix.to_string()),)+
                _ => Ok(custom_method_suffix(method)),
            }
        }

        /// Inverse of [`method_suffix`].
        pub fn method_from_suffix(suffix: &str) -> Result<String, MethodMapError> {
            match suffix {
                $($suffix => Ok($method.to_string()),)+
                _ => method_from_custom_suffix(suffix),
            }
        }

        #[cfg(test)]
        pub const METHOD_TABLE: &[(&str, &str)] = &[
            $(($method, $suffix)),+
        ];
    };
}

fn custom_method_suffix(method: &str) -> String {
    let encoded = if method.is_empty() {
        "_".to_string()
    } else {
        URL_SAFE_NO_PAD.encode(method.as_bytes())
    };
    format!("custom.{encoded}")
}

fn method_from_custom_suffix(suffix: &str) -> Result<String, MethodMapError> {
    let encoded = suffix
        .strip_prefix("custom.")
        .ok_or_else(|| MethodMapError::UnsupportedMethod {
            method: suffix.to_string(),
        })?;
    let bytes = if encoded == "_" {
        Vec::new()
    } else {
        URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| MethodMapError::InvalidCustomMethodSuffix {
                suffix: suffix.to_string(),
            })?
    };
    String::from_utf8(bytes).map_err(|_| MethodMapError::InvalidCustomMethodSuffix {
        suffix: suffix.to_string(),
    })
}

method_table! {
    ("initialize", "initialize"),
    ("ping", "ping"),
    ("server/discover", "server.discover"),
    ("completion/complete", "completion.complete"),
    ("logging/setLevel", "logging.set_level"),
    ("prompts/list", "prompts.list"),
    ("prompts/get", "prompts.get"),
    ("resources/list", "resources.list"),
    ("resources/templates/list", "resources.templates.list"),
    ("resources/read", "resources.read"),
    ("subscriptions/listen", "subscriptions.listen"),
    ("resources/subscribe", "resources.subscribe"),
    ("resources/unsubscribe", "resources.unsubscribe"),
    ("tools/list", "tools.list"),
    ("tools/call", "tools.call"),
    ("tasks/get", "tasks.get"),
    ("tasks/update", "tasks.update"),
    ("tasks/cancel", "tasks.cancel"),
    ("notifications/cancelled", "notifications.cancelled"),
    ("notifications/progress", "notifications.progress"),
    ("notifications/message", "notifications.message"),
    ("notifications/resources/updated", "notifications.resources.updated"),
    ("notifications/resources/list_changed", "notifications.resources.list_changed"),
    ("notifications/tools/list_changed", "notifications.tools.list_changed"),
    ("notifications/prompts/list_changed", "notifications.prompts.list_changed"),
    ("notifications/tasks", "notifications.tasks"),
    ("notifications/subscriptions/acknowledged", "notifications.subscriptions.acknowledged"),
    ("sampling/createMessage", "sampling.create_message"),
    ("roots/list", "roots.list"),
    ("elicitation/create", "elicitation.create"),
    ("notifications/initialized", "notifications.initialized"),
    ("notifications/roots/list_changed", "notifications.roots.list_changed"),
}

/// Role token in `{prefix}.v1.{role}.{peer_id}.{terminal}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRole {
    Server,
    Client,
}

impl McpRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }
}

/// Value object for a peer-scoped MCP subject.
#[derive(Debug, Clone)]
pub struct PeerSubject {
    prefix: McpPrefix,
    role: McpRole,
    peer_id: McpPeerId,
    suffix: String,
}

impl PeerSubject {
    pub fn new(prefix: &McpPrefix, role: McpRole, peer_id: &McpPeerId, suffix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.clone(),
            role,
            peer_id: peer_id.clone(),
            suffix: suffix.into(),
        }
    }

    pub fn for_method(
        prefix: &McpPrefix,
        role: McpRole,
        peer_id: &McpPeerId,
        method: &str,
    ) -> Result<Self, MethodMapError> {
        Ok(Self::new(prefix, role, peer_id, method_suffix(method)?))
    }
}

impl std::fmt::Display for PeerSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.{}.{}.{}",
            self.prefix.as_str(),
            self.role.as_str(),
            self.peer_id.as_str(),
            self.suffix
        )
    }
}
