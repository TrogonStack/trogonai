use async_nats::jetstream::Context;
use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseId, Port, ProviderLease};
use trogon_sandbox_protocol::{AttemptPhase, global_verifier};

use super::constants::STATE_RUNNING;
use super::{ActiveLease, E2bDispatchError, E2bWorkerProvider, LeaseAccess, LeaseLifetime, PauseOwnership};
use crate::api::validate_domain;
use crate::assignments::Assignment;
use crate::envd::EnvdTarget;

impl E2bWorkerProvider {
    /// Reconciles the durable dispatch ledger with the native resources this worker owns after a restart.
    pub(crate) async fn initialize_dispatch(&self, context: Context) -> Result<(), E2bDispatchError> {
        let Some(ledger) = self.dispatch_ledger() else {
            return Ok(());
        };
        self.exec
            .configure_worker_command_launches(context.clone(), global_verifier())?;
        self.assignments.configure(context).await?;
        for state in ledger.try_states().await? {
            let mut assignment = None;
            if state.allocation_started {
                let retained = self
                    .assignments
                    .load(&state.identity)
                    .await?
                    .ok_or(E2bDispatchError::LedgerLacksAssignment)?;
                Self::verify_recovered_tenant(&state.identity, &retained)?;
                assignment = Some(retained);
            }
            if let Some(receipt) = &state.receipt {
                Self::verify_receipt_identity(&state.identity, receipt)?;
                self.assignments
                    .bind_native(&state.identity, receipt.sandbox_id.as_str())
                    .await?;
                assignment = self.assignments.load(&state.identity).await?;
            }
            if state.allocation_started {
                if state.phase != AttemptPhase::Ready || state.backend_cleaned {
                    return Err(E2bDispatchError::UnsettledNotRetained);
                }
                let receipt = state.receipt.as_ref().ok_or(E2bDispatchError::ReadyLacksReceipt)?;
                let assignment = assignment.ok_or(E2bDispatchError::ReadyLacksOwnership)?;
                self.adopt_native_assignment(&state.identity, receipt, assignment)
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) fn verify_receipt_identity(
        identity: &LeaseAttemptIdentity,
        receipt: &ProviderLease,
    ) -> Result<(), E2bDispatchError> {
        if receipt.worker_id.as_ref() != Some(&identity.target.worker_id)
            || receipt.attempt_id.as_ref() != Some(&identity.target.attempt_id)
            || receipt.epoch != identity.target.epoch
        {
            return Err(E2bDispatchError::ReceiptChangedAssignment);
        }
        Ok(())
    }

    pub(super) fn verify_adoption(
        identity: &LeaseAttemptIdentity,
        receipt: &ProviderLease,
        (retained, retained_sandbox): (&LeaseAttemptIdentity, Option<&str>),
    ) -> Result<(), E2bDispatchError> {
        if retained != identity || retained_sandbox != Some(receipt.sandbox_id.as_str()) {
            return Err(E2bDispatchError::AdoptionMismatch);
        }
        Ok(())
    }

    pub(super) fn verify_recovered_tenant(
        identity: &LeaseAttemptIdentity,
        assignment: &Assignment,
    ) -> Result<(), E2bDispatchError> {
        let tenant = global_verifier()
            .recovered_dispatch_tenant(&identity.lease_id, &identity.target)
            .ok_or(E2bDispatchError::RecoveryLacksTenantWitness)?;
        if assignment.tenant != tenant {
            return Err(E2bDispatchError::RecoveryTenantMismatch);
        }
        Ok(())
    }

    async fn adopt_native_assignment(
        &self,
        identity: &LeaseAttemptIdentity,
        receipt: &ProviderLease,
        assignment: Assignment,
    ) -> Result<(), E2bDispatchError> {
        let retained = (&assignment.identity, assignment.sandbox_id.as_deref());
        Self::verify_adoption(identity, receipt, retained)?;
        Self::verify_recovered_tenant(identity, &assignment)?;
        let recovery = assignment.recovery.ok_or(E2bDispatchError::AdoptionLacksRecovery)?;
        let access = self.api.get_native_access(receipt.sandbox_id.as_str()).await?;
        if access.sandbox_id != receipt.sandbox_id.as_str() || access.state != STATE_RUNNING {
            return Err(E2bDispatchError::NativeIdentityUnconfirmed);
        }
        let lifetime = LeaseLifetime::from_native(&access.end_at)?;
        let token = access.token.ok_or(E2bDispatchError::RecoveryLacksToken)?.into_value()?;
        let config = self.api.config();
        let domain = access.domain.unwrap_or_else(|| config.domain.clone());
        validate_domain(&domain)?;
        let target = EnvdTarget::for_sandbox(config, receipt.sandbox_id.as_str(), Some(&domain), Some(token))?;
        let gate = self.lease_gate(&identity.lease_id);
        let active = ActiveLease {
            access: LeaseAccess::Ready,
            tenant: assignment.tenant,
            gate,
            pause: PauseOwnership::Unowned,
            sandbox_id: receipt.sandbox_id.as_str().to_owned(),
            target,
            traffic_token: None,
            traffic_privacy: access.traffic_privacy,
            domain,
            exposed_ports: recovery.exposed_ports.into_iter().map(Port::get).collect(),
            lifetime,
            region: recovery.region,
        };
        self.install_recovered(&identity.lease_id, active).await
    }

    pub(super) async fn install_recovered(
        &self,
        lease_id: &LeaseId,
        recovered: ActiveLease,
    ) -> Result<(), E2bDispatchError> {
        let gate = recovered.gate.clone();
        let _held = gate.lock().await;
        let mut active = self.active.lock().await;
        if active.contains_key(lease_id.as_str()) {
            return Err(E2bDispatchError::RecoveryWouldReplace);
        }
        active.insert(lease_id.as_str().to_owned(), recovered);
        Ok(())
    }
}
