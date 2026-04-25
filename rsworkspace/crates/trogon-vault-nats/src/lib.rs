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

#[cfg(test)]
mod grace_period_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const VAR: &str = "VAULT_GRACE_PERIOD_SECS";

    #[test]
    fn defaults_to_30_secs_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var(VAR); }
        assert_eq!(grace_period_from_env(), std::time::Duration::from_secs(30));
    }

    #[test]
    fn returns_configured_value() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var(VAR, "60"); }
        let d = grace_period_from_env();
        unsafe { std::env::remove_var(VAR); }
        assert_eq!(d, std::time::Duration::from_secs(60));
    }

    #[test]
    fn falls_back_on_non_numeric_value() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var(VAR, "not-a-number"); }
        let d = grace_period_from_env();
        unsafe { std::env::remove_var(VAR); }
        assert_eq!(d, std::time::Duration::from_secs(30));
    }
}
