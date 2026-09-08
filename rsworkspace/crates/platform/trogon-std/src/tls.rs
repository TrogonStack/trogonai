//! Process-wide TLS provider selection for rustls.
//!
//! rustls only derives its process-level [`rustls::crypto::CryptoProvider`] from
//! crate features when exactly one provider feature is enabled. A dependency
//! graph that reaches both `aws-lc-rs` and `ring` leaves the choice ambiguous,
//! and rustls panics on the first handshake rather than guessing. Installing the
//! provider explicitly removes the dependency on feature unification, so adding
//! a dependency cannot change which cryptography a binary uses.
//!
//! Call [`install_default_crypto_provider`] before the first TLS handshake,
//! ahead of any client construction or task spawn.

/// A rustls [`rustls::crypto::CryptoProvider`] was already installed for this
/// process, so the caller has no guarantee which cryptography it now uses.
#[derive(Debug, thiserror::Error)]
#[error("a rustls CryptoProvider was already installed for this process")]
pub struct CryptoProviderAlreadyInstalledError;

/// Installs aws-lc-rs as the process-wide rustls
/// [`rustls::crypto::CryptoProvider`].
///
/// Fails rather than accepting whichever provider got there first, so a second
/// installer cannot silently downgrade the process to different cryptography.
pub fn install_default_crypto_provider() -> Result<(), CryptoProviderAlreadyInstalledError> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| CryptoProviderAlreadyInstalledError)
}

#[cfg(test)]
mod tests;
