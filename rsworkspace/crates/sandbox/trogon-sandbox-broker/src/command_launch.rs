//! Launch-key journal for `run_command`: remembers, per lease, which
//! [`RunCommandRequest::launch_key`]s were already dispatched and what the
//! worker answered, so a client retrying after a lost reply gets the first
//! launch's response instead of running the command a second time.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_nats::jetstream::stream;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tokio::sync::{Mutex, OnceCell};
use trogon_sandbox_contract::{DispatchTarget, LeaseAttemptIdentity};
use trogon_sandbox_contract::{
    LaunchKey, LeaseId, LeaseRecord, RedactValues, RunCommandRequest, RunCommandResponse, TenantId,
};
use trogon_sandbox_protocol::VerbScope;

mod cancellation_authority_claim;
mod cancellation_custody_support;
mod claim;
mod command_cancellation_authority;
mod command_launch_store;
mod in_memory_command_launch_store;
mod launch_record;
mod launch_state;
mod nats_kv_command_launch_store;

pub use cancellation_authority_claim::CancellationAuthorityClaim;
pub use cancellation_custody_support::CancellationCustodySupport;
pub use claim::Claim;
pub use command_cancellation_authority::CommandCancellationAuthority;
pub use command_launch_store::CommandLaunchStore;
pub use in_memory_command_launch_store::InMemoryCommandLaunchStore;
pub use launch_record::CommandLaunch;
pub use launch_state::LaunchState;
pub use nats_kv_command_launch_store::NatsKvCommandLaunchStore;

pub use crate::command_launch_error::CommandLaunchError;

/// Identifies what a launch key was first used for: the request minus the
/// key itself and the redaction literals, which only shape the reply and
/// must never be hashed into anything persisted.
pub fn fingerprint(req: &RunCommandRequest) -> Result<String, serde_json::Error> {
    let mut identity = req.clone();
    identity.launch_key = None;
    identity.redact_values = RedactValues::default();
    let body = serde_json::to_vec(&identity)?;
    Ok(hex::encode(sha2::Sha256::digest(body)))
}

pub use crate::constants::LAUNCH_KEY_MAX_AGE;
use crate::constants::MAX_CANCELLATION_AUTHORITY_BYTES;

#[cfg(test)]
mod tests;
