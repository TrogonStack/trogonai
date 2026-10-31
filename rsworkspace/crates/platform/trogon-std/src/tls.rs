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
//! ahead of any client construction or task spawn. Libraries that open TLS
//! connections on a binary's behalf call it too, so every caller shares one
//! provider no matter which of them runs first.

/// Installs aws-lc-rs as the process-wide rustls
/// [`rustls::crypto::CryptoProvider`].
///
/// Calling it again once a provider is installed does nothing, so a binary and
/// the libraries it links can each make sure a provider exists without
/// coordinating which of them goes first.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(test)]
mod tests;
