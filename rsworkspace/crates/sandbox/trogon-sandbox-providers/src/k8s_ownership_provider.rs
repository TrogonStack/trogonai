//! Receipt-based ownership, request persistence and lease recovery on [`K8sMicroVmProvider`].

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use k8s_openapi::api::core::v1::{Pod, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use trogon_sandbox_contract::NetworkPolicy;

use crate::{
    ConfirmedK8sLease, JOB_POD_POLL_INTERVAL, JobPodReadiness, K8sMicroVmProvider, K8sOwnershipError, K8sProviderError,
    K8sResourceKind, K8sResourceReceipt, LABEL_LEASE_ID, LABEL_WARM_POOL, LABEL_WORKER_IDENTITY, LEASE_REQUEST_KEY,
    WARM_POOL_CLAIMED, WARM_POOL_IDLE, build_network_policy_manifest, job_is_finished, job_outlived_deadline,
    job_pod_readiness, lease_job_secret_names, lease_request_secret_name, pod_is_running, warm_pod_outlived_deadline,
    warm_pod_restore_secret_name,
};
use crate::{egress_proxy_configmap_name, network_policy_name};

fn locked(receipts: &Mutex<Vec<K8sResourceReceipt>>) -> MutexGuard<'_, Vec<K8sResourceReceipt>> {
    receipts.lock().unwrap_or_else(PoisonError::into_inner)
}

fn job_pods_selector(job_name: &str) -> String {
    format!("job-name={job_name}")
}

impl K8sMicroVmProvider {
    pub fn worker_identity(&self) -> Option<&str> {
        self.config.worker_identity.as_deref()
    }

    pub fn with_resource_receipts(&self) -> Self {
        let mut scoped = self.with_lifecycle_emitter(self.lifecycle.clone());
        scoped.receipts = Some(Default::default());
        scoped
    }

    pub fn resource_receipts(&self) -> Vec<K8sResourceReceipt> {
        self.receipts
            .as_ref()
            .map(|receipts| locked(receipts).clone())
            .unwrap_or_default()
    }

    pub(crate) fn record_created_resource(
        &self,
        kind: K8sResourceKind,
        metadata: &ObjectMeta,
    ) -> Result<(), K8sProviderError> {
        if let Some(receipts) = &self.receipts {
            let receipt = K8sResourceReceipt::from_metadata(kind, &self.config.namespace, metadata)?;
            let mut receipts = locked(receipts);
            if !receipts.contains(&receipt) {
                receipts.push(receipt);
            }
        }
        Ok(())
    }

    pub(crate) async fn attach_recorded_resources(&self, owner: &K8sResourceReceipt) -> Result<(), K8sProviderError> {
        for receipt in self.resource_receipts() {
            if &receipt != owner {
                self.ops.set_resource_owner(&receipt, owner).await?;
            }
        }
        Ok(())
    }

