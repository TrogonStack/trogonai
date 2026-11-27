//! Maps the provider's internal failures onto the typed [`ProviderError`].

use trogon_sandbox_protocol::{NativeOptionsError, PrivilegedUnsupportedError, ProviderError};

/// Keeps the refusals the dispatcher answers with a typed reply, and wraps
/// everything else as a backend failure.
pub(crate) fn into_provider_error(error: anyhow::Error) -> ProviderError {
    let error = match error.downcast::<NativeOptionsError>() {
        Ok(refusal) => return ProviderError::NativeOptions(refusal),
        Err(error) => error,
    };
    match error.downcast::<PrivilegedUnsupportedError>() {
        Ok(refusal) => ProviderError::PrivilegedUnsupported(refusal),
        Err(error) => ProviderError::backend(error),
    }
}
