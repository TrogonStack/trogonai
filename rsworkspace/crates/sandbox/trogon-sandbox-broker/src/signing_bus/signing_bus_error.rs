use trogon_sandbox_protocol::DispatchSignerConfigError;

/// Why the signing bus could not be built from its environment.
#[derive(Debug, thiserror::Error)]
pub enum SigningBusError {
    /// The signing key configuration is unusable.
    #[error("dispatch signing is misconfigured: {0}")]
    Misconfigured(#[from] DispatchSignerConfigError),
}
