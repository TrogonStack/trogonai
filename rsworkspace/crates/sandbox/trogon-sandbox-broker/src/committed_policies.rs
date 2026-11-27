//! Commits each lease's credential policy for the credential broker's fence.

mod committed_policies_error;

use std::sync::Arc;

use trogon_sandbox_credential_broker::{CommittedPolicyWriter, NatsKvCommittedPolicies};

pub use committed_policies_error::CommittedPoliciesError;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Commit each lease's credential policy (the credential references its
    /// spec names, with a revision that grows on every policy update) into the
    /// NATS KV bucket the credential broker's fence reads, and delete it when
    /// the lease is released. Takes effect with the `nats-kv` quota backend;
    /// the in-memory backend has no store to share.
    #[arg(
        long = "commit-credential-policies",
        env = "SANDBOX_COMMIT_CREDENTIAL_POLICIES",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub enabled: bool,
}

impl Args {
    pub async fn writer(
        &self,
        nats: &async_nats::Client,
        shared_store: bool,
    ) -> Result<Option<Arc<dyn CommittedPolicyWriter>>, CommittedPoliciesError> {
        if !(self.enabled && shared_store) {
            return Ok(None);
        }
        let js = async_nats::jetstream::new(nats.clone());
        let bucket = NatsKvCommittedPolicies::open_bucket(&js).await?;
        Ok(Some(Arc::new(NatsKvCommittedPolicies::new(bucket))))
    }
}

#[cfg(test)]
mod tests;
