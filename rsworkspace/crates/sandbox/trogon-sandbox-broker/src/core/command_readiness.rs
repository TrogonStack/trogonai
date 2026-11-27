use bytes::Bytes;
use trogon_sandbox_contract::{
    CommandReadinessRequest, CommandReadinessResponse, CommandReadinessRuntime, CommandReadinessTarget, LeaseRecord,
    LeaseStatus, SandboxId, SandboxUserName, WorkerHeartbeat,
};
use trogon_sandbox_protocol::VerbScope;

use super::BrokerCore;
use crate::constants::{COMMAND_READINESS_MAX_RESPONSE_BYTES, COMMAND_READINESS_QUERY_TIMEOUT};

/// Whether `heartbeat` is a targeted-dispatch worker heartbeat for `record`'s provider and the expected worker.
fn heartbeat_serves(
    heartbeat: &WorkerHeartbeat,
    record: &LeaseRecord,
    worker: &trogon_sandbox_contract::WorkerId,
    expected_profile: Option<&trogon_sandbox_contract::ProviderCapabilityProfile>,
) -> bool {
    heartbeat.worker_id == *worker
        && heartbeat.provider == record.provider
        && heartbeat.profile.provider == record.provider
        && heartbeat.dispatch_protocol == trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
        && heartbeat.supports_lease_epoch.is_supported()
        && expected_profile.is_none_or(|profile| heartbeat.profile == *profile)
}

impl BrokerCore {
    pub(super) async fn command_readiness(
        &self,
        record: &LeaseRecord,
        run_as_user: Option<&SandboxUserName>,
        expected_profile: Option<&trogon_sandbox_contract::ProviderCapabilityProfile>,
    ) -> Option<CommandReadinessRuntime> {
        let (Some(worker), Some(attempt), Some(epoch)) = (
            record.worker_id.as_ref(),
            record.dispatch_attempt.as_ref(),
            record.epoch,
        ) else {
            return None;
        };
        if record.status != LeaseStatus::Running
            || record.deadline <= std::time::SystemTime::now()
            || record.freeze_in_flight.is_some()
            || attempt.epoch != Some(epoch)
        {
            return None;
        }
        let heartbeat = self.worker_registry.current_worker(record.provider, worker).await?;
        if !heartbeat_serves(&heartbeat, record, worker, expected_profile) {
            return None;
        }
        let target = CommandReadinessTarget {
            worker_id: worker.clone(),
            attempt_id: attempt.id.clone(),
            epoch,
        };
        let request = CommandReadinessRequest::new(
            record.tenant.clone(),
            record.lease_id.clone(),
            record.provider,
            target,
            SandboxId::new(record.sandbox_id.clone()).ok()?,
            run_as_user.cloned(),
        )
        .ok()?;
        let subject =
            trogon_sandbox_protocol::subjects::worker_command_readiness(record.provider, worker, &record.lease_id);
        let payload = serde_json::to_vec(&request).ok()?;
        if payload.len() > CommandReadinessRequest::MAX_BYTES {
            return None;
        }
        let signer = self.dispatch_signer.as_ref()?;
        let scope = VerbScope {
            tenant: record.tenant.clone(),
            lease_id: record.lease_id.clone(),
            worker_id: Some(worker.clone()),
            attempt_id: Some(attempt.id.clone()),
            epoch: Some(epoch),
        };
        let auth = signer.sign_verb(&subject, &payload, &scope).ok()?;
        let response = self
            .bus
            .request_with_auth(
                &subject,
                &auth,
                Bytes::from(payload),
                Some(COMMAND_READINESS_QUERY_TIMEOUT),
            )
            .await
            .ok()?;
        if response.len() > COMMAND_READINESS_MAX_RESPONSE_BYTES {
            return None;
        }
        let response: CommandReadinessResponse = serde_json::from_slice(&response).ok()?;
        let CommandReadinessResponse::Ready { binding, runtime } = response else {
            return None;
        };
        if binding != request || runtime.protocol != CommandReadinessRuntime::PROTOCOL {
            return None;
        }
        let current_heartbeat = self.worker_registry.current_worker(record.provider, worker).await?;
        if !heartbeat_serves(&current_heartbeat, record, worker, expected_profile) {
            return None;
        }
        let current = self.active_leases.get(&record.lease_id).await.ok().flatten()?;
        if current.status != LeaseStatus::Running
            || current.deadline <= std::time::SystemTime::now()
            || current.freeze_in_flight.is_some()
            || current.tenant != record.tenant
            || current.provider != record.provider
            || current.sandbox_id != record.sandbox_id
            || current.worker_id != record.worker_id
            || current.dispatch_attempt != record.dispatch_attempt
            || current.epoch != record.epoch
        {
            return None;
        }
        Some(runtime)
    }

    pub(super) async fn command_readiness_for_grant(
        &self,
        grant: &trogon_sandbox_contract::LeaseGranted,
        captured: &LeaseRecord,
        profile: &trogon_sandbox_contract::ProviderCapabilityProfile,
    ) -> bool {
        if grant.provider != captured.provider
            || grant.sandbox_id != captured.sandbox_id
            || grant.lease_id != captured.lease_id
            || captured.status != LeaseStatus::Running
            || captured.deadline <= std::time::SystemTime::now()
            || captured.freeze_in_flight.is_some()
        {
            return false;
        }
        let Some(worker) = captured.worker_id.as_ref() else {
            return false;
        };
        let Some(heartbeat) = self.worker_registry.current_worker(captured.provider, worker).await else {
            return false;
        };
        if heartbeat.profile != *profile {
            return false;
        }
        self.command_readiness(captured, None, Some(profile)).await.is_some()
    }
}
