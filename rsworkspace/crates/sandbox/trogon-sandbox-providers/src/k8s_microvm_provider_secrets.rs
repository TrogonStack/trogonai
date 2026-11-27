use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};

use crate::k8s_ops::constants::LEASE_REQUEST_KEY;
use crate::k8s_ops::{LeaseOwnerKind, lease_request_secret_name};
use crate::{
    K8sMicroVmProvider, K8sProviderError, K8sResourceKind, LABEL_LEASE_ID, build_workspace_tar_secret_with_naming,
};

impl K8sMicroVmProvider {
    pub async fn create_workspace_tar_secret(
        &self,
        secret_name: &str,
        job_name: &str,
        tar_bytes: &[u8],
    ) -> Result<(), K8sProviderError> {
        let secret = build_workspace_tar_secret_with_naming(
            secret_name,
            &self.config.namespace,
            job_name,
            tar_bytes,
            self.config.resource_naming,
        );
        let created = self.ops.create_secret(&self.config.namespace, secret).await?;
        self.record_created_resource(K8sResourceKind::Secret, &created.metadata)?;
        Ok(())
    }

    /// Stores the serialized request of `lease_id` in a Secret owned by the
    /// lease's Job or warm pod `owner_name`, replacing an earlier copy, and
    /// returns the Secret's name. The request can carry env values, which is
    /// why it lives in a Secret and not on the Job.
    pub async fn store_lease_request(
        &self,
        kind: LeaseOwnerKind,
        owner_name: &str,
        lease_id: &str,
        request_json: &[u8],
    ) -> Result<String, K8sProviderError> {
        let selector = format!("{LABEL_LEASE_ID}={lease_id}");
        let (api_version, kind_name, uid) = match kind {
            LeaseOwnerKind::Job => (
                "batch/v1",
                "Job",
                self.ops
                    .list_jobs(&self.config.namespace, &selector)
                    .await?
                    .jobs
                    .into_iter()
                    .find(|job| job.metadata.name.as_deref() == Some(owner_name))
                    .and_then(|job| job.metadata.uid),
            ),
            LeaseOwnerKind::Pod => (
                "v1",
                "Pod",
                self.ops
                    .list_pods(&self.config.namespace, &selector)
                    .await?
                    .pods
                    .into_iter()
                    .find(|pod| pod.metadata.name.as_deref() == Some(owner_name))
                    .and_then(|pod| pod.metadata.uid),
            ),
        };
        let uid = uid.ok_or_else(|| K8sProviderError::LeaseOwnerNotFound {
            kind: kind_name,
            name: owner_name.to_string(),
        })?;
        let name = lease_request_secret_name(owner_name);
        let mut labels = BTreeMap::new();
        labels.insert(
            "app.kubernetes.io/managed-by".to_string(),
            self.config.resource_naming.managed_by().to_string(),
        );
        labels.insert(
            self.config.resource_naming.job_label().to_string(),
            owner_name.to_string(),
        );
        let mut data = BTreeMap::new();
        data.insert(
            LEASE_REQUEST_KEY.to_string(),
            k8s_openapi::ByteString(request_json.to_vec()),
        );
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(labels),
                owner_references: Some(vec![OwnerReference {
                    api_version: api_version.to_string(),
                    kind: kind_name.to_string(),
                    name: owner_name.to_string(),
                    uid,
                    ..Default::default()
                }]),
                ..Default::default()
            },
            type_: Some("Opaque".to_string()),
            data: Some(data),
            ..Default::default()
        };
        self.ops.put_secret(&self.config.namespace, secret).await?;
        Ok(name)
    }

    /// The serialized request [`Self::store_lease_request`] kept for the Job
    /// or warm pod `owner_name`, if any.
    pub async fn load_lease_request(&self, owner_name: &str) -> Result<Option<Vec<u8>>, K8sProviderError> {
        Ok(self
            .ops
            .read_secret_key(
                &self.config.namespace,
                &lease_request_secret_name(owner_name),
                LEASE_REQUEST_KEY,
            )
            .await?)
    }

    /// Deletes the Secret `name` in the worker's namespace; a missing Secret
    /// is fine. For a Secret created ahead of a pod that then failed to
    /// appear, which no pod owns and so nothing else would reclaim.
    pub async fn delete_secret_named(&self, name: &str) -> Result<(), K8sProviderError> {
        Ok(self.ops.delete_secret(&self.config.namespace, name).await?)
    }
}
