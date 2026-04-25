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