    /// Deletes every resource, reporting the first failure after attempting all of them.
    pub async fn release_resources(&self, resources: &[K8sResourceReceipt]) -> Result<(), K8sProviderError> {
        let mut first_error = None;
        for resource in resources {
            if let Err(error) = self.ops.delete_resource(resource, None).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), |error| Err(error.into()))
    }

    pub async fn resource_is_held(&self, receipt: &K8sResourceReceipt) -> Result<bool, K8sProviderError> {
        let Some(metadata) = self
            .ops
            .resource_metadata(receipt.kind(), receipt.namespace(), receipt.name())
            .await?
        else {
            return Ok(false);
        };
        let observed = K8sResourceReceipt::from_metadata(receipt.kind(), receipt.namespace(), &metadata)?;
        Ok(observed == *receipt)
    }

    pub async fn owned_resource_receipts(
        &self,
        owner: &K8sResourceReceipt,
    ) -> Result<Vec<K8sResourceReceipt>, K8sProviderError> {
        let secret_names = if owner.kind() == K8sResourceKind::Job {
            lease_job_secret_names(owner.name())
        } else {
            vec![lease_request_secret_name(owner.name())]
        };
        let mut resources = Vec::new();
        for (kind, name) in secret_names
            .into_iter()
            .map(|name| (K8sResourceKind::Secret, name))
            .chain([
                (K8sResourceKind::NetworkPolicy, network_policy_name(owner.name())),
                (K8sResourceKind::ConfigMap, egress_proxy_configmap_name(owner.name())),
            ])
        {
            if let Some(metadata) = self.ops.resource_metadata(kind, owner.namespace(), &name).await?
                && owner.owns(&metadata)
            {
                resources.push(K8sResourceReceipt::from_metadata(kind, owner.namespace(), &metadata)?);
            }
        }
        Ok(resources)
    }

    pub async fn record_owned_secret(&self, owner: &K8sResourceReceipt, name: &str) -> Result<(), K8sProviderError> {
        let metadata = self
            .ops
            .resource_metadata(K8sResourceKind::Secret, owner.namespace(), name)
            .await?
            .ok_or(K8sOwnershipError::RestoreSecretNotConfirmed)?;
        if !owner.owns(&metadata) {
            return Err(K8sOwnershipError::RestoreSecretNotOwned.into());
        }
        self.record_created_resource(K8sResourceKind::Secret, &metadata)
    }

    pub async fn store_owned_lease_request(
        &self,
        owner: &K8sResourceReceipt,
        lease_id: &str,
        request_json: &[u8],
    ) -> Result<K8sResourceReceipt, K8sProviderError> {
        if !self.resource_is_held(owner).await? {
            return Err(K8sOwnershipError::RequestOwnerReplaced.into());
        }
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(lease_request_secret_name(owner.name())),
                namespace: Some(owner.namespace().to_string()),
                labels: Some(BTreeMap::from([(LABEL_LEASE_ID.to_string(), lease_id.to_string())])),
                owner_references: Some(vec![owner.owner_reference()]),
                ..Default::default()
            },
            type_: Some("Opaque".to_string()),
            data: Some(BTreeMap::from([(
                LEASE_REQUEST_KEY.to_string(),
                k8s_openapi::ByteString(request_json.to_vec()),
            )])),
            ..Default::default()
        };
        let created = self.ops.put_owned_secret(owner, secret).await?;
        self.record_created_resource(K8sResourceKind::Secret, &created.metadata)?;
        Ok(K8sResourceReceipt::from_metadata(
            K8sResourceKind::Secret,
            owner.namespace(),
            &created.metadata,
        )?)
    }

    pub async fn push_owned_network_policy(
        &self,
        owner: &K8sResourceReceipt,
        previous: Option<&K8sResourceReceipt>,
        policy: &NetworkPolicy,
        labels: &BTreeMap<String, String>,
    ) -> Result<Option<K8sResourceReceipt>, K8sProviderError> {
        if !self.resource_is_held(owner).await? {
            return Err(K8sOwnershipError::NetworkPolicyOwnerReplaced.into());
        }
        let mut manifest = build_network_policy_manifest(labels, policy, owner.namespace(), owner.name());
        if let Some(manifest) = &mut manifest {
            manifest.metadata.owner_references = Some(vec![owner.owner_reference()]);
        }
        let applied = self.ops.apply_owned_network_policy(owner, previous, manifest).await?;
        Ok(applied
            .map(|applied| {
                K8sResourceReceipt::from_metadata(K8sResourceKind::NetworkPolicy, owner.namespace(), &applied.metadata)
            })
            .transpose()?)
    }

    /// Evicts one warm pod that was observed idle, with the restore Secret it owns. Returns `false` when the pod
    /// changed between the observation and the deletion, so the caller re-observes instead of retrying blindly.
    pub async fn delete_idle_warm_pod(&self, pod: &Pod) -> Result<bool, K8sProviderError> {
        let observed_idle = pod
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(LABEL_WARM_POOL))
            .map(String::as_str)
            == Some(WARM_POOL_IDLE);
        if !observed_idle {
            return Err(K8sOwnershipError::WarmEvictionNotIdle.into());
        }
        let owner = K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, &self.config.namespace, &pod.metadata)?;
        let revision = pod
            .metadata
            .resource_version
            .as_deref()
            .ok_or(K8sOwnershipError::WarmPodRevisionMissing)?;
        let secret = match warm_pod_restore_secret_name(pod) {
            Some(name) => self
                .ops
                .resource_metadata(K8sResourceKind::Secret, owner.namespace(), &name)
                .await?
                .filter(|metadata| owner.owns(metadata))
                .map(|metadata| {
                    K8sResourceReceipt::from_metadata(K8sResourceKind::Secret, owner.namespace(), &metadata)
                })
                .transpose()?,
            None => None,
        };
        if let Err(error) = self.ops.delete_resource(&owner, Some(revision)).await {
            let current = self
                .ops
                .resource_metadata(K8sResourceKind::Pod, owner.namespace(), owner.name())
                .await?;
            if current.is_some_and(|metadata| {
                metadata.uid.as_deref() == Some(owner.uid()) && metadata.resource_version.as_deref() != Some(revision)
            }) {
                return Ok(false);
            }
            return Err(error.into());
        }
        if let Some(secret) = secret {
            if let Some(receipts) = &self.receipts {
                locked(receipts).push(secret.clone());
            }
            self.ops.delete_resource(&secret, None).await?;
        }
        Ok(true)
    }

    /// Waits for the pod `owner` created to be ready, recording every pod of the job it sees.
    pub async fn wait_for_owned_job_pod(
        &self,
        owner: &K8sResourceReceipt,
        timeout: Duration,
    ) -> Result<K8sResourceReceipt, K8sProviderError> {
        if owner.kind() != K8sResourceKind::Job || owner.namespace() != self.config.namespace {
            return Err(K8sOwnershipError::JobOutsideNamespace.into());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self.resource_is_held(owner).await? {
                return Err(K8sOwnershipError::JobReplaced.into());
            }
            let listing = self
                .ops
                .list_pods(owner.namespace(), &job_pods_selector(owner.name()))
                .await?;
            for pod in listing.pods {
                if !owner.owns(&pod.metadata) {
                    continue;
                }
                self.record_created_resource(K8sResourceKind::Pod, &pod.metadata)?;
                match job_pod_readiness(&pod) {
                    JobPodReadiness::Ready => {
                        return Ok(K8sResourceReceipt::from_metadata(
                            K8sResourceKind::Pod,
                            owner.namespace(),
                            &pod.metadata,
                        )?);
                    }
                    JobPodReadiness::Failed(reason) => return Err(K8sOwnershipError::PodFailedToStart(reason).into()),
                    JobPodReadiness::Starting => {}
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(K8sOwnershipError::PodStartTimeout {
                    job: owner.name().to_string(),
                    seconds: timeout.as_secs(),
                }
                .into());
            }
            tokio::time::sleep(JOB_POD_POLL_INTERVAL).await;
        }
    }

    /// Every lease of this worker's identity that the cluster still shows, with the objects each one owns. A
    /// provider without a worker identity recovers nothing.
    pub async fn confirmed_lease_inventory(&self) -> Result<Vec<ConfirmedK8sLease>, K8sProviderError> {
        let Some(identity) = self.worker_identity().filter(|identity| !identity.is_empty()) else {
            return Ok(Vec::new());
        };
        let selector = format!("{LABEL_WORKER_IDENTITY}={identity},{LABEL_LEASE_ID}");
        let mut leases = self.confirmed_job_leases(identity, &selector).await?;
        leases.extend(self.confirmed_warm_leases(identity, &selector).await?);
        Ok(leases)
    }

    async fn confirmed_job_leases(
        &self,
        identity: &str,
        selector: &str,
    ) -> Result<Vec<ConfirmedK8sLease>, K8sProviderError> {
        let listing = self.ops.list_jobs(&self.config.namespace, selector).await?;
        let mut leases = Vec::new();
        for job in listing.jobs {
            let Some(labels) = job.metadata.labels.as_ref() else {
                continue;
            };
            if labels.get(LABEL_WORKER_IDENTITY).map(String::as_str) != Some(identity) {
                continue;
            }
            let Some(lease_id) = labels.get(LABEL_LEASE_ID) else {
                continue;
            };
            let Ok(owner) =
                K8sResourceReceipt::from_metadata(K8sResourceKind::Job, &self.config.namespace, &job.metadata)
            else {
                continue;
            };
            let expired = job.metadata.deletion_timestamp.is_some()
                || job_is_finished(&job)
                || listing.clock.is_some_and(|now| job_outlived_deadline(&job, now));
            let pods = self
                .ops
                .list_pods(owner.namespace(), &job_pods_selector(owner.name()))
                .await?;
            let owned_pods: Vec<(&Pod, K8sResourceReceipt)> = pods
                .pods
                .iter()
                .filter(|pod| owner.owns(&pod.metadata))
                .filter_map(|pod| {
                    K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, owner.namespace(), &pod.metadata)
                        .ok()
                        .map(|receipt| (pod, receipt))
                })
                .collect();
            let pod = owned_pods
                .iter()
                .find(|(pod, _)| {
                    job_pod_readiness(pod) == JobPodReadiness::Ready && pod.metadata.deletion_timestamp.is_none()
                })
                .map(|(_, receipt)| receipt.clone());
            let mut resources = vec![owner.clone()];
            resources.extend(owned_pods.into_iter().map(|(_, receipt)| receipt));
            resources.extend(self.owned_resource_receipts(&owner).await?);
            leases.push(ConfirmedK8sLease {
                lease_id: lease_id.clone(),
                owner,
                pod,
                resources,
                expired,
            });
        }
        Ok(leases)
    }

    async fn confirmed_warm_leases(
        &self,
        identity: &str,
        selector: &str,
    ) -> Result<Vec<ConfirmedK8sLease>, K8sProviderError> {
        let listing = self
            .ops
            .list_pods(
                &self.config.namespace,
                &format!("{selector},{LABEL_WARM_POOL}={WARM_POOL_CLAIMED}"),
            )
            .await?;
        let mut leases = Vec::new();
        for pod in listing.pods {
            let Some(labels) = pod.metadata.labels.as_ref() else {
                continue;
            };
            if labels.get(LABEL_WORKER_IDENTITY).map(String::as_str) != Some(identity)
                || labels.get(LABEL_WARM_POOL).map(String::as_str) != Some(WARM_POOL_CLAIMED)
            {
                continue;
            }
            let Some(lease_id) = labels.get(LABEL_LEASE_ID) else {
                continue;
            };
            let Ok(owner) =
                K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, &self.config.namespace, &pod.metadata)
            else {
                continue;
            };
            let mut resources = vec![owner.clone()];
            resources.extend(self.owned_resource_receipts(&owner).await?);
            if let Some(secret) = warm_pod_restore_secret_name(&pod)
                && let Some(metadata) = self
                    .ops
                    .resource_metadata(K8sResourceKind::Secret, owner.namespace(), &secret)
                    .await?
                && owner.owns(&metadata)
                && let Ok(receipt) =
                    K8sResourceReceipt::from_metadata(K8sResourceKind::Secret, owner.namespace(), &metadata)
            {
                resources.push(receipt);
            }
            leases.push(ConfirmedK8sLease {
                lease_id: lease_id.clone(),
                owner: owner.clone(),
                pod: (pod_is_running(&pod) && pod.metadata.deletion_timestamp.is_none()).then_some(owner),
                resources,
                expired: pod.metadata.deletion_timestamp.is_some()
                    || listing.clock.is_some_and(|now| warm_pod_outlived_deadline(&pod, now)),
            });
        }
        Ok(leases)
    }
}

#[cfg(test)]
mod tests;
