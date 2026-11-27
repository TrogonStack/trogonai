use std::collections::BTreeMap;

use trogon_sandbox_contract::{LeaseId, NetworkPolicy};
use trogon_sandbox_providers::{K8sResourceKind, LABEL_LEASE_ID};

use crate::policy_rules::ensure_policy_enforceable;
use crate::{K8sWorkerError, K8sWorkerProvider};

impl K8sWorkerProvider {
    /// Pushes an updated `NetworkPolicy` to `lease_id`'s running sandbox
    /// without restarting it. The lease's `NetworkPolicy` is created when it
    /// started open and had none, and deleted when the update opens it.
    pub async fn push_network_policy_for_lease(
        &self,
        lease_id: &LeaseId,
        policy: &NetworkPolicy,
    ) -> Result<(), K8sWorkerError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        ensure_policy_enforceable(Some(policy))?;
        let active = self
            .active
            .lock()
            .await
            .get(lease_id)
            .cloned()
            .ok_or_else(|| K8sWorkerError::NoActiveLease(lease_id.clone()))?;
        let mut sandbox_pod_labels = BTreeMap::new();
        if active.is_warm_pod {
            sandbox_pod_labels.insert(LABEL_LEASE_ID.to_string(), lease_id.as_str().to_string());
        } else {
            sandbox_pod_labels.insert(
                self.inner.resource_naming().job_label().to_string(),
                active.sandbox_id.clone(),
            );
        }
        let previous = active
            .resources
            .iter()
            .rev()
            .find(|receipt| receipt.kind() == K8sResourceKind::NetworkPolicy);
        let updated = self
            .inner
            .push_owned_network_policy(&active.owner, previous, policy, &sandbox_pod_labels)
            .await?;
        if let Some(updated) = updated
            && let Some(record) = self.active.lock().await.get_mut(lease_id)
        {
            record.resources.retain(|receipt| receipt != &updated);
            record.resources.push(updated);
        }
        self.record_policy_in_request(lease_id, policy).await;
        Ok(())
    }
}
