use std::sync::OnceLock;

use async_nats::jetstream::{self, kv};
use base64::Engine;
use tokio::sync::OnceCell;
use trogon_sandbox_contract::{LeaseAttemptIdentity, TenantId};

use super::assignment::Assignment;
use super::assignments_error::AssignmentsError;
use super::constants::{BUCKET, MAX_RECORD_BYTES, MAX_ROUNDS};
use super::recovery::Recovery;
use super::sandbox_id::validate_sandbox_id;
use super::stored::Stored;
use crate::bounded_retry::bounded_retry;
use crate::custody_bucket::{ensure_durable, open_custody_bucket};
use crate::kv_outcome::{created, updated};

/// The KV store of assignments, bound to JetStream on first use.
#[derive(Default)]
pub(crate) struct Assignments {
    context: OnceLock<jetstream::Context>,
    store: OnceCell<kv::Store>,
}

fn key(identity: &LeaseAttemptIdentity) -> Result<String, AssignmentsError> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&(&identity.target.worker_id, &identity.lease_id))?))
}

impl Assignments {
    pub(crate) async fn configure(&self, context: jetstream::Context) -> Result<(), AssignmentsError> {
        self.context
            .set(context)
            .map_err(|_| AssignmentsError::AlreadyConfigured)?;
        self.storage().await?;
        Ok(())
    }

    async fn storage(&self) -> Result<&kv::Store, AssignmentsError> {
        let context = self.context.get().ok_or(AssignmentsError::NotConfigured)?;
        let store = self
            .store
            .get_or_try_init(|| {
                open_custody_bucket(
                    context,
                    kv::Config {
                        bucket: BUCKET.into(),
                        history: 1,
                        max_value_size: i32::try_from(MAX_RECORD_BYTES).unwrap_or(i32::MAX),
                        ..Default::default()
                    },
                )
            })
            .await?;
        ensure_durable(store).await?;
        Ok(store)
    }

    async fn stored(&self, identity: &LeaseAttemptIdentity) -> Result<Option<Stored>, AssignmentsError> {
        let Some(entry) = self.storage().await?.entry(key(identity)?).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put || entry.value.len() > MAX_RECORD_BYTES {
            return Err(AssignmentsError::UnreadableMarker);
        }
        let assignment: Assignment = serde_json::from_slice(&entry.value)?;
        assignment.validate()?;
        if assignment.identity != *identity {
            return Err(AssignmentsError::IdentityChanged);
        }
        Ok(Some(Stored {
            assignment,
            revision: entry.revision,
        }))
    }

    pub(crate) async fn load(&self, identity: &LeaseAttemptIdentity) -> Result<Option<Assignment>, AssignmentsError> {
        Ok(self.stored(identity).await?.map(|stored| stored.assignment))
    }

    /// Claims the assignment for `tenant`, or returns the one already recorded, adding `recovery` when it was
    /// missing.
    pub(crate) async fn reserve(
        &self,
        tenant: &TenantId,
        identity: &LeaseAttemptIdentity,
        recovery: Option<Recovery>,
    ) -> Result<Assignment, AssignmentsError> {
        bounded_retry(MAX_ROUNDS, AssignmentsError::Contended, || async {
            match self.stored(identity).await? {
                None => {
                    let assignment = Assignment::reserved(tenant.clone(), identity.clone(), recovery.clone());
                    let bytes = assignment.bytes()?;
                    let outcome = created(self.storage().await?.create(key(identity)?, bytes).await)?;
                    Ok(outcome.map(|_| assignment))
                }
                Some(stored) => self.adopt(tenant, identity, recovery.as_ref(), stored).await,
            }
        })
        .await
    }

    async fn adopt(
        &self,
        tenant: &TenantId,
        identity: &LeaseAttemptIdentity,
        recovery: Option<&Recovery>,
        stored: Stored,
    ) -> Result<Option<Assignment>, AssignmentsError> {
        if stored.assignment.tenant != *tenant {
            return Err(AssignmentsError::TenantChanged);
        }
        let Some(recovery) = recovery else {
            return Ok(Some(stored.assignment));
        };
        if let Some(existing) = &stored.assignment.recovery {
            if existing != recovery {
                return Err(AssignmentsError::RecoveryChanged);
            }
            return Ok(Some(stored.assignment));
        }
        let mut next = stored.assignment;
        next.recovery = Some(recovery.clone());
        let revision = self
            .storage()
            .await?
            .update(key(identity)?, next.bytes()?, stored.revision)
            .await;
        Ok(updated(revision)?.map(|_| next))
    }

    /// Records `sandbox_id` as the native sandbox the assignment holds.
    pub(crate) async fn bind_native(
        &self,
        identity: &LeaseAttemptIdentity,
        sandbox_id: &str,
    ) -> Result<(), AssignmentsError> {
        validate_sandbox_id(sandbox_id)?;
        bounded_retry(MAX_ROUNDS, AssignmentsError::NativeContended, || async {
            let stored = self.stored(identity).await?.ok_or(AssignmentsError::LacksOwnership)?;
            if let Some(existing) = &stored.assignment.sandbox_id {
                if existing != sandbox_id {
                    return Err(AssignmentsError::NativeIdentityChanged);
                }
                return Ok(Some(()));
            }
            let mut next = stored.assignment;
            next.sandbox_id = Some(sandbox_id.into());
            let revision = self
                .storage()
                .await?
                .update(key(identity)?, next.bytes()?, stored.revision)
                .await;
            Ok(updated(revision)?.map(|_| ()))
        })
        .await
    }
}
