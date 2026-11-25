use crate::KeyUseDenialReason;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyOperationError {
    #[error("key use denied ({0:?})")]
    KeyUseDenied(KeyUseDenialReason),
    #[error("provider unavailable")]
    ProviderUnavailable,
    #[error("provider throttled")]
    ProviderThrottled,
    #[error("authenticated binding mismatch")]
    BindingMismatch,
    #[error("transport integrity mismatch")]
    TransportIntegrityMismatch,
    #[error("cryptographic integrity failure")]
    IntegrityFailure,
    #[error("unsupported key capability")]
    UnsupportedCapability,
    #[error("internal key operation failure")]
    InternalFailure,
}
