//! JSON-RPC error codes used by the A2A NATS binding.
//!
//! Spec-defined codes (Section 8 / `tasks/*` and `message/*` operations of the JSON-RPC
//! binding) are reused where possible. Binding-specific codes (NATS transport issues that
//! have no spec analogue) use the JSON-RPC reserved server-error range -32000..-32099.

pub use crate::constants::{
    AGENT_UNAVAILABLE, CONTENT_TYPE_NOT_SUPPORTED, EXTENDED_AGENT_CARD_NOT_CONFIGURED, EXTENSION_SUPPORT_REQUIRED,
    INVALID_AGENT_RESPONSE, PUSH_NOTIFICATION_NOT_SUPPORTED, TASK_NOT_CANCELABLE, TASK_NOT_FOUND,
    UNSUPPORTED_OPERATION, VERSION_NOT_SUPPORTED,
};

#[cfg(test)]
mod tests;
