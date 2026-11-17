use std::sync::Arc;

/// Why a value is not the secrets service's public curve key.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ServiceXkeyError {
    #[error("service xkey is not a curve public key")]
    NotACurvePublicKey(#[source] Arc<nkeys::error::Error>),
}
