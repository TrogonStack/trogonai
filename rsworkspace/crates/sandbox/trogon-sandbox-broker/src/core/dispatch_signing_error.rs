use trogon_sandbox_protocol::DispatchSignerConfigError;

/// Why the broker could not install the dispatch signer from its environment.
#[derive(Debug, thiserror::Error)]
pub enum DispatchSigningError {
    /// A signing key is configured but unusable.
    #[error("dispatch signing is misconfigured: {0}")]
    Misconfigured(#[source] DispatchSignerConfigError),
}
