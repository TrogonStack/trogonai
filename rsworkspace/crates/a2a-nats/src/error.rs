//! JSON-RPC error codes used by the A2A NATS binding.
//!
//! Spec-defined codes (Section 8 / `tasks/*` and `message/*` operations of the JSON-RPC
//! binding) are reused where possible. Binding-specific codes (NATS transport issues that
//! have no spec analogue) use the JSON-RPC reserved server-error range -32000..-32099.

/// A2A-defined: requested task ID is unknown to the agent.
pub const TASK_NOT_FOUND: i32 = -32001;

/// A2A-defined: task exists but is not in a cancelable state (already terminal or interrupted).
pub const TASK_NOT_CANCELABLE: i32 = -32002;

/// A2A-defined: agent does not support push notifications.
pub const PUSH_NOTIFICATION_NOT_SUPPORTED: i32 = -32003;

/// A2A-defined: operation not supported by this agent's declared capabilities.
pub const UNSUPPORTED_OPERATION: i32 = -32004;

/// A2A-defined: requested content media type is not supported.
pub const CONTENT_TYPE_NOT_SUPPORTED: i32 = -32005;

/// A2A-defined: agent response is malformed or otherwise invalid.
pub const INVALID_AGENT_RESPONSE: i32 = -32006;

/// Binding-specific: no agent replicas are reachable on the agent subject (no responders).
pub const AGENT_UNAVAILABLE: i32 = -32050;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct() {
        let codes = [
            TASK_NOT_FOUND,
            TASK_NOT_CANCELABLE,
            PUSH_NOTIFICATION_NOT_SUPPORTED,
            UNSUPPORTED_OPERATION,
            CONTENT_TYPE_NOT_SUPPORTED,
            INVALID_AGENT_RESPONSE,
            AGENT_UNAVAILABLE,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in codes {
            assert!(seen.insert(code), "duplicate error code {code}");
        }
    }

    #[test]
    fn codes_in_jsonrpc_server_range() {
        // JSON-RPC reserves -32000..-32099 for server errors. All our binding-specific codes
        // should fall in that range. Spec-defined codes are also there per A2A binding rules.
        for code in [
            TASK_NOT_FOUND,
            TASK_NOT_CANCELABLE,
            PUSH_NOTIFICATION_NOT_SUPPORTED,
            UNSUPPORTED_OPERATION,
            CONTENT_TYPE_NOT_SUPPORTED,
            INVALID_AGENT_RESPONSE,
            AGENT_UNAVAILABLE,
        ] {
            assert!((-32099..=-32000).contains(&code), "{code} out of server range");
        }
    }
}
