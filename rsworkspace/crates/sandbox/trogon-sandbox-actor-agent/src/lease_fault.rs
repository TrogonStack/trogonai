use trogon_sandbox_contract::LeaseError;

/// A backend fault with a message, kept as one call so error mapping stays on one line at every call site.
pub(crate) fn backend(message: String) -> LeaseError {
    LeaseError::Backend { message }
}

/// A request the sandbox cannot satisfy as specified.
pub(crate) fn invalid_spec(reason: String) -> LeaseError {
    LeaseError::InvalidSpec { reason }
}
