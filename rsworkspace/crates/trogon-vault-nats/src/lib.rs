//! NATS JetStream Key-Value cache for [`trogon_vault::VaultStore`].
//!
//! Wraps a NATS KV bucket with AES-256-GCM encryption (Argon2id KDF) and an
//! in-process [`dashmap::DashMap`] cache for nanosecond-latency resolution.
//! Supports named vaults (one bucket per name) and zero-downtime rotation via
//! [`RotationSlot`].

pub mod audit;
pub mod backend;
pub mod bucket;
pub mod crypto;
pub mod error;
pub mod slot;

pub use audit::{AuditEvent, AuditPublisher, ensure_audit_stream, ensure_audit_stream_with_max_age};
pub use backend::NatsKvVault;
pub use bucket::ensure_vault_bucket;
pub use crypto::CryptoCtx;
pub use error::NatsKvVaultError;
pub use slot::RotationSlot;

/// Read the rotation grace period from `VAULT_GRACE_PERIOD_SECS`.
///
/// Defaults to 30 seconds if the variable is absent or unparseable.
pub fn grace_period_from_env() -> std::time::Duration {
    std::env::var("VAULT_GRACE_PERIOD_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(30))
}
