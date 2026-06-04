//! TLS 1.3-only edge for the MCP gateway with SPIFFE SVID-aware
//! mTLS verification and hot reload of cert material.
//!
//! Three primitives:
//! - [`TlsConfig`] — load PEMs, build rustls server/client configs.
//! - [`SpiffeClientCertVerifier`] — validates client certs against a
//!   trust bundle and (optionally) a SPIFFE trust-domain allowlist
//!   extracted from the URI SAN.
//! - [`PemReloader`] — file watcher that re-reads cert PEMs and swaps
//!   the active material atomically, without restarting the process.
//!
//! Production code paths reject TLS 1.2 and below (`rustls`
//! `DEFAULT_VERSIONS` includes 1.2; we override to TLS 1.3 only).

pub mod config;
pub mod reload;
pub mod verifier;

pub use config::{TlsConfig, TlsConfigError, TlsMaterial};
pub use reload::{PemReloader, ReloadError, ReloadEvent};
pub use verifier::{
    ERR_MTLS_REQUIRED, ERR_MTLS_SAN_MISMATCH, ERR_TLS_REQUIRED, ERR_TLS_VERSION_UNSUPPORTED, SpiffeClientCertVerifier,
    SpiffeIdentity, SpiffeVerifierError, TLS_MIN_VERSION,
};
