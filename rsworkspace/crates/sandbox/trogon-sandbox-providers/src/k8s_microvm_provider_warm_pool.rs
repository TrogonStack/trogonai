use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{
    Container, Pod, PodSpec, ResourceRequirements, SecretVolumeSource, SecurityContext, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use trogon_sandbox_contract::NetworkPolicy;

use crate::k8s_ops::constants::{
    ANNOTATION_DIGEST, ANNOTATION_SOURCE_REGISTRY, ANNOTATION_SOURCE_REPOSITORY, LABEL_DIGEST, LABEL_LEASE_DEADLINE,
    LABEL_LEASE_ID, LABEL_TENANT_ID, LABEL_WARM_POOL, LABEL_WORKER_IDENTITY, RESTORE_VOLUME_NAME, RUNTIME_TMP_VOLUME,
    SANDBOX_TERMINATION_GRACE_SECONDS, WARM_POOL_CLAIMED, WARM_POOL_IDLE, WORKSPACE_PATH,
};
use crate::k8s_ops::{
    container_resources, hardened_pod_security_context, keep_alive_command, pod_is_running, runtime_class_name,
    runtime_tmp_volume,
};
use crate::{
    ClaimedWarmPod, FencedPatchOutcome, K8sMicroVmProvider, K8sProviderError, K8sResourceKind, K8sResourceReceipt,
    PodListing, build_network_policy_manifest, build_workspace_import_init_container, digest_label_value,
    policy_needs_egress_proxy, warm_pod_restore_secret_name,
};

impl K8sMicroVmProvider {
    pub async fn list_warm_pool_pods(&self) -> Result<PodListing, K8sProviderError> {
        let mut listing = self.ops.list_pods(&self.config.namespace, LABEL_WARM_POOL).await?;
        for pod in &listing.pods {
            if let Some(labels) = &pod.metadata.labels {
                self.ensure_resource_naming(labels)?;
            }
        }
        listing.pods.retain(|pod| {
            pod.metadata
                .labels
                .as_ref()
                .is_some_and(|labels| self.config.resource_naming.owns_warm_labels(labels))
        });
        Ok(listing)
    }

    pub async fn delete_warm_pod(&self, pod_name: &str, secret_names: &[&str]) -> Result<(), K8sProviderError> {
        let namespace = self.config.namespace.as_str();
        self.ops.delete_pod(namespace, pod_name).await?;
        for secret_name in secret_names {
            self.ops.delete_secret(namespace, secret_name).await?;
        }
        Ok(())
    }

    /// Create a standalone warm-pool Pod with a pre-restored workspace snapshot.
    pub async fn create_warm_pod(
        &self,
        digest: &str,
        image: &str,
        restore_secret_name: &str,
        runtime_class: &str,
        cpu: f32,
        memory_mib: u32,
    ) -> Result<String, K8sProviderError> {
        self.create_warm_pod_from(digest, None, image, restore_secret_name, runtime_class, cpu, memory_mib)
            .await
    }

    /// Like [`Self::create_warm_pod`], and records `source` (registry and
    /// repository) on the pod so the pool can be rebuilt after a restart.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_warm_pod_from(
        &self,
        digest: &str,
        source: Option<(&str, &str)>,
        image: &str,
        restore_secret_name: &str,
        runtime_class: &str,
        cpu: f32,
        memory_mib: u32,
    ) -> Result<String, K8sProviderError> {
        let digest_slug = digest
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let prefix = format!("wp-{}-", &digest_slug[..digest_slug.len().min(24)]);

        let mut labels = BTreeMap::new();
        labels.insert(
            "app.kubernetes.io/managed-by".to_string(),
            self.config.resource_naming.managed_by().to_string(),
        );
        labels.insert(
            "app.kubernetes.io/name".to_string(),
            self.config.resource_naming.warm_pool_name().to_string(),
        );
        labels.insert(LABEL_WARM_POOL.to_string(), WARM_POOL_IDLE.to_string());
        labels.insert(LABEL_DIGEST.to_string(), digest_label_value(digest));

        // No creation-time annotation: the apiserver already stamps
        // `creationTimestamp`, and a second stamp from this worker's local
        // clock is exactly the value another replica must not compare against.
        let mut annotations = BTreeMap::from([(ANNOTATION_DIGEST.to_string(), digest.to_string())]);
        if let Some((registry, repository)) = source {
            annotations.insert(ANNOTATION_SOURCE_REGISTRY.to_string(), registry.to_string());
            annotations.insert(ANNOTATION_SOURCE_REPOSITORY.to_string(), repository.to_string());
        }

        let resource_map = container_resources(cpu, memory_mib);
        let default_ids = self.agent_ids_for_image(image).await;

        let init_containers = vec![build_workspace_import_init_container(
            "restore-snapshot",
            RESTORE_VOLUME_NAME,
        )];

        let agent_container = Container {
            name: "agent".to_string(),
            image: Some(image.to_string()),
            image_pull_policy: Some("IfNotPresent".to_string()),
            command: Some(keep_alive_command()),
            resources: Some(ResourceRequirements {
                limits: Some(resource_map.clone()),
                requests: Some(resource_map),
                ..Default::default()
            }),
            security_context: Some(SecurityContext {
                run_as_non_root: Some(true),
                run_as_user: default_ids.0,
                run_as_group: default_ids.1,
                read_only_root_filesystem: Some(true),
                capabilities: Some(k8s_openapi::api::core::v1::Capabilities {
                    drop: Some(vec!["ALL".to_string()]),
                    add: None,
                }),
                ..Default::default()
            }),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: "workspace".to_string(),
                    mount_path: WORKSPACE_PATH.to_string(),
                    ..Default::default()
                },
                VolumeMount {
                    name: RUNTIME_TMP_VOLUME.into(),
                    mount_path: "/tmp".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let image_pull_secrets: Option<Vec<k8s_openapi::api::core::v1::LocalObjectReference>> =
            if self.config.image_pull_secrets.is_empty() {
                None
            } else {
                Some(
                    self.config
                        .image_pull_secrets
                        .iter()
                        .map(|s| k8s_openapi::api::core::v1::LocalObjectReference { name: s.clone() })
                        .collect(),
                )
            };

        let pod = Pod {
            metadata: ObjectMeta {
                generate_name: Some(prefix),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(labels),
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: Some(PodSpec {
                runtime_class_name: runtime_class_name(runtime_class),
                restart_policy: Some("Never".to_string()),
                termination_grace_period_seconds: Some(SANDBOX_TERMINATION_GRACE_SECONDS),
                service_account_name: Some(self.config.service_account.clone()),
                automount_service_account_token: Some(false),
                security_context: Some(hardened_pod_security_context()),
                init_containers: Some(init_containers),
                containers: vec![agent_container],
                volumes: Some(vec![
                    Volume {
                        name: "workspace".to_string(),
                        empty_dir: Some(Default::default()),
                        ..Default::default()
                    },
                    runtime_tmp_volume(memory_mib),
                    Volume {
                        name: RESTORE_VOLUME_NAME.to_string(),
                        secret: Some(SecretVolumeSource {
                            secret_name: Some(restore_secret_name.to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ]),
                image_pull_secrets,
                ..Default::default()
            }),
            ..Default::default()
        };

        let created = self.ops.create_pod(&self.config.namespace, pod).await?;
        self.record_created_resource(K8sResourceKind::Pod, &created.metadata)?;
        if self.receipts.is_some() {
            let owner =
                K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, &self.config.namespace, &created.metadata)?;
            self.attach_recorded_resources(&owner).await?;
        }
        let name = created
            .metadata
            .name
            .clone()
            .ok_or(K8sProviderError::WarmPodWithoutName)?;
        // The Secret is created before the pod it belongs to, so ownership is
        // set afterwards. With it, deleting the pod by any path, including a
        // worker that no longer knows the Secret's name, also deletes the
        // Secret.
        match created.metadata.uid.clone().filter(|_| self.receipts.is_none()) {
            Some(uid) => {
                let owner = OwnerReference {
                    api_version: "v1".to_string(),
                    kind: "Pod".to_string(),
                    name: name.clone(),
                    uid,
                    ..Default::default()
                };
                if let Err(error) = self
                    .ops
                    .set_secret_owner(&self.config.namespace, restore_secret_name, owner)
                    .await
                {
                    tracing::warn!(pod = %name, secret = %restore_secret_name, error = %error, "failed to make the warm pod own its restore secret");
                }
            }
            None if self.receipts.is_none() => {
                tracing::warn!(pod = %name, "warm pod created without a uid, its restore secret has no owner");
            }
            None => {}
        }
        Ok(name)
    }

    /// Applies `policy` to a warm pod already claimed for `lease_id` and returns
    /// the name of the `NetworkPolicy` created, or `None` when the policy
    /// restricts nothing (`AllowAll`). A warm pod is created without any
    /// `NetworkPolicy`, so until this runs its egress is open. The lease must
    /// not be granted before it returns `Ok`.
    ///
    /// Refuses a policy that needs the egress proxy sidecar, since a running
    /// pod cannot be given one.
    pub async fn apply_network_policy_to_claimed_warm_pod(
        &self,
        pod_name: &str,
        lease_id: &str,
        policy: &NetworkPolicy,
    ) -> Result<Option<String>, K8sProviderError> {
        if policy_needs_egress_proxy(policy) {
            return Err(K8sProviderError::WarmPodPolicyNeedsEgressProxy);
        }
        let selector = BTreeMap::from([(LABEL_LEASE_ID.to_string(), lease_id.to_string())]);
        let Some(manifest) = build_network_policy_manifest(&selector, policy, &self.config.namespace, pod_name) else {
            return Ok(None);
        };
        let name = manifest.metadata.name.clone();
        let created = self
            .ops
            .create_network_policy(&self.config.namespace, manifest)
            .await
            .map_err(K8sProviderError::CreateWarmPodNetworkPolicy)?;
        self.record_created_resource(K8sResourceKind::NetworkPolicy, &created.metadata)?;
        if self.receipts.is_some() {
            let owner = self
                .resource_receipts()
                .into_iter()
                .find(|receipt| receipt.kind() == K8sResourceKind::Pod && receipt.name() == pod_name)
                .ok_or(K8sProviderError::WarmPolicyWithoutOwner)?;
            let kind = K8sResourceKind::NetworkPolicy;
            let receipt = K8sResourceReceipt::from_metadata(kind, &self.config.namespace, &created.metadata)?;
            self.ops.set_resource_owner(&receipt, &owner).await?;
        }
        Ok(name)
    }

    /// Deletes the `NetworkPolicy` named `name`, if present.
    pub async fn delete_network_policy(&self, name: &str) -> Result<(), K8sProviderError> {
        Ok(self.ops.delete_network_policy(&self.config.namespace, name).await?)
    }

    /// Claim an idle warm-pool Pod for a lease by flipping warm-pool labels.
    ///
    /// A non-zero `timeout` is stamped as [`LABEL_LEASE_DEADLINE`] on the
    /// apiserver clock of the listing, so [`Self::sweep_claimed_warm_pods`]
    /// can reclaim the pod once the lease runs out, the way a lease Job's
    /// `activeDeadlineSeconds` does. A zero timeout, or a listing with no
    /// clock, stamps no deadline and the pod lives until it is cancelled.
    pub async fn claim_warm_pod(
        &self,
        digest: &str,
        lease_id: &str,
        tenant_id: &str,
        timeout: std::time::Duration,
    ) -> Result<Option<ClaimedWarmPod>, K8sProviderError> {
        let selector = format!(
            "app.kubernetes.io/managed-by={},{LABEL_WARM_POOL}={WARM_POOL_IDLE},{LABEL_DIGEST}={}",
            self.config.resource_naming.managed_by(),
            digest_label_value(digest)
        );
        let listing = self.ops.list_pods(&self.config.namespace, &selector).await?;
        let deadline = match (timeout.is_zero(), listing.clock) {
            (true, _) => None,
            (false, Some(now)) => {
                let seconds = i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX);
                Some(now.as_second().saturating_add(seconds))
            }
            (false, None) => {
                tracing::warn!(lease_id = %lease_id, "no apiserver clock in the warm pod listing, the claimed pod gets no lease deadline");
                None
            }
        };
        for pod in listing.pods {
            if !pod
                .metadata
                .labels
                .as_ref()
                .is_some_and(|labels| self.config.resource_naming.owns_warm_labels(labels))
            {
                continue;
            }
            let Some(name) = pod.metadata.name.clone() else {
                continue;
            };
            if !pod_is_running(&pod) {
                continue;
            }
            if self.receipts.is_some()
                && K8sResourceReceipt::from_metadata(K8sResourceKind::Pod, &self.config.namespace, &pod.metadata)
                    .is_err()
            {
                continue;
            }
            // The claim is exclusive only if it is fenced: without a
            // `resourceVersion` to write against, two workers that listed the
            // same idle pod would both succeed and both believe they own it.
            let Some(resource_version) = pod.metadata.resource_version.clone() else {
                tracing::debug!(pod = %name, "warm pod has no resourceVersion, cannot fence a claim");
                continue;
            };
            let mut labels = pod.metadata.labels.clone().unwrap_or_default();
            labels.insert(LABEL_WARM_POOL.to_string(), WARM_POOL_CLAIMED.to_string());
            labels.insert(LABEL_LEASE_ID.to_string(), lease_id.to_string());
            labels.insert(LABEL_TENANT_ID.to_string(), tenant_id.to_string());
            if let Some(identity) = &self.config.worker_identity {
                labels.insert(LABEL_WORKER_IDENTITY.to_string(), identity.clone());
            }
            match deadline {
                Some(deadline) => {
                    labels.insert(LABEL_LEASE_DEADLINE.to_string(), deadline.to_string());
                }
                None => {
                    labels.remove(LABEL_LEASE_DEADLINE);
                }
            }
            match self
                .ops
                .patch_pod_labels(&self.config.namespace, &name, labels, &resource_version)
                .await
            {
                Ok(FencedPatchOutcome::Applied) => {
                    self.record_created_resource(K8sResourceKind::Pod, &pod.metadata)?;
                    return Ok(Some(ClaimedWarmPod {
                        restore_secret_name: warm_pod_restore_secret_name(&pod),
                        name,
                    }));
                }
                Ok(FencedPatchOutcome::PreconditionFailed) => {
                    tracing::debug!(pod = %name, "warm pod claimed by another worker, trying next");
                }
                Err(e) => {
                    tracing::debug!(pod = %name, error = %e, "warm pod claim failed, trying next");
                }
            }
        }
        Ok(None)
    }
}
