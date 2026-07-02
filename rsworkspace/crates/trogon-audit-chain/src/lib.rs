#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

//! # trogon-audit-chain
//!
//! Tamper-evident SHA-256 hash chain for audit events durably stored on
//! NATS JetStream.
//!
//! This crate has two layers:
//!
//! - A pure chain core ([`ChainHash`], [`CanonicalEventBytes`], [`extend`],
//!   [`verify_chain`]) with no NATS dependency, usable and testable in
//!   complete isolation.
//! - A JetStream integration layer ([`ChainPublisher`],
//!   [`verify_stream_chain`]) built on `trogon-nats`'s JetStream traits.
//!
//! See the crate README for the threat model and how this maps to
//! AUDIT-COMPLIANCE-1.0 Level 3.

mod canonical_event_bytes;
mod chain_break;
mod chain_entry;
mod chain_hash;
mod chain_headers;
mod chain_publisher;
mod chain_sequence;
mod extend;
mod replay_verifier;
mod verify_chain;

pub use canonical_event_bytes::{CanonicalEventBytes, CanonicalizeError};
pub use chain_break::ChainBreak;
pub use chain_entry::ChainEntry;
pub use chain_hash::{ChainHash, ChainHashParseError};
pub use chain_headers::{
    CHAIN_HASH_HEADER, CHAIN_PREVIOUS_HASH_HEADER, ChainHeaderError, read_chain_headers, write_chain_headers,
};
pub use chain_publisher::{ChainPublishError, ChainPublisher};
pub use chain_sequence::{ChainSequence, InvalidChainSequence};
pub use extend::extend;
pub use replay_verifier::{ReplayVerifyError, verify_stream_chain};
pub use verify_chain::verify_chain;
