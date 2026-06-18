//! Server-side error surface for A2A handler implementations.
//!
//! The `A2aExecutor` trait that handler implementations satisfy grows method-by-method
//! as each operation lands in its own PR. This file ships the error type those methods
//! return and the streaming alias `message/stream` and `tasks/resubscribe` reach for.

use std::pin::Pin;

use futures::Stream;

use crate::error::{
    AGENT_UNAVAILABLE, CONTENT_TYPE_NOT_SUPPORTED, EXTENDED_AGENT_CARD_NOT_CONFIGURED, EXTENSION_SUPPORT_REQUIRED,
    INVALID_AGENT_RESPONSE, PUSH_NOTIFICATION_NOT_SUPPORTED, TASK_NOT_CANCELABLE, TASK_NOT_FOUND,
    UNSUPPORTED_OPERATION, VERSION_NOT_SUPPORTED,
};

pub type TaskEventStream = Pin<Box<dyn Stream<Item = Result<a2a::event::StreamResponse, A2aError>> + Send + 'static>>;

/// Error returned by an `A2aExecutor` implementation and mapped to a JSON-RPC error response.
#[derive(Debug, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct A2aError {
    pub code: i32,
    pub message: String,
}

impl A2aError {
    /// Construct an error with an arbitrary JSON-RPC code.
    ///
    /// Prefer the typed helpers below (`task_not_found`, `task_not_cancelable`, …) for
    /// spec-defined codes. This constructor is the escape hatch for codes the protocol
    /// adds before this crate ships matching helpers; tracked for a typed `A2aErrorCode`
    /// value object in a follow-up that restricts construction to known codes.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn task_not_found(message: impl Into<String>) -> Self {
        Self::new(TASK_NOT_FOUND, message)
    }

    pub fn task_not_cancelable(message: impl Into<String>) -> Self {
        Self::new(TASK_NOT_CANCELABLE, message)
    }

    pub fn push_notification_not_supported(message: impl Into<String>) -> Self {
        Self::new(PUSH_NOTIFICATION_NOT_SUPPORTED, message)
    }

    pub fn unsupported_operation(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_OPERATION, message)
    }

    pub fn content_type_not_supported(message: impl Into<String>) -> Self {
        Self::new(CONTENT_TYPE_NOT_SUPPORTED, message)
    }

    pub fn invalid_agent_response(message: impl Into<String>) -> Self {
        Self::new(INVALID_AGENT_RESPONSE, message)
    }

    pub fn agent_unavailable(message: impl Into<String>) -> Self {
        Self::new(AGENT_UNAVAILABLE, message)
    }

    pub fn extended_agent_card_not_configured(message: impl Into<String>) -> Self {
        Self::new(EXTENDED_AGENT_CARD_NOT_CONFIGURED, message)
    }

    pub fn extension_support_required(message: impl Into<String>) -> Self {
        Self::new(EXTENSION_SUPPORT_REQUIRED, message)
    }

    pub fn version_not_supported(message: impl Into<String>) -> Self {
        Self::new(VERSION_NOT_SUPPORTED, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_carries_code_and_message() {
        let e = A2aError::new(-32000, "boom");
        assert_eq!(e.code, -32000);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn helpers_use_their_spec_codes() {
        assert_eq!(A2aError::task_not_found("x").code, TASK_NOT_FOUND);
        assert_eq!(A2aError::task_not_cancelable("x").code, TASK_NOT_CANCELABLE);
        assert_eq!(
            A2aError::push_notification_not_supported("x").code,
            PUSH_NOTIFICATION_NOT_SUPPORTED
        );
        assert_eq!(A2aError::unsupported_operation("x").code, UNSUPPORTED_OPERATION);
        assert_eq!(
            A2aError::content_type_not_supported("x").code,
            CONTENT_TYPE_NOT_SUPPORTED
        );
        assert_eq!(A2aError::invalid_agent_response("x").code, INVALID_AGENT_RESPONSE);
        assert_eq!(A2aError::agent_unavailable("x").code, AGENT_UNAVAILABLE);
        assert_eq!(
            A2aError::extended_agent_card_not_configured("x").code,
            EXTENDED_AGENT_CARD_NOT_CONFIGURED
        );
        assert_eq!(
            A2aError::extension_support_required("x").code,
            EXTENSION_SUPPORT_REQUIRED
        );
        assert_eq!(A2aError::version_not_supported("x").code, VERSION_NOT_SUPPORTED);
        assert_eq!(A2aError::internal("x").code, -32603);
    }

    #[test]
    fn display_includes_code_and_message() {
        assert_eq!(
            format!("{}", A2aError::task_not_found("missing")),
            format!("[{TASK_NOT_FOUND}] missing")
        );
    }

    #[test]
    fn implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(A2aError::internal("oops"));
        assert!(e.to_string().contains("oops"));
    }
}
