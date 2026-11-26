/// A `native` payload the worker refuses.
///
/// Workers return it, wrapped in a [`ProviderError`](crate::ProviderError), from
/// [`Provider::lease`](crate::Provider::lease); [`dispatch_failure_reply`](crate::dispatch_failure_reply)
/// turns it into the typed `native_error` dispatch reply.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("{reason}")]
pub struct NativeOptionsError {
    reason: String,
}

impl NativeOptionsError {
    /// A refusal that carries `reason` back to the caller.
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }

    /// Why the payload was refused.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}
